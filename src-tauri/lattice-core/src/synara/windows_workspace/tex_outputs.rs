use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::latex::{aux_files, AuxFiles};

use super::files::{
    destination_snapshot, selected_directory, selected_file, stage_file, Selection,
};
use super::native::Root;
use super::snapshot::{self, Contents, Snapshot, Step};
use super::{invalid, permitted_name, transaction, RECOVERY_PREFIX};

const MAX_PDF: u64 = 128 * 1024 * 1024;
const MAX_SYNCTEX: u64 = 64 * 1024 * 1024;
const MAX_AUXILIARY: u64 = 4 * 1024 * 1024;
const AUXILIARIES: &[&str] =
    &["bcf", "blg", "fls", "idx", "ilg", "ind", "lof", "log", "lot", "out", "toc"];
const MAX_AUX_FILES: usize = 1024;

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

    let (pdf, map, gz) = read_preview(&output, leaf)?;
    validate_pair(&pdf, &map, gz, &root.path.join(document))?;
    let staged = Root::create_staging(
        &output.path.join(format!("publish-{}", uuid::Uuid::new_v4().simple())),
        &root,
    )?;
    let relative = Path::new("");
    let sync = aux_files::synctex_in(relative, document, layout, gz);
    stage_file(&staged, &Path::new(document).with_extension("pdf"), &pdf)?;
    stage_file(&staged, &sync, &map)?;
    stage_auxiliaries(&staged, &output, &aux_files::folder(relative, document, layout), leaf)?;
    let source = Snapshot::read(&staged)?;
    let mut obsolete = obsolete_auxiliaries(&root, document, leaf, aux_files::other(layout))?;
    for layout in [AuxFiles::Beside, AuxFiles::Hidden] {
        obsolete.extend(
            aux_files::SYNCTEX
                .iter()
                .map(|ext| aux_files::in_layout(relative, document, ext, layout))
                .filter(|path| path != &sync),
        );
    }
    apply_publication(&source, &root, document, &obsolete)
}

fn apply_publication(
    source: &Snapshot<'_>, root: &Root, document: &str, obsolete: &BTreeSet<PathBuf>,
) -> io::Result<Vec<String>> {
    let touched = std::iter::once(Path::new(document))
        .chain(source.entries.iter().filter_map(|(path, record)| {
            matches!(record.contents, Contents::File(_)).then_some(path.as_path())
        }))
        .chain(obsolete.iter().map(PathBuf::as_path));
    let original = destination_snapshot(root, touched, Selection::BuildArtifacts)?;
    if !original.entries.contains_key(Path::new(document)) {
        return Err(invalid("Missing TeX project document", Path::new(document)));
    }
    let baseline = original
        .entries
        .iter()
        .filter(|(path, _)| source.entries.contains_key(*path))
        .map(|(path, record)| (path.clone(), *record))
        .collect();
    let mut steps = snapshot::plan(&baseline, source, &original, false)?;
    steps.extend(removal_steps(&original, obsolete));
    Ok(transaction::run(source, &original, steps, &mut false)?
        .into_iter()
        .filter(|(_, record)| {
            record.is_none_or(|record| matches!(record.contents, Contents::File(_)))
        })
        .map(|(path, _)| path.to_string_lossy().replace('\\', "/"))
        .collect())
}

fn document_leaf(document: &str) -> io::Result<&Path> {
    let path = Path::new(document);
    if !valid_artifact_path(path, "tex") {
        return Err(invalid("Invalid TeX project document", path));
    }
    path.file_name().map(Path::new).ok_or_else(|| invalid("Missing TeX document name", path))
}

fn valid_artifact_path(path: &Path, extension: &str) -> bool {
    !snapshot::protected_path(path)
        && path.components().all(|component| {
            matches!(component, Component::Normal(name) if permitted_name(name)
                && !name.to_string_lossy().to_ascii_lowercase().starts_with(RECOVERY_PREFIX))
        })
        && path.extension().is_some_and(|name| name.eq_ignore_ascii_case(extension))
}

fn read_artifact(root: &Root, name: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    open_artifact(root, name)?.map(|file| bounded_bytes(file, limit, name)).transpose()
}

fn open_artifact(root: &Root, path: &Path) -> io::Result<Option<File>> {
    let (parent, name) = snapshot::split(path)?;
    let mut directory = Arc::clone(&root.directory);
    let mut ancestors = Vec::new();
    for part in parent {
        let Some((child, _)) = selected_directory(&directory, part)? else { return Ok(None) };
        ancestors.push(directory);
        directory = Arc::new(child);
    }
    selected_file(&directory, name, false)
}

fn read_preview(output: &Root, leaf: &Path) -> io::Result<(Vec<u8>, Vec<u8>, bool)> {
    let pdf = read_artifact(output, &leaf.with_extension("pdf"), MAX_PDF)?
        .ok_or_else(|| invalid("Missing TeX PDF", &output.path))?;
    let [compressed, plain] = aux_files::SYNCTEX;
    let gzip = read_artifact(output, &leaf.with_extension(compressed), MAX_SYNCTEX)?;
    let text = read_artifact(output, &leaf.with_extension(plain), MAX_SYNCTEX)?;
    match (gzip, text) {
        (Some(bytes), None) => Ok((pdf, bytes, true)),
        (None, Some(bytes)) => Ok((pdf, bytes, false)),
        _ => Err(invalid("Expected exactly one TeX SyncTeX map", &output.path)),
    }
}

fn stage_auxiliaries(staged: &Root, output: &Root, folder: &Path, leaf: &Path) -> io::Result<()> {
    for extension in AUXILIARIES.iter().copied().chain(["bbl"]) {
        let name = leaf.with_extension(extension);
        if let Some(bytes) = read_artifact(output, &name, MAX_AUXILIARY)? {
            stage_file(staged, &folder.join(name), &bytes)?;
        }
    }
    visit_included_auxiliaries(leaf.with_extension("aux"), |name| {
        let bytes = read_artifact(output, name, MAX_AUXILIARY)?;
        if let Some(bytes) = &bytes {
            stage_file(staged, &folder.join(name), bytes)?;
        }
        Ok(bytes)
    })
}

fn visit_included_auxiliaries(
    main: PathBuf, mut visit: impl FnMut(&Path) -> io::Result<Option<Vec<u8>>>,
) -> io::Result<()> {
    let mut visited = BTreeSet::from([main.to_string_lossy().to_ascii_lowercase()]);
    let mut pending = vec![main];
    while let Some(path) = pending.pop() {
        let Some(bytes) = visit(&path)? else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for name in text.lines().filter_map(included_auxiliary) {
            if visited.insert(name.to_string_lossy().to_ascii_lowercase()) {
                if visited.len() > MAX_AUX_FILES {
                    return Err(invalid("Too many TeX intermediate files", &path));
                }
                pending.push(name);
            }
        }
    }
    Ok(())
}

fn included_auxiliary(line: &str) -> Option<PathBuf> {
    let name = line.trim().strip_prefix("\\@input{")?.strip_suffix('}')?;
    let path: PathBuf =
        Path::new(name).components().filter(|part| *part != Component::CurDir).collect();
    valid_artifact_path(&path, "aux").then_some(path)
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

pub(in crate::synara) fn clean(root: &Path, document: &str) -> Result<(), String> {
    clean_inner(root, document).map_err(|error| error.to_string())
}

fn clean_inner(root: &Path, document: &str) -> io::Result<()> {
    let leaf = document_leaf(document)?;
    let root = Root::open(root)?;
    transaction::check_recovery(&root)?;
    let mut paths = clean_paths(&root, document, leaf, AuxFiles::Beside)?;
    paths.extend(clean_paths(&root, document, leaf, AuxFiles::Hidden)?);
    let original =
        destination_snapshot(&root, paths.iter().map(PathBuf::as_path), Selection::BuildArtifacts)?;
    let steps = removal_steps(&original, &paths);
    let scratch = tempfile::tempdir()?;
    let empty = Root::open(scratch.path())?;
    transaction::run(&Snapshot::empty(&empty), &original, steps, &mut false)?;
    Ok(())
}

fn clean_paths(
    root: &Root, document: &str, leaf: &Path, layout: AuxFiles,
) -> io::Result<BTreeSet<PathBuf>> {
    let folder = aux_files::folder(Path::new(""), document, layout);
    let mut paths: BTreeSet<_> = AUXILIARIES
        .iter()
        .copied()
        .chain(["fdb_latexmk", "xdv"])
        .map(|extension| folder.join(leaf.with_extension(extension)))
        .collect();
    visit_included_auxiliaries(leaf.with_extension("aux"), |name| {
        let path = folder.join(name);
        let bytes = read_artifact(root, &path, MAX_AUXILIARY)?;
        if bytes.is_some() {
            paths.insert(path);
        }
        Ok(bytes)
    })?;
    Ok(paths)
}

fn obsolete_auxiliaries(
    root: &Root, document: &str, leaf: &Path, layout: AuxFiles,
) -> io::Result<BTreeSet<PathBuf>> {
    let mut paths = clean_paths(root, document, leaf, layout)?;
    let bibliography = aux_files::in_layout(Path::new(""), document, "bbl", layout);
    if layout == AuxFiles::Hidden
        || open_artifact(root, &bibliography.with_extension("blg"))?.is_some()
    {
        paths.insert(bibliography);
    }
    Ok(paths)
}

fn removal_steps(original: &Snapshot<'_>, paths: &BTreeSet<PathBuf>) -> Vec<Step> {
    paths
        .iter()
        .filter_map(|path| {
            let before = original.entries.get(path).copied()?;
            Some(Step { path: path.clone(), before: Some(before), after: None })
        })
        .collect()
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
        fs::write(root.join("main.bbl"), "supplied bibliography").unwrap();
        fs::write(root.join("main.log"), "previous layout log").unwrap();
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
        fs::create_dir(output.path().join("chapters")).unwrap();
        fs::write(output.path().join("chapters/intro.aux"), "\\newlabel{example}{{1}{1}}").unwrap();
        fs::write(output.path().join("main.aux"), "\\@input{chapters/intro.aux}\n").unwrap();
        fs::write(output.path().join("main.log"), "build diagnostics").unwrap();
        let bibliography =
            "\\begin{thebibliography}{1}\n\\bibitem{sample} Sample.\n\\end{thebibliography}";
        fs::write(output.path().join("main.bbl"), bibliography).unwrap();
        fs::write(output.path().join("main.blg"), "bibliography diagnostics").unwrap();
        fs::write(output.path().join("extra.tex"), "unselected").unwrap();
        publish(root, output.path(), "main.tex", AuxFiles::Hidden).unwrap();
        assert_eq!(pdf, fs::read(root.join("main.pdf")).unwrap());
        assert_eq!(MAP, fs::read_to_string(root.join(".research/build/main.synctex")).unwrap());
        assert!(root.join(".research/build/main.aux").is_file());
        assert!(root.join(".research/build/main.log").is_file());
        assert_eq!(
            bibliography,
            fs::read_to_string(root.join(".research/build/main.bbl")).unwrap()
        );
        assert!(root.join(".research/build/main.blg").is_file());
        assert!(root.join(".research/build/chapters/intro.aux").is_file());
        assert!(!root.join("main.synctex.gz").exists());
        assert!(!root.join("main.log").exists());
        assert!(!root.join("extra.tex").exists());
        assert_eq!("supplied bibliography", fs::read_to_string(root.join("main.bbl")).unwrap());

        fs::remove_file(output.path().join("main.bbl")).unwrap();
        fs::remove_file(output.path().join("main.blg")).unwrap();
        publish(root, output.path(), "main.tex", AuxFiles::Beside).unwrap();
        assert!(root.join("chapters/intro.aux").is_file());
        assert_eq!(MAP, fs::read_to_string(root.join("main.synctex")).unwrap());
        for name in ["main.aux", "main.bbl", "main.blg", "main.log", "chapters/intro.aux"] {
            assert!(!root.join(".research/build").join(name).exists());
        }
    }

    #[test]
    fn clean_removes_build_intermediates_in_both_layouts() {
        let original = tempfile::tempdir().unwrap();
        let root = original.path();
        let beside = root.join("chapter");
        let hidden = root.join(".research/build/chapter");
        let intermediates = ["main.log", "main.toc", "main.blg", "main.fdb_latexmk"];
        let retained = ["main.synctex.gz", "main.pdf", "main.bbl", "other.aux", ".chem/cache.aux"];
        for folder in [&beside, &hidden] {
            fs::create_dir_all(folder.join("parts")).unwrap();
            fs::create_dir(folder.join(".chem")).unwrap();
            for name in intermediates.into_iter().chain(retained).chain(["parts/intro.aux"]) {
                fs::write(folder.join(name), name).unwrap();
            }
            fs::write(
                folder.join("main.aux"),
                "\\@input{parts/intro.aux}\n\\@input{.chem/cache.aux}\n",
            )
            .unwrap();
        }
        let journal = root.join(".lattice-mxc-test.journal");
        fs::write(&journal, "unfinished").unwrap();
        assert!(clean(root, "chapter/main.tex").unwrap_err().contains("Unfinished"));
        assert!(hidden.join("main.aux").exists());
        fs::remove_file(journal).unwrap();
        fs::hard_link(hidden.join("other.aux"), hidden.join("main.fls")).unwrap();
        assert!(clean(root, "chapter/main.tex").is_err());
        assert!(hidden.join("main.aux").exists());
        assert!(beside.join("main.aux").exists());
        fs::remove_file(hidden.join("main.fls")).unwrap();
        clean(root, "chapter/main.tex").unwrap();
        for folder in [&beside, &hidden] {
            for name in intermediates.into_iter().chain(["main.aux", "parts/intro.aux"]) {
                assert!(!folder.join(name).exists());
            }
            for name in retained {
                assert!(folder.join(name).exists());
            }
        }
        assert!(clean(root, ".research/main.tex").is_err());
    }
}
