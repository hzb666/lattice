//! Persistent ChemDraw assets for both the editor and LaTeX.

use crate::util::{err, sha256_hex};
use crate::{models::FileNode, project};
use chemex_cdxml::{DrawingDocument, FieldValues, MAX_SOURCE_BYTES};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use svg2pdf::usvg;

mod compound_groups;
mod group_preview;
mod molecules;
mod prepared;
#[cfg(windows)]
mod preview_worker;

pub use compound_groups::compound_groups;
pub use group_preview::preview_group_svg;
pub use molecules::{
    MoleculeAnalysisError, MoleculeLocation, ProjectMolecule, ProjectMoleculeReport,
    analyze_project_molecules, compound_formula, compound_formula_display, display_formula,
};
pub use prepared::built_preview_svg;
pub(crate) use prepared::{Asset, pdf_key, ready_path, spectrum_key};

const CACHE: &str = ".chem/cache/chemdraw";
const SPECTRUM_CACHE: &str = ".chem/cache/mestrenova";
static EXPORT: Mutex<()> = Mutex::new(());

pub fn is_cdxml(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cdxml"))
}

pub fn cdxml_files(tree: &[FileNode]) -> impl Iterator<Item = &str> {
    project::tree_files(tree)
        .into_iter()
        .map(|file| file.path.as_str())
        .filter(|path| is_cdxml(Path::new(path)))
}

pub fn is_mnova(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("mnova"))
}

pub fn mnova_files(tree: &[FileNode]) -> impl Iterator<Item = &str> {
    project::tree_files(tree)
        .into_iter()
        .map(|file| file.path.as_str())
        .filter(|path| is_mnova(Path::new(path)))
}

pub fn read_document(path: &Path) -> Result<DrawingDocument, String> {
    let bytes = read_asset(path)?;
    let source = String::from_utf8(bytes).map_err(err)?;
    DrawingDocument::parse(source).map_err(err)
}

pub(crate) fn read_asset(path: &Path) -> Result<Vec<u8>, String> {
    if !path.symlink_metadata().map_err(err)?.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(err)?
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(format!("{} exceeds the Chemex file size limit", path.display()));
    }
    Ok(bytes)
}

pub(crate) fn cache_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    project::creation_path(root, &format!("{CACHE}/{relative}"))
}

pub(crate) fn spectrum_cache_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    project::creation_path(root, &format!("{SPECTRUM_CACHE}/{relative}"))
}

pub(crate) fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} is not a regular file", path.display()));
        }
        Ok(metadata) if metadata.len() == bytes.len() as u64 => {
            if fs::read(path).map_err(err)? == bytes {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(err(error)),
    }
    let parent = path.parent().ok_or_else(|| format!("No parent folder for {}", path.display()))?;
    let mut pending = tempfile::NamedTempFile::new_in(parent).map_err(err)?;
    pending.write_all(bytes).map_err(err)?;
    pending.persist(path).map_err(err)?;
    Ok(())
}

pub fn preview_svg(root: &Path, source: &str) -> Result<Vec<u8>, String> {
    svg_with(root, source, |directory| {
        chemex_chemdraw::export_svg(directory, std::time::Duration::from_secs(120)).map_err(err)
    })
    .map(|(_, bytes)| bytes)
}

/// Export a project CDXML drawing as its cached SVG.
pub fn drawing_preview_svg(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, String> {
    if !is_cdxml(Path::new(relative)) {
        return Ok(None);
    }
    let path = project::safe_path(root, relative)?;
    let document = read_document(&path)?;
    preview_svg(root, document.source()).map(Some)
}

pub fn mnova_preview_pdf(
    root: &Path, relative: &str, spectrum: NonZeroUsize,
) -> Result<PathBuf, String> {
    let source = project::safe_path(root, relative)?;
    export_spectrum_pdf(root, &source, spectrum, |mut command, directory| {
        command.current_dir(directory);
        let output = crate::commands::output_with_timeout(&mut command, Duration::from_secs(120))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "MestReNova failed ({}): {}",
                output.status,
                crate::commands::combined_output(&output).trim()
            ))
        }
    })
}

pub(crate) fn export_spectrum_pdf(
    root: &Path, source: &Path, spectrum: NonZeroUsize,
    run: impl FnOnce(Command, &Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let asset = spectrum_cache_file(root, ".keep")?;
    let cache = asset.parent().ok_or("Missing MestReNova cache directory")?;
    chemex_cli::export_spectrum_pdf(cache, source, spectrum, run)
}

pub(crate) fn cached_spectrum_pdf(folder: &Path, name: &str) -> bool {
    if spectrum_key(name).is_none() {
        return false;
    }
    let path = folder.join(name);
    path.symlink_metadata().is_ok_and(|metadata| metadata.file_type().is_file())
        && chemex_cli::is_valid_spectrum_pdf(&path).unwrap_or(false)
}

/// Export the unique ChemDraw group named by a `\chemex{key}` reference.
pub fn compound_preview_svg(root: &Path, compound_id: &str) -> Result<Option<Vec<u8>>, String> {
    if compound_id.is_empty() || compound_id.chars().count() > 200 {
        return Ok(None);
    }
    let tree = project::scan_tree(root, project::TreeView::Project)?;
    let mut matching = None;
    for relative in cdxml_files(&tree) {
        let Ok(path) = project::safe_path(root, relative) else { continue };
        let Ok(document) = read_document(&path) else { continue };
        if !document.fields().iter().any(|field| field.compound_id == compound_id) {
            continue;
        }
        if matching.is_some() {
            return Err(format!("ChemDraw key {compound_id} occurs in more than one drawing"));
        }
        matching = Some(document);
    }
    matching.map(|document| preview_group_svg(root, &document, compound_id)).transpose()
}

/// Read the compound labels produced by the latest Chemex numbering pass.
pub fn built_compound_numbers(root: &Path, paper: &str) -> Option<BTreeMap<String, String>> {
    let namespace = sha256_hex(paper);
    let path = cache_file(root, &format!("{namespace}/number/chemex-numbers.dat")).ok()?;
    chemex_cli::read_numbers(&path).ok()
}

/// Render the latest LaTeX compound numbers into a new CDXML source string.
pub fn render_compound_numbers(
    document: &DrawingDocument, numbers: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut values = FieldValues::new();
    for field in document.fields() {
        if values
            .insert(
                field.compound_id.clone(),
                numbers
                    .get(&field.compound_id)
                    .ok_or_else(|| format!("No rendered number for {}", field.compound_id))?
                    .clone(),
            )
            .is_err()
        {
            continue;
        }
    }
    document.render_fields(&values).map_err(err)
}

pub(crate) fn svg_with(
    root: &Path, source: &str, run: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(PathBuf, Vec<u8>), String> {
    let path = svg_cache_path(root, &sha256_hex(source))?;
    let bytes = cached_svg(&path, || {
        let scratch = tempfile::tempdir().map_err(err)?;
        fs::write(scratch.path().join("drawing.cdxml"), source).map_err(err)?;
        run(scratch.path())?;
        read_asset(&scratch.path().join("drawing.svg"))
    })?;
    Ok((path, bytes))
}

fn svg_cache_path(root: &Path, key: &str) -> Result<PathBuf, String> {
    cache_file(root, &format!("assets/{key}.svg"))
}

pub(crate) fn read_svg(path: &Path) -> Option<Vec<u8>> {
    let bytes = read_asset(path).ok()?;
    project::check_embedded_svg(&bytes).ok()?;
    Some(bytes)
}

fn cached_svg(
    path: &Path, generate: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    if let Some(bytes) = read_svg(path) {
        return Ok(bytes);
    }
    let _turn = EXPORT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(bytes) = read_svg(path) {
        return Ok(bytes);
    }
    let mut bytes = generate()?;
    remove_chemdraw_doctype(&mut bytes);
    project::check_embedded_svg(&bytes)?;
    write_if_changed(path, &bytes)?;
    Ok(bytes)
}

fn remove_chemdraw_doctype(svg: &mut Vec<u8>) {
    const DECLARATION: &[u8] = b"<?xml version=\"1.0\" standalone=\"no\" ?>";
    const DOCTYPE: &[u8] = b"<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">";
    let Some(after_declaration) = svg.strip_prefix(DECLARATION) else { return };
    let after_declaration = after_declaration.trim_ascii_start();
    if after_declaration.starts_with(DOCTYPE) {
        let start = svg.len() - after_declaration.len();
        svg.drain(start..start + DOCTYPE.len());
    }
}

pub(crate) fn pdf(
    root: &Path, source: &str, run: impl FnOnce(Command) -> Result<(), String>,
) -> Result<(PathBuf, PathBuf), String> {
    let (svg, svg_bytes) = svg_with(root, source, |directory| {
        let mut command = chemex_chemdraw::prepare_svg_export(directory).map_err(err)?;
        command.current_dir(directory);
        run(command)
    })?;
    let key = sha256_hex(&svg_bytes);
    let path = cache_file(root, &format!("assets/chemex-{key}.pdf"))?;
    if read_asset(&path).is_ok_and(|bytes| valid_pdf(&bytes)) {
        return Ok((svg, path));
    }
    let _turn = EXPORT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if read_asset(&path).is_ok_and(|bytes| valid_pdf(&bytes)) {
        return Ok((svg, path));
    }
    let bytes = svg_to_pdf(&svg_bytes)?;
    if bytes.len() > MAX_SOURCE_BYTES || !valid_pdf(&bytes) {
        return Err("Chemex PDF is invalid or exceeds the file size limit".into());
    }
    write_if_changed(&path, &bytes)?;
    Ok((svg, path))
}

pub(crate) fn valid_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-") && bytes.trim_ascii_end().ends_with(b"%%EOF")
}

fn svg_to_pdf(svg: &[u8]) -> Result<Vec<u8>, String> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let mut options = usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.fontdb = FONTS
        .get_or_init(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone();
    let tree = usvg::Tree::from_str(std::str::from_utf8(svg).map_err(err)?, &options)
        .map_err(|error| format!("Could not read ChemDraw SVG: {error}"))?;
    let page = svg2pdf::PageOptions { dpi: options.dpi };
    svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|error| format!("Could not convert ChemDraw SVG to PDF: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_svg_yields_a_vector_pdf_without_a_drawing_program() {
        let root = tempfile::tempdir().unwrap();
        let source = "cached drawing";
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96px" height="48px">
            <path d="M0 0L96 48" stroke="black"/>
        </svg>"#;
        let path = svg_cache_path(root.path(), &sha256_hex(source)).unwrap();
        write_if_changed(&path, svg).unwrap();

        let (_, pdf_path) = pdf(root.path(), source, |_| panic!("SVG already cached")).unwrap();
        let document = lopdf::Document::load(pdf_path).unwrap();
        let page = document.get_pages()[&1];
        let media_box = document.get_dictionary(page).unwrap().get(b"MediaBox").unwrap();
        let dimensions: Vec<_> =
            media_box.as_array().unwrap().iter().map(|value| value.as_float().unwrap()).collect();
        assert_eq!(vec![0.0, 0.0, 72.0, 36.0], dimensions);
        let content = document.get_and_decode_page_content(page).unwrap();
        assert!(content.operations.iter().any(|op| op.operator == "l"));
    }

    #[test]
    fn drawing_preview_reuses_the_cached_svg_for_cdxml_only() {
        let fixture = crate::project::test_support::Fixture::project("drawing-preview");
        let source = "<CDXML><page/></CDXML>";
        let svg = b"<svg><path d='M 0 0 L 1 1'/></svg>";
        fixture.write("figure.cdxml", source);
        let cache = svg_cache_path(&fixture.root, &sha256_hex(source)).unwrap();
        write_if_changed(&cache, svg).unwrap();

        assert_eq!(
            Some(svg.as_slice()),
            drawing_preview_svg(&fixture.root, "figure.cdxml").unwrap().as_deref()
        );
        assert_eq!(None, drawing_preview_svg(&fixture.root, "figure.svg").unwrap());
        assert_eq!(None, drawing_preview_svg(&fixture.root, "figure.cdx").unwrap());
        assert!(drawing_preview_svg(&fixture.root, "../outside.cdxml").is_err());
    }

    #[test]
    fn lists_mnova_files_without_following_links() {
        let fixture = crate::project::test_support::Fixture::project("mnova-files");
        fixture.write("spectra/first.MNOVA", b"first");
        fixture.write("spectra/notes.txt", b"notes");
        let tree = project::scan_tree(&fixture.root, project::TreeView::Project).unwrap();

        assert_eq!(vec!["spectra/first.MNOVA"], mnova_files(&tree).collect::<Vec<_>>());
    }

    #[test]
    fn compound_preview_resolves_and_caches_the_unique_cdxml_group() {
        let fixture = crate::project::test_support::Fixture::project("compound-preview");
        let source = "<CDXML><page><group id='10'><fragment/><t><s>[[@target]]</s></t></group></page></CDXML>";
        fixture.write("scheme.cdxml", source);
        let document = read_document(&fixture.root.join("scheme.cdxml")).unwrap();
        let group = document.compound_group_source_without_fields("target").unwrap();
        let svg = b"<svg><path d='M 0 0 L 1 1'/></svg>";
        let cache = svg_cache_path(&fixture.root, &sha256_hex(&group)).unwrap();
        write_if_changed(&cache, svg).unwrap();

        assert_eq!(
            Some(svg.as_slice()),
            compound_preview_svg(&fixture.root, "target").unwrap().as_deref()
        );
        assert_eq!(None, compound_preview_svg(&fixture.root, "missing").unwrap());
        fixture.write("duplicate.cdxml", source);
        assert!(compound_preview_svg(&fixture.root, "target").is_err());
    }

    #[test]
    fn renders_compound_numbers_without_mutating_the_drawing() {
        let source = "<CDXML><page><t id='1'><s>[[@x]]</s></t></page></CDXML>";
        let document = DrawingDocument::parse(source.into()).unwrap();
        let numbers = BTreeMap::from([("x".to_string(), "3a".to_string())]);

        let rendered = render_compound_numbers(&document, &numbers).unwrap();

        assert!(rendered.contains(">3a</s>"));
        assert!(!rendered.contains("[[@x]]"));
        assert_eq!(source, document.source());
        assert!(render_compound_numbers(&document, &BTreeMap::new()).is_err());
    }
}
