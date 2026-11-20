//! Native ChemDraw export through a dedicated Windows PowerShell STA process.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ChemDraw automation requires Windows")]
    UnsupportedPlatform,
    #[error("ChemDraw export preparation: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid ChemDraw export input: {0}")]
    InvalidSource(String),
    #[error("ChemDraw SVG export ended with {0:?}")]
    Export(ExportStatus),
}

/// How ChemDraw completed its export process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportStatus {
    Success,
    Failed,
    TimedOut,
}

/// Prepare an SVG export from `drawing.cdxml` in a private build directory.
///
/// The caller runs the returned command with a timeout and captured logs, then
/// validates `drawing.svg`. Only a new copy is opened in native ChemDraw.
pub fn prepare_svg_export(directory: &Path) -> Result<Command, Error> {
    if !cfg!(windows) {
        return Err(Error::UnsupportedPlatform);
    }
    let directory = std::path::absolute(directory)?;
    let mut source = String::new();
    fs::File::open(directory.join("drawing.cdxml"))?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_string(&mut source)?;
    validate_source(&source)?;
    let copy = directory.join("chemdraw-input.cdxml");
    create_new(&copy, source.as_bytes())?;
    let script = directory.join("chemdraw-export.ps1");
    create_new(&script, include_bytes!("export.ps1"))?;
    let system = std::env::var_os("SystemRoot")
        .ok_or_else(|| Error::InvalidSource("SystemRoot is not set".into()))?;
    let program = Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(program);
    command
        .args(["-NoProfile", "-NonInteractive", "-STA", "-File"])
        .arg(script)
        .env("CHEMEX_EXPORT_SOURCE", copy)
        .env("CHEMEX_EXPORT_SVG", directory.join("drawing.svg"));
    Ok(command)
}

/// Run ChemDraw's native SVG export, stopping only its helper on timeout.
pub fn export_svg(directory: &Path, timeout: Duration) -> Result<(), Error> {
    export_svg_with(directory, timeout, &mut std::thread::sleep)
}

fn export_svg_with(
    directory: &Path,
    timeout: Duration,
    sleep: &mut impl FnMut(Duration),
) -> Result<(), Error> {
    let mut command = prepare_svg_export(directory)?;
    let mut child = command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() && directory.join("drawing.svg").is_file() {
                return Ok(());
            }
            return Err(Error::Export(ExportStatus::Failed));
        }
        if started.elapsed() >= timeout {
            child.kill()?;
            child.wait()?;
            return Err(Error::Export(ExportStatus::TimedOut));
        }
        sleep(Duration::from_millis(100));
    }
}

fn create_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

fn validate_source(source: &str) -> Result<(), Error> {
    let fail = |message: &str| Error::InvalidSource(message.into());
    if source.len() as u64 > MAX_SOURCE_BYTES {
        return Err(fail("source exceeds 64 MiB"));
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 250_000,
        entity_resolver: None,
    };
    let xml = roxmltree::Document::parse_with_options(source, options)
        .map_err(|error| Error::InvalidSource(error.to_string()))?;
    if !xml.root_element().has_tag_name("CDXML") {
        return Err(fail("expected CDXML"));
    }
    let pages: Vec<_> = xml
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("page"))
        .collect();
    // shortcut: reject multipage drawings until native per-page SVG export is verified.
    if pages.len() != 1
        || ["WidthPages", "HeightPages"].iter().any(|name| {
            pages[0]
                .attribute(*name)
                .is_some_and(|value| value.parse::<u32>() != Ok(1))
        })
    {
        return Err(fail(
            "only single-page export is verified; split pages in ChemDraw first",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Error, ExportStatus, export_svg_with, validate_source};
    use std::time::Duration;

    #[test]
    fn chemistry_and_unknown_objects_do_not_gate_native_export() {
        assert!(validate_source("<CDXML><page><fragment><n NodeType='GenericNickname'/></fragment><vendor/></page></CDXML>").is_ok());
    }

    #[test]
    fn unverified_page_layouts_are_rejected_instead_of_silently_truncated() {
        for source in [
            "<CDXML/>",
            "<CDXML><page/><page/></CDXML>",
            "<CDXML><page WidthPages='2'/></CDXML>",
            "<CDXML><page HeightPages='bad'/></CDXML>",
        ] {
            assert!(validate_source(source).is_err(), "{source}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn timed_out_export_stops_only_its_helper() {
        let directory = std::env::temp_dir().join(format!("chemex-timeout-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("drawing.cdxml"),
            "<CDXML><page><group><fragment/><t><s>x</s></t></group></page></CDXML>",
        )
        .unwrap();
        let error = export_svg_with(&directory, Duration::ZERO, &mut |_| {}).unwrap_err();
        assert!(matches!(error, Error::Export(ExportStatus::TimedOut)));
        assert!(directory.join("drawing.cdxml").is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn native_helper_receives_paths_as_data_and_opens_only_a_private_copy() {
        let directory =
            std::env::temp_dir().join(format!("chemex-分子 $()-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let source = "<CDXML><page><vendor/></page></CDXML>";
        std::fs::write(directory.join("drawing.cdxml"), source).unwrap();
        let command = super::prepare_svg_export(&directory).unwrap();
        let input = command
            .get_envs()
            .find(|(name, _)| *name == "CHEMEX_EXPORT_SOURCE")
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            directory.join("chemdraw-input.cdxml"),
            std::path::Path::new(input)
        );
        assert_eq!(
            source,
            std::fs::read_to_string(directory.join("drawing.cdxml")).unwrap()
        );
        assert_eq!(source, std::fs::read_to_string(input).unwrap());
        assert!(!command.get_args().any(|argument| argument == "-Command"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
