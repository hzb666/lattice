use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::{Result, output, tools};

const CACHE_DIRECTORY: &str = ".chem/drawing-cache/v1";

pub(crate) fn drawings(project: &Path, stage: &Path, drawings: &[PathBuf]) -> Result<()> {
    let cache = project.join(CACHE_DIRECTORY);
    fs::create_dir_all(&cache)?;
    let cache = tools::canonicalize(&cache)?;
    for (index, directory) in drawings.iter().enumerate() {
        drawing(&cache, directory, convert)?;
        fs::copy(
            directory.join("drawing.pdf"),
            stage.join(format!("drawings/drawing-{:03}.pdf", index + 1)),
        )?;
    }
    Ok(())
}

fn drawing(
    cache: &Path,
    directory: &Path,
    convert: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let hash = Sha256::digest(fs::read(directory.join("drawing.cdxml"))?);
    let svg = cache.join(format!("{hash:x}.svg"));
    let pdf = cache.join(format!("{hash:x}.pdf"));
    if cached_file(tools::check_pdf(&pdf))? && cached_file(check_svg(&svg))? {
        fs::copy(&svg, directory.join("drawing.svg"))?;
        fs::copy(&pdf, directory.join("drawing.pdf"))?;
        println!("Reused cached CDXML export ({hash:x})");
        return Ok(());
    }

    convert(directory)?;
    let generated_svg = directory.join("drawing.svg");
    let generated_pdf = directory.join("drawing.pdf");
    check_svg(&generated_svg)?;
    tools::check_pdf(&generated_pdf)?;
    let temporary = output::Stage::create(cache)?;
    for (source, target) in [(generated_svg, svg), (generated_pdf, pdf)] {
        let staged = temporary.0.join("asset");
        fs::copy(source, &staged)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&staged)?
            .sync_all()?;
        fs::rename(staged, target)?;
    }
    Ok(())
}

fn cached_file(result: io::Result<()>) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::UnexpectedEof | ErrorKind::InvalidData
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn check_svg(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.len() == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{} is empty", path.display()),
        ));
    }
    Ok(())
}

fn convert(directory: &Path) -> Result<()> {
    let inkscape = tools::find("CHEMEX_INKSCAPE", "inkscape")?;
    let mut native = chemex_chemdraw::prepare_svg_export(directory)?;
    tools::run(&mut native, directory, "chemdraw")?;
    let mut convert = Command::new(inkscape);
    convert.args([
        "drawing.svg",
        "--export-type=pdf",
        "--export-area-page",
        "--export-filename=drawing.pdf",
    ]);
    tools::run(&mut convert, directory, "inkscape")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_export(directory: &Path) -> Result<()> {
        fs::write(directory.join("drawing.svg"), "<svg/>")?;
        fs::write(directory.join("drawing.pdf"), "%PDF-test")?;
        Ok(())
    }

    #[test]
    fn changed_input_and_missing_or_invalid_assets_are_reconverted() {
        let root = output::Stage::create(&std::env::temp_dir()).unwrap();
        let cache = root.0.join("cache");
        let directory = root.0.join("drawing");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("drawing.cdxml"), "first").unwrap();
        drawing(&cache, &directory, fixture_export).unwrap();

        fs::write(directory.join("drawing.cdxml"), "changed").unwrap();
        let mut conversions = 0;
        let mut convert = |directory: &Path| {
            conversions += 1;
            fixture_export(directory)
        };
        drawing(&cache, &directory, &mut convert).unwrap();
        let key = format!("{:x}", Sha256::digest(b"changed"));
        fs::remove_file(cache.join(format!("{key}.svg"))).unwrap();
        drawing(&cache, &directory, &mut convert).unwrap();
        fs::write(cache.join(format!("{key}.pdf")), "corrupt").unwrap();
        drawing(&cache, &directory, &mut convert).unwrap();
        drawing(&cache, &directory, &mut convert).unwrap();
        assert_eq!(3, conversions);
    }

    #[test]
    fn failed_conversion_does_not_replace_existing_entries_or_cache_partial_results() {
        let root = output::Stage::create(&std::env::temp_dir()).unwrap();
        let cache = root.0.join("cache");
        let directory = root.0.join("drawing");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&directory).unwrap();
        let input = directory.join("drawing.cdxml");
        fs::write(&input, "last successful").unwrap();
        drawing(&cache, &directory, fixture_export).unwrap();

        fs::write(&input, "changed").unwrap();
        let result = drawing(&cache, &directory, |directory| {
            fixture_export(directory)?;
            Err("conversion failed".into())
        });
        assert!(result.is_err());
        assert_eq!(2, fs::read_dir(&cache).unwrap().count());

        fs::write(input, "last successful").unwrap();
        drawing(&cache, &directory, |_| Err("expected cache hit".into())).unwrap();
    }
}
