//! The source line of code a package typeset from a file it generated.
//!
//! minted has Pygments write each code block, highlighted, to a file in its
//! cache folder (`_minted/<hash>.highlight.minted`, minted 2.x's
//! `_minted-<job>/<hash>.pygtex`) and inputs that file in place of the block.
//! SyncTeX records the code there, so `synctex edit` on a code line names the
//! generated file, and nothing the writer can edit: TeX recorded no line of
//! the block itself. The block is found again by its text instead.

use super::{input_names, records, Inputs};
use crate::util;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the writer wrote what `input` (canonical), a file in a package's
/// cache folder, typeset at its line `line` and the point `(x, y)` of `page`.
/// For minted's highlighted code, the line of the code block in the
/// project's `.tex` sources; for any other generated file (svg-inkscape's
/// `.pdf_tex`), or code no block holds, the nearest place on the page TeX
/// recorded a source of the project at. `None` for an input outside the
/// cache folders (a `cachedir=` of another name is not recognised).
pub(super) fn source(
    synctex: &str, inputs: &Inputs, root: &Path, input: &Path, line: u32, page: u32,
    point: (f64, f64),
) -> Option<(PathBuf, u32)> {
    let generated = |path: &Path| {
        path.strip_prefix(root).is_ok_and(|relative| {
            util::in_package_cache_folder(&relative.to_string_lossy().replace('\\', "/"))
        })
    };
    if !generated(input) {
        return None;
    }
    let project_source = |name: &str| {
        let path = inputs.path(name).canonicalize().ok()?;
        (path.starts_with(root) && !generated(&path)).then_some(path)
    };
    let mut seen = HashSet::new();
    let sources: Vec<(PathBuf, String)> = input_names(synctex)
        .filter_map(project_source)
        .filter(|path| {
            path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
                && seen.insert(path.clone())
        })
        .filter_map(|path| Some((path.clone(), fs::read_to_string(&path).ok()?)))
        .collect();
    let blocks = fs::read_to_string(input)
        .map(|highlighted| blocks(&highlighted, line, sources.iter().cloned()))
        .unwrap_or_default();
    if blocks.is_empty() {
        let (name, line, _) =
            records::source_at(synctex, page, point, |name| project_source(name).is_some())?;
        return Some((project_source(&name)?, line));
    }
    // minted names its file by the code, so blocks of the same code share
    // it, and TeX opens it once for each, declaring it again under a new tag:
    // the block clicked is the one TeX read that time. Nothing typeset around
    // it tells reliably (a float, a page's header and footer, two blocks in a
    // row), but the order TeX declared the file in does.
    let is_input = |name: &str| inputs.path(name).canonicalize().is_ok_and(|path| path == input);
    let time = records::source_at(synctex, page, point, is_input).map_or(0, |(.., tag)| {
        input_tags(synctex).filter(|(other, name)| *other < tag && is_input(name)).count()
    });
    let ordered = reading_order(&sources, &blocks);
    let block = ordered.get(time).or(ordered.first())?;
    Some((block.path.clone(), block.line))
}

/// Every input SyncTeX declares: its tag and its name, as TeX opened it.
fn input_tags(synctex: &str) -> impl Iterator<Item = (u32, &str)> {
    synctex.lines().filter_map(|record| {
        let (tag, name) = record.strip_prefix("Input:")?.split_once(':')?;
        Some((tag.parse().ok()?, name))
    })
}

/// `blocks` in the order TeX reads them: down the first of `sources` (the
/// document TeX opened first), into each source it `\input`s, `\include`s
/// or `\subfile`s where it does, then any block that leaves unread (a
/// source reached another way) in the order of `sources`.
fn reading_order<'a>(
    sources: &[(PathBuf, String)], blocks: &'a [Block<PathBuf>],
) -> Vec<&'a Block<PathBuf>> {
    let mut ordered = Vec::new();
    if let Some((document, _)) = sources.first() {
        let folder = document.parent().unwrap_or(document);
        read(document, folder, sources, blocks, &mut vec![], &mut ordered);
    }
    for block in blocks {
        if !ordered.iter().any(|seen| std::ptr::eq(*seen, block)) {
            ordered.push(block);
        }
    }
    ordered
}

/// Add `path`'s blocks to `ordered` line by line, reading each source it
/// includes where it does. TeX names an included file relative to the
/// document's `folder`, with `.tex` left off or not. `open` holds the sources
/// being read, so a file that includes itself ends.
fn read<'a>(
    path: &Path, folder: &Path, sources: &[(PathBuf, String)], blocks: &'a [Block<PathBuf>],
    open: &mut Vec<PathBuf>, ordered: &mut Vec<&'a Block<PathBuf>>,
) {
    let Some((_, text)) = sources.iter().find(|(source, _)| source == path) else { return };
    open.push(path.to_path_buf());
    for (index, line) in text.lines().enumerate() {
        let number = index as u32 + 1;
        ordered.extend(blocks.iter().filter(|block| block.path == path && block.start == number));
        let code = line.split('%').next().unwrap_or_default();
        for command in ["\\input{", "\\include{", "\\subfile{"] {
            for rest in code.split(command).skip(1) {
                let name = rest.split('}').next().unwrap_or_default().trim();
                let named = folder.join(name);
                let included = [named.with_extension("tex"), named]
                    .into_iter()
                    .filter_map(|candidate| candidate.canonicalize().ok())
                    .find(|candidate| sources.iter().any(|(source, _)| source == candidate));
                if let Some(included) = included.filter(|included| !open.contains(included)) {
                    read(&included, folder, sources, blocks, open, ordered);
                }
            }
        }
    }
    open.pop();
}

/// A minted environment holding a block's code: where its code starts, and
/// the line of the clicked code in it (1-based).
#[derive(Debug, PartialEq)]
struct Block<P> {
    path: P,
    start: u32,
    line: u32,
}

/// Every minted environment in `sources` (path and text, in order) holding
/// the code of `highlighted`, a minted cache file, with its line `line`'s
/// place in it. Empty when none does: an `\inputminted` file, code `gobble`
/// changed, or a source edited since the build.
fn blocks<P: Clone>(
    highlighted: &str, line: u32, sources: impl IntoIterator<Item = (P, String)>,
) -> Vec<Block<P>> {
    let lines: Vec<&str> = highlighted.lines().collect();
    // Pygments wraps the block in a verbatim environment (`Verbatim`, minted
    // 3's `MintedVerbatim`) on lines of their own.
    let begin = usize::from(lines.first().is_some_and(|text| text.starts_with("\\begin{")));
    let end =
        lines.len() - usize::from(lines.last().is_some_and(|text| text.starts_with("\\end{")));
    let Some(code) = lines.get(begin..end.max(begin)) else { return Vec::new() };
    let code: Vec<String> = code.iter().map(|text| highlighted_text(text)).collect();
    // Code with no letter or digit would match too much.
    if !code.iter().any(|text| text.chars().any(char::is_alphanumeric)) {
        return Vec::new();
    }
    let offset = (line as usize).saturating_sub(begin + 1).min(code.len() - 1);
    let mut found = Vec::new();
    for (path, source) in sources {
        let source: Vec<&str> = source.lines().collect();
        let text: Vec<String> = source.iter().map(|text| code_text(text)).collect();
        for (start, run) in text.windows(code.len()).enumerate() {
            let opens = || source[..start].iter().rev().find(|text| !text.trim().is_empty());
            if run == code.as_slice() && opens().is_some_and(|text| opens_minted(text)) {
                let start = start as u32 + 1;
                found.push(Block { path: path.clone(), start, line: start + offset as u32 });
            }
        }
    }
    found
}

/// Whether `line` opens a minted environment: `minted`, or one `\newminted`
/// made (`pythoncode`, `pythoncode*`). A `verbatim` or `lstlisting` holding
/// the same code is not where it was highlighted from.
fn opens_minted(line: &str) -> bool {
    line.split("\\begin{").skip(1).any(|rest| {
        let name = rest.split('}').next().unwrap_or_default().trim_end_matches('*');
        name == "minted" || name.ends_with("code")
    })
}

/// A line of code reduced to what survives highlighting unchanged: its
/// letters and digits, as Pygments escapes punctuation and spaces
/// (`\PYGZob{}` for `{`). A line of punctuation alone (`}`) is `\u{1}`, so
/// it matches no blank line.
fn code_text(line: &str) -> String {
    let text: String = line.chars().filter(|character| character.is_alphanumeric()).collect();
    if text.is_empty() && !line.trim().is_empty() {
        return "\u{1}".to_string();
    }
    text
}

/// A line Pygments highlighted, reduced as `code_text` reduces its source,
/// without its markup: `\PYG{k}{def}` (a token and its style) is `def`, and
/// an escape such as `\PYGZbs{}` (a backslash) is punctuation. Any other
/// command came from the code itself (an `escapeinside` escape), so it counts
/// as it is written.
fn highlighted_text(line: &str) -> String {
    let mut text = String::new();
    let mut punctuation = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            if character.is_alphanumeric() {
                text.push(character);
            }
            // Braces are the markup's own; Pygments escapes the code's.
            punctuation |= !character.is_whitespace() && !matches!(character, '{' | '}');
            continue;
        }
        let mut name = String::new();
        while let Some(letter) = characters.next_if(char::is_ascii_alphabetic) {
            name.push(letter);
        }
        if !name.starts_with("PYG") {
            text.push_str(&name);
            punctuation = true;
        } else if name.starts_with("PYGZ") {
            punctuation = true;
        } else if characters.next_if_eq(&'{').is_some() {
            // The token's style (`k`, `n+nf`) is markup, not code.
            characters.by_ref().find(|character| *character == '}');
        }
    }
    if text.is_empty() && punctuation {
        return "\u{1}".to_string();
    }
    text
}

#[cfg(test)]
mod tests;
