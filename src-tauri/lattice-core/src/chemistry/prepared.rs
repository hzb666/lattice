use super::{cache_file, read_asset, read_svg, sha256_hex, CACHE};
use std::path::{Path, PathBuf};

const READY: &str = "chemex-ready.dat";

pub(crate) fn ready_path(root: &Path, namespace: &str) -> Result<PathBuf, String> {
    cache_file(root, &format!("{namespace}/{READY}"))
}

pub(crate) struct Asset<'a> {
    pub pdf: &'a str,
    pub source: &'a str,
    pub detail: &'a str,
}

impl<'a> Asset<'a> {
    pub fn parse(line: &'a str) -> Option<Self> {
        let mut parts = line.split(' ');
        let asset = Self { pdf: parts.next()?, source: parts.next()?, detail: parts.next()? };
        let detail_valid = if pdf_key(asset.pdf).is_some() {
            valid_hash(asset.detail)
        } else if spectrum_key(asset.pdf).is_some() {
            asset.detail.parse::<std::num::NonZeroUsize>().is_ok()
        } else {
            false
        };
        (parts.next().is_none() && valid_hash(asset.source) && detail_valid).then_some(asset)
    }
}

pub(crate) fn pdf_key(name: &str) -> Option<&str> {
    let hash = name.strip_prefix("chemex-")?.strip_suffix(".pdf")?;
    valid_hash(hash).then_some(hash)
}

pub(crate) fn spectrum_key(name: &str) -> Option<&str> {
    let hash = name.strip_prefix("mnova-")?.strip_suffix(".pdf")?;
    valid_hash(hash).then_some(hash)
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Reuse the whole drawing exported for this paper's latest Chemex preparation.
pub fn built_preview_svg(root: &Path, paper: &str, source: &str) -> Option<Vec<u8>> {
    let namespace = sha256_hex(paper);
    let marker = crate::project::safe_path(root, &format!("{CACHE}/{namespace}/{READY}")).ok()?;
    let marker = String::from_utf8(read_asset(&marker).ok()?).ok()?;
    let source = sha256_hex(source);
    let mut matches = marker
        .lines()
        .skip(2)
        .filter_map(Asset::parse)
        .filter(|asset| pdf_key(asset.pdf).is_some() && asset.source == source);
    let asset = matches.next()?;
    if matches.any(|other| other.detail != asset.detail || other.pdf != asset.pdf) {
        return None;
    }
    let svg_path =
        crate::project::safe_path(root, &format!("{CACHE}/assets/{}.svg", asset.detail)).ok()?;
    let svg = read_svg(&svg_path)?;
    (pdf_key(asset.pdf)? == sha256_hex(&svg)).then_some(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chemistry::{svg_cache_path, write_if_changed};

    #[test]
    fn looks_up_numbered_svgs_by_paper_and_source_revision() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let paper = "paper.tex";
        let marker = ready_path(root, &sha256_hex(paper)).unwrap();
        let first = b"<svg><text>1</text></svg>";
        let second = b"<svg><text>2</text></svg>";
        let record = |source: &str, svg: &[u8]| {
            let numbered = sha256_hex([source.as_bytes(), svg].concat());
            write_if_changed(&svg_cache_path(root, &numbered).unwrap(), svg).unwrap();
            format!("chemex-{}.pdf {} {numbered}", sha256_hex(svg), sha256_hex(source))
        };
        let first_record = record("first source", first);
        let second_record = record("second source", second);
        write_if_changed(
            &marker,
            format!("fingerprint\nregister\n{first_record}\n{second_record}").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            Some(first.as_slice()),
            built_preview_svg(root, paper, "first source").as_deref()
        );
        assert_eq!(
            Some(second.as_slice()),
            built_preview_svg(root, paper, "second source").as_deref()
        );
        assert_eq!(None, built_preview_svg(root, paper, "edited first source"));
        assert_eq!(None, built_preview_svg(root, "other.tex", "first source"));

        let other_record = record("first source", second);
        write_if_changed(
            &marker,
            format!("fingerprint\nregister\n{first_record}\n{other_record}").as_bytes(),
        )
        .unwrap();
        assert_eq!(None, built_preview_svg(root, paper, "first source"));
        write_if_changed(
            &marker,
            format!("fingerprint\nregister\nchemex-{}.pdf", sha256_hex(first)).as_bytes(),
        )
        .unwrap();
        assert_eq!(None, built_preview_svg(root, paper, "first source"));
    }
}
