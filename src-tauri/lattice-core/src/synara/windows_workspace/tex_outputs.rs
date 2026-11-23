use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::latex::{aux_files, AuxFiles};

use super::native::{self, Root};
use super::snapshot::{self, Contents, Record, Snapshot, Step};
use super::{invalid, permitted_name, transaction, RECOVERY_PREFIX};

const MAX_PDF: u64 = 128 * 1024 * 1024;
const MAX_SYNCTEX: u64 = 64 * 1024 * 1024;
const MAX_AUXILIARY: u64 = 4 * 1024 * 1024;
const ROOT_AUXILIARIES: &[&str] = &["aux", "log", "fls", "fdb_latexmk", "bbl", "blg"];

pub(in crate::synara) fn publish(
    root: &Path, output: &Path, document: &str, layout: AuxFiles,
) -> Result<Vec<String>, String> {
    publish_inner(root, output, document, layout).map_err(|error| error.to_string())
}

fn publish_inner(
    root: &Path, output: &Path, document: &str, layout: AuxFiles,
) -> io::Result<Vec<String>> {
    let leaf = document_leaf(document)?;
    let root = Root::open(root)?;
    let output = Root::open(output)?;
    if root.overlaps(&output) {
        return Err(invalid("TeX output overlaps the project", &output.path));
    }

    let pdf = read_artifact(&output, &leaf.with_extension("pdf"), MAX_PDF)?
        .ok_or_else(|| invalid("Missing TeX PDF", &output.path))?;
    let [compressed, plain] = aux_files::SYNCTEX;
    let gzip = read_artifact(&output, &leaf.with_extension(compressed), MAX_SYNCTEX)?;
    let text = read_artifact(&output, &leaf.with_extension(plain), MAX_SYNCTEX)?;
    let (map, gz) = match (gzip, text) {
        (Some(bytes), None) => (bytes, true),
        (None, Some(bytes)) => (bytes, false),
        _ => return Err(invalid("Expected exactly one TeX SyncTeX map", &output.path)),
    };
    validate_pair(&pdf, &map, gz, &root.path.join(document))?;
    let staged = Root::create_staging(
        &output.path.join(format!("publish-{}", uuid::Uuid::new_v4().simple())),
        &root,
    )?;
    let relative = Path::new("");
    let sync = aux_files::synctex_in(relative, document, layout, gz);
    stage_file(&staged, &Path::new(document).with_extension("pdf"), &pdf)?;
    stage_file(&staged, &sync, &map)?;
    for extension in ["log", "aux"] {
        if let Some(bytes) = read_artifact(&output, &leaf.with_extension(extension), MAX_AUXILIARY)?
        {
            stage_file(
                &staged,
                &aux_files::in_layout(relative, document, extension, layout),
                &bytes,
            )?;
        }
    }
    let source = Snapshot::read(&staged)?;
    let obsolete: Vec<_> = [AuxFiles::Beside, AuxFiles::Hidden]
        .into_iter()
        .flat_map(|layout| {
            aux_files::SYNCTEX
                .map(|extension| aux_files::in_layout(relative, document, extension, layout))
        })
        .filter(|path| path != &sync)
        .collect();
    let touched = std::iter::once(Path::new(document))
        .chain(source.entries.iter().filter_map(|(path, record)| {
            matches!(record.contents, Contents::File(_)).then_some(path.as_path())
        }))
        .chain(obsolete.iter().map(PathBuf::as_path));
    let original = destination_snapshot(&root, touched)?;
    if !original.entries.contains_key(Path::new(document)) {
        return Err(invalid("Missing TeX project document", Path::new(document)));
    }
    let baseline = original
        .entries
        .iter()
        .filter(|(path, _)| source.entries.contains_key(*path))
        .map(|(path, record)| (path.clone(), *record))
        .collect();
    let mut steps = snapshot::plan(&baseline, &source, &original, false)?;
    steps.extend(obsolete.into_iter().filter_map(|path| {
        let before = original.entries.get(&path).copied()?;
        Some(Step { path, before: Some(before), after: None })
    }));
    Ok(transaction::run(&source, &original, steps, &mut false)?
        .into_iter()
        .filter(|(_, record)| {
            record.is_some_and(|record| matches!(record.contents, Contents::File(_)))
        })
        .map(|(path, _)| path.to_string_lossy().replace('\\', "/"))
        .collect())
}

fn document_leaf(document: &str) -> io::Result<&Path> {
    let path = Path::new(document);
    if snapshot::protected_path(path)
        || !path.components().all(|component| {
            matches!(component, Component::Normal(name) if permitted_name(name)
                && !name.to_string_lossy().to_ascii_lowercase().starts_with(RECOVERY_PREFIX))
        })
        || !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
    {
        return Err(invalid("Invalid TeX project document", path));
    }
    path.file_name().map(Path::new).ok_or_else(|| invalid("Missing TeX document name", path))
}

fn selected_file(parent: &File, name: &OsStr, movable: bool) -> io::Result<Option<File>> {
    let Some(entry) = selected_child(parent, name)? else {
        return Ok(None);
    };
    let file = native::read_file(parent, OsStr::new(&entry.name), movable)?;
    native::verify_file(&file, movable)?;
    if native::identity(&file)?.index != entry.index {
        return Err(invalid("TeX artifact changed during inspection", Path::new(name)));
    }
    Ok(Some(file))
}

fn selected_child(parent: &File, name: &OsStr) -> io::Result<Option<native::Child>> {
    let children = native::children(parent)?;
    if let Some(entry) =
        children.iter().find(|entry| entry.name.to_ascii_lowercase().starts_with(RECOVERY_PREFIX))
    {
        return Err(invalid("Unfinished workspace transaction", Path::new(&entry.name)));
    }
    Ok(children.into_iter().find(|entry| name.eq_ignore_ascii_case(OsStr::new(&entry.name))))
}

fn selected_directory(parent: &File, name: &OsStr) -> io::Result<Option<(File, native::Identity)>> {
    let Some(entry) = selected_child(parent, name)? else { return Ok(None) };
    let file = native::open_directory(parent, OsStr::new(&entry.name), false)?;
    let identity = native::identity(&file)?;
    if identity.index != entry.index {
        return Err(invalid("TeX artifact directory changed during inspection", Path::new(name)));
    }
    Ok(Some((file, identity)))
}

fn read_artifact(root: &Root, name: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    selected_file(&root.directory, name.as_os_str(), false)?
        .map(|file| bounded_bytes(file, limit, name))
        .transpose()
}

fn bounded_bytes(input: impl Read, limit: u64, path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("TeX artifact exceeds its size limit", path));
    }
    Ok(bytes)
}

fn validate_pair(pdf: &[u8], sync: &[u8], gz: bool, document: &Path) -> io::Result<()> {
    if !pdf.starts_with(b"%PDF-") || !pdf.trim_ascii_end().ends_with(b"%%EOF") {
        return Err(invalid("Malformed TeX PDF", document));
    }
    let parsed =
        lopdf::Document::load_mem(pdf).map_err(|_| invalid("Malformed TeX PDF", document))?;
    let pages = parsed.get_pages().len();
    if pages == 0 || parsed.is_encrypted() {
        return Err(invalid("Malformed TeX PDF", document));
    }

    let decoded;
    let sync = if gz {
        decoded = bounded_bytes(flate2::read::MultiGzDecoder::new(sync), MAX_SYNCTEX, document)?;
        decoded.as_slice()
    } else {
        sync
    };
    let text = std::str::from_utf8(sync).map_err(|_| invalid("Malformed SyncTeX", document))?;
    if !matching_map(text, document, pages) {
        return Err(invalid("Malformed or mismatched SyncTeX", document));
    }
    Ok(())
}

fn matching_map(text: &str, document: &Path, pages: usize) -> bool {
    let mut sources = text
        .lines()
        .filter_map(|line| line.strip_prefix("Input:")?.split_once(':'))
        .filter(|(tag, _)| tag.parse::<u32>().ok() == Some(1));
    let Some((_, input)) = sources.next() else { return false };
    if sources.next().is_some() {
        return false;
    }
    let input = Path::new(input)
        .components()
        .filter(|part| *part != Component::CurDir)
        .collect::<PathBuf>();
    let expected = crate::host_paths::tex_path(document);
    if !crate::host_paths::tex_path(&input).eq_ignore_ascii_case(&expected)
        && !(input.components().count() == 1 && input.file_name() == document.file_name())
    {
        return false;
    }

    let mut lines = text.lines();
    if lines.next() != Some("SyncTeX Version:1") {
        return false;
    }
    let outputs: Vec<_> = lines
        .by_ref()
        .take_while(|line| *line != "Content:")
        .filter_map(|line| line.strip_prefix("Output:"))
        .collect();
    matches!(outputs.as_slice(), ["pdf" | "xdv" | "dvi"]) && matching_pages(lines, pages)
}

fn matching_pages(mut lines: std::str::Lines<'_>, pages: usize) -> bool {
    let mut closed = 0;
    let mut open = false;
    for line in lines.by_ref().take_while(|line| *line != "Postamble:") {
        if let Some(page) = line.strip_prefix('{') {
            if open || page.parse::<usize>().ok() != Some(closed + 1) {
                return false;
            }
            open = true;
        } else if let Some(page) = line.strip_prefix('}') {
            if !open || page.parse::<usize>().ok() != Some(closed + 1) {
                return false;
            }
            closed += 1;
            open = false;
        }
    }
    !open
        && closed == pages
        && lines
            .next()
            .and_then(|line| line.strip_prefix("Count:"))
            .and_then(|count| count.parse::<usize>().ok())
            .is_some_and(|count| count > 0)
        && lines.any(|line| line == "Post scriptum:")
}

fn stage_file(root: &Root, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (parent, name) = snapshot::split(path)?;
    let mut directory = Arc::clone(&root.directory);
    for part in parent {
        let child = match native::open_directory(&directory, part, false) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                native::open_directory(&directory, part, true)?
            }
            result => result?,
        };
        directory = Arc::new(child);
    }
    native::new_file(&directory, name, &[])?.write_all(bytes)
}

fn destination_snapshot<'root, 'paths>(
    root: &'root Root, paths: impl IntoIterator<Item = &'paths Path>,
) -> io::Result<Snapshot<'root>> {
    let mut snapshot = Snapshot {
        root,
        entries: BTreeMap::new(),
        dirs: BTreeMap::from([(PathBuf::new(), Arc::clone(&root.directory))]),
    };
    'file: for path in paths {
        let (parent, name) = snapshot::split(path)?;
        let mut relative = PathBuf::new();
        let mut directory = Arc::clone(&root.directory);
        for part in parent {
            relative.push(part);
            if let Some(open) = snapshot.dirs.get(&relative) {
                directory = Arc::clone(open);
                continue;
            }
            let Some((file, identity)) = selected_directory(&directory, part)? else {
                continue 'file;
            };
            directory = Arc::new(file);
            snapshot.dirs.insert(relative.clone(), Arc::clone(&directory));
            snapshot
                .entries
                .insert(relative.clone(), Record { identity, contents: Contents::Directory });
        }
        if let Some(mut file) = selected_file(&directory, name, false)? {
            let identity = native::identity(&file)?;
            let contents = snapshot::transfer(&mut file, &mut io::sink())?;
            native::verify_file(&file, false)?;
            snapshot.entries.insert(path.to_owned(), Record { identity, contents });
        }
    }
    Ok(snapshot)
}

pub(in crate::synara) fn clean(root: &Path, document: &str) -> Result<(), String> {
    clean_inner(root, document).map_err(|error| error.to_string())
}

fn clean_inner(root: &Path, document: &str) -> io::Result<()> {
    let leaf = document_leaf(document)?;
    let root = Root::open(root)?;
    transaction::check_recovery(&root)?;
    let mirror = aux_files::folder(Path::new(""), document, AuxFiles::Hidden);
    let mut directory = Arc::clone(&root.directory);
    let mut ancestors = Vec::new();
    for part in &mirror {
        let Some((child, _)) = selected_directory(&directory, part)? else { return Ok(()) };
        ancestors.push(directory);
        directory = Arc::new(child);
    }
    let mut files = Vec::new();
    for extension in ROOT_AUXILIARIES {
        if let Some(file) =
            selected_file(&directory, leaf.with_extension(extension).as_os_str(), true)?
        {
            files.push(file);
        }
    }
    for file in files {
        native::verify_file(&file, true)?;
        native::unlink(&file)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document};
    use std::fs;

    const MAP: &str = "SyncTeX Version:1\nInput:1:./main.tex\nOutput:pdf\nContent:\n{1\n}1\nPostamble:\nCount:1\nPost scriptum:\n";

    fn pdf() -> Vec<u8> {
        let mut pdf = Document::with_version("1.4");
        let pages = pdf.new_object_id();
        let page = pdf.add_object(dictionary! { "Type" => "Page", "Parent" => pages });
        pdf.objects.insert(
            pages,
            dictionary! {
                "Type" => "Pages", "Kids" => vec![lopdf::Object::Reference(page)], "Count" => 1,
            }
            .into(),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        pdf.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn publishes_a_complete_pair_and_retires_the_other_map() {
        let original = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let root = original.path();
        fs::write(root.join("main.tex"), "source").unwrap();
        fs::write(root.join("main.pdf"), "previous PDF").unwrap();
        fs::write(root.join("main.synctex.gz"), "previous map").unwrap();
        fs::create_dir(root.join(".chem")).unwrap();
        fs::write(root.join(".chem/current.sqlite"), "unrelated").unwrap();
        fs::hard_link(root.join(".chem/current.sqlite"), root.join(".chem/linked.sqlite")).unwrap();
        let pdf = pdf();
        fs::write(output.path().join("main.pdf"), &pdf).unwrap();
        assert!(publish(root, output.path(), "main.tex", AuxFiles::Hidden).is_err());
        fs::write(output.path().join("main.synctex"), "malformed").unwrap();
        assert!(publish(root, output.path(), "main.tex", AuxFiles::Hidden).is_err());
        fs::write(output.path().join("main.synctex"), MAP).unwrap();
        fs::write(output.path().join("main.pdf"), "%PDF-1.4\n%%EOF").unwrap();
        assert!(publish(root, output.path(), "main.tex", AuxFiles::Hidden).is_err());
        assert_eq!(b"previous PDF", fs::read(root.join("main.pdf")).unwrap().as_slice());
        fs::write(output.path().join("main.pdf"), &pdf).unwrap();
        fs::write(output.path().join("main.aux"), "\\newlabel{example}{{1}{1}}").unwrap();
        fs::write(output.path().join("main.log"), "build diagnostics").unwrap();
        fs::write(output.path().join("extra.tex"), "unselected").unwrap();
        publish(root, output.path(), "main.tex", AuxFiles::Hidden).unwrap();
        assert_eq!(pdf, fs::read(root.join("main.pdf")).unwrap());
        assert_eq!(MAP, fs::read_to_string(root.join(".research/build/main.synctex")).unwrap());
        assert!(root.join(".research/build/main.aux").is_file());
        assert!(root.join(".research/build/main.log").is_file());
        assert!(!root.join("main.synctex.gz").exists());
        assert!(!root.join("extra.tex").exists());
    }

    #[test]
    fn clean_only_unlinks_selected_hidden_regular_files() {
        let original = tempfile::tempdir().unwrap();
        let root = original.path();
        let mirror = root.join(".research/build/chapter");
        fs::create_dir_all(&mirror).unwrap();
        for name in ["main.aux", "main.log", "main.synctex.gz", "main.pdf", "other.aux"] {
            fs::write(mirror.join(name), name).unwrap();
        }
        let journal = root.join(".lattice-mxc-test.journal");
        fs::write(&journal, "unfinished").unwrap();
        assert!(clean(root, "chapter/main.tex").unwrap_err().contains("Unfinished"));
        assert!(mirror.join("main.aux").exists());
        fs::remove_file(journal).unwrap();
        fs::hard_link(mirror.join("other.aux"), mirror.join("main.fls")).unwrap();
        assert!(clean(root, "chapter/main.tex").is_err());
        assert!(mirror.join("main.aux").exists());
        fs::remove_file(mirror.join("main.fls")).unwrap();
        clean(root, "chapter/main.tex").unwrap();
        assert!(!mirror.join("main.aux").exists());
        assert!(!mirror.join("main.log").exists());
        for name in ["main.synctex.gz", "main.pdf", "other.aux"] {
            assert!(mirror.join(name).exists());
        }
        assert!(clean(root, ".research/main.tex").is_err());
    }
}
