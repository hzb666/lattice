use super::*;
use crate::test_support::TempDir;

const HIGHLIGHTED: &str = "\\begin{Verbatim}[commandchars=\\\\\\{\\}]\n\
\\PYG{k}{def}\\PYG{+w}{ }\\PYG{n+nf}{f}\\PYG{p}{(}\\PYG{n}{x}\\PYG{p}{):}\n\
    \\PYG{k}{return} \\PYG{n}{x} \\PYG{o}{+} \\PYG{l+m+mi}{1}\n\
\n\
\\PYG{n+nb}{print}\\PYG{p}{(}\\PYG{l+s+s2}{\\PYGZdq{}}\\PYG{l+s+se}{\\PYGZbs{}n}\\PYG{l+s+s2}{\\PYGZdq{}}\\PYG{p}{)}\n\
\\end{Verbatim}\n";

const MAIN: &str = "\\documentclass{article}\n\\usepackage{minted}\n\\begin{document}\n\
Before.\n\n\\begin{minted}{python}\ndef f(x):\n    return x + 1\n\nprint(\"\\n\")\n\
\\end{minted}\n\nAfter.\n\\end{document}\n";

fn find(line: u32, sources: &[(&'static str, &str)]) -> Vec<(&'static str, u32)> {
    let sources = sources.iter().map(|(path, text)| (*path, text.to_string()));
    blocks(HIGHLIGHTED, line, sources).into_iter().map(|block| (block.path, block.line)).collect()
}

#[test]
fn a_highlighted_code_line_lands_on_its_line_in_the_block() {
    // Cache line 2 is the block's first line of code, source line 7.
    assert_eq!(find(2, &[("main.tex", MAIN)]), [("main.tex", 7)]);
    assert_eq!(find(3, &[("main.tex", MAIN)]), [("main.tex", 8)]);
    assert_eq!(find(5, &[("main.tex", MAIN)]), [("main.tex", 10)]);
    // The environment's own lines land on the code's first and last line.
    assert_eq!(find(1, &[("main.tex", MAIN)]), [("main.tex", 7)]);
    assert_eq!(find(6, &[("main.tex", MAIN)]), [("main.tex", 10)]);
}

#[test]
fn only_a_minted_environment_holding_the_same_code_is_its_block() {
    let chapter = MAIN.replace("x + 1", "x + 2");
    let sources = [("chapter.tex", chapter.as_str()), ("main.tex", MAIN)];
    assert_eq!(find(3, &sources), [("main.tex", 8)]);
    // Code no source holds is not guessed at.
    assert_eq!(find(3, &[("chapter.tex", chapter.as_str())]), []);
    // The same code in a comment, a verbatim copy or lstlisting is not a block;
    // in an environment `\newminted` made, it is.
    for (open, block) in
        [("%", false), ("\\begin{verbatim}", false), ("\\begin{lstlisting}", false)]
            .into_iter()
            .chain([("\\begin{pythoncode*}{linenos}", true)])
    {
        let copy = MAIN.replace("\\begin{minted}{python}", open);
        let expected: &[(&str, u32)] = if block { &[("main.tex", 8)] } else { &[] };
        assert_eq!(find(3, &[("main.tex", &copy)]), expected, "{open}");
    }
}

#[test]
fn strips_pygments_markup_and_escapes() {
    assert_eq!(highlighted_text("\\PYG{n+nf}{f}\\PYG{p}{(}\\PYG{n}{x}"), "fx");
    assert_eq!(highlighted_text("\\PYG{l+s+se}{\\PYGZbs{}n}\\PYGZob{}"), "n");
    // A command the code itself holds is part of the code.
    assert_eq!(highlighted_text("\\PYG{n}{a} \\textbf{b}"), "atextbfb");
    assert_eq!(code_text("print(\"\\n\") # {x}"), "printnx");
    // A line of punctuation is not a blank line, on either side.
    assert_eq!(highlighted_text("\\PYG{p}{\\PYGZcb{}}"), code_text("}"));
    assert_eq!(highlighted_text("    \\PYG{p}{):}"), code_text("  ):"));
    assert_ne!(code_text("}"), code_text("   "));
    assert_eq!(highlighted_text(""), code_text("   "));
}

/// A box SyncTeX recorded for `tag`'s line `line`, 200 x 10 bp with its
/// baseline `v` bp down the page.
fn record(tag: u32, line: u32, v: f64) -> String {
    let sp = |bp: f64| (bp * 65781.76) as i64;
    format!("({tag},{line}:{},{}:{},{},0\n)\n", sp(72.0), sp(v), sp(200.0), sp(10.0))
}

/// A project where TeX reads a block, a chapter opening with a block of the
/// same code, then two more of it in a row, all four from one minted cache
/// file; and a verbatim copy of that code before them.
fn project() -> (TempDir, PathBuf, String) {
    let parent = TempDir::new("synctex-highlighted");
    let minted = "\\begin{minted}{python}\ndef f(x):\n    return x + 1\n\\end{minted}\n";
    let verbatim = "\\begin{verbatim}\ndef f(x):\n    return x + 1\n\\end{verbatim}\n";
    parent.write(
        "main.tex",
        format!(
            "\\documentclass{{article}}\n\\usepackage{{minted}}\n\\begin{{document}}\nBefore.\n\
             {verbatim}{minted}\\input{{chapter}}\n{minted}{minted}\\end{{document}}\n"
        ),
    );
    parent.write("chapter.tex", minted);
    parent.write(
        "_minted/C.highlight.minted",
        "\\begin{MintedVerbatim}[commandchars=\\\\\\{\\}]\n\
         \\PYG{k}{def} \\PYG{n+nf}{f}\\PYG{p}{(}\\PYG{n}{x}\\PYG{p}{):}\n\
         \x20   \\PYG{k}{return} \\PYG{n}{x} \\PYG{o}{+} \\PYG{l+m+mi}{1}\n\\end{MintedVerbatim}\n",
    );
    parent.write(
        "_minted/D.highlight.minted",
        "\\begin{MintedVerbatim}\n\\PYG{n}{other}\n\\end{MintedVerbatim}\n",
    );
    let root = parent.canonicalize().unwrap();
    let name = |relative: &str| root.join(relative).display().to_string();
    let (cached, sp) = (name("_minted/C.highlight.minted"), |bp: f64| (bp * 65781.76) as i64);
    // The third block floats to the top of the page, above the others.
    let synctex = format!(
        "SyncTeX Version:1\nInput:1:{}\nInput:2:{cached}\nInput:3:{}\nInput:4:{cached}\n\
         Input:5:{cached}\nInput:6:{cached}\nInput:7:{}\nOutput:pdf\nMagnification:1000\nUnit:1\n\
         X Offset:0\nY Offset:0\nContent:\n{{1\n[1,22:0,0:{},{},0\n{}{}{}{}{}{}{}]\n}}1\n",
        name("main.tex"),
        name("chapter.tex"),
        name("_minted/D.highlight.minted"),
        sp(612.0),
        sp(792.0),
        record(5, 3, 100.0),
        record(1, 4, 150.0),
        record(2, 3, 200.0),
        record(4, 3, 250.0),
        record(6, 3, 300.0),
        record(1, 22, 350.0),
        record(7, 2, 400.0),
    );
    (parent, root, synctex)
}

#[test]
fn a_click_lands_on_the_block_tex_read_at_that_point() {
    let (_parent, root, synctex) = project();
    let inputs = Inputs::new(Some(&synctex), &root.join("main.pdf"));
    let at = |input: &str, line: u32, v: f64| {
        let (path, line) =
            source(&synctex, &inputs, &root, &root.join(input), line, 1, (100.0, v - 5.0))?;
        Some((path.strip_prefix(&root).unwrap().display().to_string(), line))
    };
    let cached = "_minted/C.highlight.minted";
    // TeX read main's first block, the chapter's, then main's other two: not
    // the order the sources list them in (main's three, then the chapter's),
    // nor where they are on the page.
    assert_eq!(at(cached, 3, 200.0), Some(("main.tex".into(), 11)));
    assert_eq!(at(cached, 3, 250.0), Some(("chapter.tex".into(), 3)));
    assert_eq!(at(cached, 3, 100.0), Some(("main.tex".into(), 16)));
    assert_eq!(at(cached, 3, 300.0), Some(("main.tex".into(), 20)));
    // Code no block holds lands on the nearest line TeX recorded for the project.
    assert_eq!(at("_minted/D.highlight.minted", 2, 400.0), Some(("main.tex".into(), 22)));
    // Any other input is not this lookup's.
    assert_eq!(at("main.tex", 4, 150.0), None);
}
