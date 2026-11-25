mod build;
mod export;
mod output;
mod tools;

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use chemex_cdxml::{DrawingDocument, MAX_SOURCE_BYTES};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELP: &str = "\
Chemex — CDXML inspection and compound fields (no UI)

  chemex inspect SOURCE.cdxml
  chemex structures SOURCE.cdxml
  chemex render MANUSCRIPT.tex NEW_OUTPUT_DIRECTORY
  chemex export MANUSCRIPT.tex NEW_OUTPUT_DIRECTORY
  chemex build MANUSCRIPT.tex NEW_OUTPUT_DIRECTORY

Fields: [[@compound-001]], [[@parent|child]]
Decorated fields: [[@compound-001_ Scheme ^ product ]]
LaTeX refs: \\chemex{key}, \\chemex{parent|child}, or \\chemex{key1, key2}
LaTeX figures: \\includegraphics{drawing.cdxml}, \\chemn{sample.mnova}, or \\chemd{sample.mnova}

render creates numbered CDXML copies; export adds native vector PDFs and Mnova spectrum PDFs.
build publishes only the manuscript PDF. SVG and PDF exports are cached per project.
Tools: CHEMEX_INKSCAPE, CHEMEX_LATEXMK, and CHEMEX_UV, or executables on PATH.
Original TeX and CDXML files are never overwritten.
";

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1).collect()) {
        eprintln!("chemex: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: Vec<OsString>) -> Result<()> {
    match arguments.as_slice() {
        [] => {
            print!("{HELP}");
            Ok(())
        }
        [help] if help == "--help" || help == "-h" => {
            print!("{HELP}");
            Ok(())
        }
        [command, source] if command == "inspect" => inspect(Path::new(source)),
        [command, source] if command == "structures" => structures(Path::new(source)),
        [command, manuscript, destination] if command == "render" => build::run(
            Path::new(manuscript),
            Path::new(destination),
            build::OutputKind::Render,
        ),
        [command, manuscript, destination] if command == "export" => build::run(
            Path::new(manuscript),
            Path::new(destination),
            build::OutputKind::Export,
        ),
        [command, manuscript, destination] if command == "build" => build::run(
            Path::new(manuscript),
            Path::new(destination),
            build::OutputKind::Build,
        ),
        _ => Err(format!("invalid arguments\n\n{HELP}").into()),
    }
}

fn inspect(source: &Path) -> Result<()> {
    let document = load(source)?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, &document)?;
    writeln!(stdout)?;
    Ok(())
}

fn structures(source: &Path) -> Result<()> {
    let source = read_stable(source)?;
    let report =
        chemex_cli::analyze_compounds_with_rdkit(&source).map_err(|error| error.to_string())?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, &report)?;
    writeln!(stdout)?;
    Ok(())
}

fn load(path: &Path) -> Result<DrawingDocument> {
    Ok(DrawingDocument::parse(read_stable(path)?)?)
}

fn read_stable(path: &Path) -> Result<String> {
    let before = read_source(path)?;
    std::thread::sleep(Duration::from_millis(100));
    let bytes = read_source(path)?;
    if before != bytes {
        return Err(format!(
            "{} changed while reading; wait for ChemDraw to finish saving",
            path.display()
        )
        .into());
    }
    let source = String::from_utf8(bytes).map_err(|error| {
        format!(
            "{} is not UTF-8 CDXML ({error}); CDX needs a converted copy",
            path.display()
        )
    })?;
    Ok(source)
}

fn read_source(path: &Path) -> Result<Vec<u8>> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(format!("{} exceeds the source size limit", path.display()).into());
    }
    Ok(bytes)
}
