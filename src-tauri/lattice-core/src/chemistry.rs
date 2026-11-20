//! Persistent ChemDraw assets for both the editor and LaTeX.

use crate::project;
use crate::util::{err, sha256_hex};
use chemex_cdxml::{DrawingDocument, MAX_SOURCE_BYTES};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use svg2pdf::usvg;

mod group_preview;
mod prepared;
#[cfg(windows)]
mod preview_worker;

pub use group_preview::preview_group_svg;
pub use prepared::built_preview_svg;
pub(crate) use prepared::{pdf_key, ready_path, Asset};

const CACHE: &str = ".research/cache/chemex-v1";
static EXPORT: Mutex<()> = Mutex::new(());

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

pub(crate) fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} is not a regular file", path.display()))
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
}
