use super::{cached_svg, err, preview_svg, read_svg, sha256_hex, svg_cache_path};
use chemex_cdxml::DrawingDocument;
use std::path::Path;

pub fn preview_group_svg(
    root: &Path, document: &DrawingDocument, compound_id: &str,
) -> Result<Vec<u8>, String> {
    group_svg_with(root, document, compound_id, direct_export, preview_svg)
}

fn group_svg_with(
    root: &Path, document: &DrawingDocument, compound_id: &str,
    direct: impl FnOnce(&DrawingDocument, &str, &str) -> Result<Vec<u8>, String>,
    fallback: impl FnOnce(&Path, &str) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let revision = sha256_hex(document.source());
    let group_key = sha256_hex(format!("{revision}:{compound_id}"));
    let path = svg_cache_path(root, &format!("group-v2-{group_key}"))?;
    if let Some(bytes) = read_svg(&path) {
        return Ok(bytes);
    }
    let source = document.compound_group_source_without_fields(compound_id).map_err(err)?;
    let key = sha256_hex(&source);
    if let Some(bytes) = read_svg(&svg_cache_path(root, &key)?) {
        return Ok(bytes);
    }
    match cached_svg(&path, || {
        let bytes = direct(document, compound_id, &revision)?;
        if !has_drawing(&bytes) {
            return Err("ChemDraw returned an empty group SVG".into());
        }
        Ok(bytes)
    }) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            log::debug!("ChemDraw group export fell back to SaveAs: {error}");
            fallback(root, &source)
        }
    }
}

#[cfg(windows)]
fn direct_export(
    document: &DrawingDocument, compound_id: &str, _revision: &str,
) -> Result<Vec<u8>, String> {
    let group = document.compound_group(compound_id).map_err(err)?;
    let id = group.id.as_deref().ok_or("ChemDraw group has no ID")?;
    if document.object_by_id(id).is_none() {
        return Err("ChemDraw group ID is ambiguous".into());
    }
    let id = id.parse::<i32>().map_err(err)?;
    let source = document.compound_group_source_without_fields(compound_id).map_err(err)?;
    super::preview_worker::export(&source, &sha256_hex(&source), id)
}

#[cfg(not(windows))]
fn direct_export(_: &DrawingDocument, _: &str, _: &str) -> Result<Vec<u8>, String> {
    Err("ChemDraw automation requires Windows".into())
}

fn has_drawing(svg: &[u8]) -> bool {
    use quick_xml::{events::Event, Reader};

    let mut reader = Reader::from_reader(svg);
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag) | Event::Empty(tag)) => {
                if matches!(
                    tag.local_name().as_ref(),
                    b"path"
                        | b"text"
                        | b"image"
                        | b"use"
                        | b"line"
                        | b"rect"
                        | b"circle"
                        | b"ellipse"
                        | b"polyline"
                        | b"polygon"
                ) {
                    return true;
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWING: &str = "<CDXML><page><group id='10'><fragment/><t><s>[[@x]]</s></t></group><t><s>other</s></t></page></CDXML>";
    const SVG: &[u8] = b"<svg><path d='M 0 0 L 1 1'/></svg>";

    #[test]
    fn ignores_empty_svg_with_nonzero_canvas() {
        assert!(!has_drawing(br#"<svg width="125" height="75"></svg>"#));
        assert!(has_drawing(b"<svg><g><path/></g></svg>"));
    }

    #[test]
    fn caches_native_svg_with_its_external_doctype_removed() {
        let root = tempfile::tempdir().unwrap();
        let path = svg_cache_path(root.path(), "native").unwrap();
        let header = "<?xml version=\"1.0\" standalone=\"no\" ?>\r\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\r\n";
        let svg = format!("{header}<svg><path/></svg>");
        let cached = cached_svg(&path, || Ok(svg.into_bytes())).unwrap();
        assert_eq!(
            b"<?xml version=\"1.0\" standalone=\"no\" ?>\r\n\r\n<svg><path/></svg>",
            cached.as_slice()
        );
        let other = svg_cache_path(root.path(), "other").unwrap();
        let svg = header.replace("www.w3.org", "example.com") + "<svg/>";
        assert!(cached_svg(&other, || Ok(svg.into_bytes())).is_err());
        assert!(!other.exists());
    }

    #[test]
    fn reuses_persisted_group_preview_until_document_changes() {
        let root = tempfile::tempdir().unwrap();
        let original = DrawingDocument::parse(DRAWING.into()).unwrap();
        let unexpected_fallback = |_: &Path, _: &str| panic!("unexpected fallback");
        let generated = group_svg_with(
            root.path(),
            &original,
            "x",
            |_, _, _| Ok(SVG.to_vec()),
            unexpected_fallback,
        )
        .unwrap();
        let cached = group_svg_with(
            root.path(),
            &original,
            "x",
            |_, _, _| panic!("unexpected export"),
            unexpected_fallback,
        )
        .unwrap();
        assert_eq!(SVG, generated);
        assert_eq!(SVG, cached);

        let changed = DrawingDocument::parse(DRAWING.replace("other", "changed")).unwrap();
        let svg = b"<svg><text>new</text></svg>";
        let actual = group_svg_with(
            root.path(),
            &changed,
            "x",
            |_, _, _| Ok(svg.to_vec()),
            unexpected_fallback,
        )
        .unwrap();
        assert_eq!(svg, actual.as_slice());
    }

    #[test]
    fn empty_group_export_uses_save_as_and_reuses_its_cache() {
        let root = tempfile::tempdir().unwrap();
        let document = DrawingDocument::parse(DRAWING.into()).unwrap();
        let actual = group_svg_with(
            root.path(),
            &document,
            "x",
            |_, _, _| Ok(b"<svg/>".to_vec()),
            |root, source| {
                assert!(!source.contains("other"));
                super::super::svg_with(root, source, |dir| {
                    std::fs::write(dir.join("drawing.svg"), SVG).map_err(err)
                })
                .map(|(_, bytes)| bytes)
            },
        )
        .unwrap();
        assert_eq!(SVG, actual);
        let actual = group_svg_with(
            root.path(),
            &document,
            "x",
            |_, _, _| panic!("unexpected export"),
            |_, _| panic!("unexpected fallback"),
        )
        .unwrap();
        assert_eq!(SVG, actual);
    }
}
