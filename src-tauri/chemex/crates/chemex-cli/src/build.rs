use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chemex_cdxml::{DrawingDocument, FieldValues};

use crate::{Result, export, output, tools};
use chemex_cli as tex;
use chemex_cli::{DrawingRegistration, DrawingRequest, PACKAGE};

const MANUSCRIPT_PDF: &str = "manuscript.pdf";

#[derive(Clone, Copy)]
pub(crate) enum OutputKind {
    Render,
    Export,
    Build,
}

pub(crate) fn run(manuscript: &Path, destination: &Path, kind: OutputKind) -> Result<()> {
    let manuscript = tools::canonicalize(manuscript)?;
    crate::read_stable(&manuscript)?;
    let latexmk = tools::find("CHEMEX_LATEXMK", "latexmk")?;
    let file = matches!(kind, OutputKind::Build).then_some(MANUSCRIPT_PDF);
    output::publish_with(destination, file, |stage| {
        write_new(&stage.join("chemex.sty"), PACKAGE.as_bytes())?;
        let manuscript_parent = manuscript.parent().unwrap_or(Path::new("."));
        let project = tools::canonicalize(manuscript_parent)?;
        let drawings = discover(&latexmk, stage, &project, &manuscript)?;
        if drawings.is_empty() {
            return Err("manuscript contains no CDXML references".into());
        }
        let (documents, fields) = load_drawings(&drawings, &manuscript)?;
        let registrations = registrations(&drawings, &fields);
        write_new(
            &stage.join("chemex-register.tex"),
            tex::registrations(&registrations).as_bytes(),
        )?;
        run_tex_mode(
            &latexmk,
            stage,
            &project,
            &manuscript,
            "numbering",
            "\\ChemexNumberingMode",
        )?;
        let numbered_drawings = tex::read_drawings(&stage.join("chemex-drawings.dat"))?;
        tex::validate_drawings(&drawings, &numbered_drawings, "numbering")?;
        let numbers = tex::read_numbers(&stage.join("chemex-numbers.dat"))?;
        let all_fields: Vec<_> = fields.iter().flatten().cloned().collect();
        let values = tex::field_values(&all_fields, &numbers)?;
        let exports = render_drawings(stage, &documents, &values)?;
        if !matches!(kind, OutputKind::Render) {
            fs::write(
                stage.join("chemex-register.tex"),
                tex::prepared_registrations(&registrations, &numbers)?,
            )?;
            export::drawings(&project, stage, &exports)?;
        }
        if matches!(kind, OutputKind::Build) {
            run_tex_mode(
                &latexmk,
                stage,
                &project,
                &manuscript,
                "manuscript",
                "\\ChemexFinalMode",
            )?;
            let final_drawings = tex::read_drawings(&stage.join("chemex-drawings.dat"))?;
            tex::validate_drawings(&drawings, &final_drawings, "final")?;
            tools::check_pdf(&stage.join(MANUSCRIPT_PDF))?;
        } else if matches!(kind, OutputKind::Export) {
            fs::write(stage.join("chemex-mode.tex"), "\\ChemexPreparedMode\n")?;
        }
        Ok(())
    })?;
    println!("Completed Chemex build in {}", destination.display());
    Ok(())
}

fn discover(
    latexmk: &Path,
    stage: &Path,
    project: &Path,
    manuscript: &Path,
) -> Result<Vec<DrawingRequest>> {
    run_tex_mode(
        latexmk,
        stage,
        project,
        manuscript,
        "discovery",
        "\\ChemexDiscoveryMode",
    )?;
    tex::read_drawings(&stage.join("chemex-drawings.dat"))
}

fn load_drawings(
    requests: &[DrawingRequest],
    manuscript: &Path,
) -> Result<(Vec<DrawingDocument>, Vec<Vec<String>>)> {
    let mut documents = Vec::with_capacity(requests.len());
    let mut fields = Vec::with_capacity(requests.len());
    for request in requests {
        let path = resolve_drawing(manuscript, &request.path)?;
        let document = crate::load(&path)?;
        let keys: Vec<_> = document
            .fields()
            .iter()
            .map(|field| field.compound_id.clone())
            .collect();
        documents.push(document);
        fields.push(keys);
    }
    Ok((documents, fields))
}

fn registrations(requests: &[DrawingRequest], fields: &[Vec<String>]) -> Vec<DrawingRegistration> {
    requests
        .iter()
        .zip(fields)
        .enumerate()
        .map(|(index, (request, fields))| DrawingRegistration {
            counter: request.counter.clone(),
            fields: fields.clone(),
            asset: format!("drawings/drawing-{:03}.pdf", index + 1),
        })
        .collect()
}

fn render_drawings(
    stage: &Path,
    documents: &[DrawingDocument],
    values: &FieldValues,
) -> Result<Vec<PathBuf>> {
    let drawing_directory = stage.join("drawings");
    fs::create_dir(&drawing_directory)?;
    documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let directory = drawing_directory.join(format!("drawing-{:03}", index + 1));
            fs::create_dir(&directory)?;
            let rendered = document.render_fields(values)?;
            write_new(&directory.join("drawing.cdxml"), rendered.as_bytes())?;
            Ok(directory)
        })
        .collect()
}

fn run_tex_mode(
    latexmk: &Path,
    stage: &Path,
    project: &Path,
    manuscript: &Path,
    jobname: &str,
    mode: &str,
) -> Result<()> {
    fs::write(stage.join("chemex-mode.tex"), format!("{mode}\n"))?;
    let mut command = latexmk_command(latexmk, stage, project, manuscript, jobname)?;
    tools::run(&mut command, stage, &format!("latex-{jobname}"))
}

fn latexmk_command(
    latexmk: &Path,
    stage: &Path,
    project: &Path,
    manuscript: &Path,
    jobname: &str,
) -> Result<Command> {
    let parent = latexmk
        .parent()
        .ok_or("latexmk executable has no parent directory")?;
    let mut paths = vec![parent.to_owned()];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let mut command = Command::new(latexmk);
    command
        .env("PATH", env::join_paths(paths)?)
        .env("TEXINPUTS", tex_search_paths(stage, project))
        .args([
            "-norc",
            "-pdf",
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-no-shell-escape",
            "-g",
            &format!("-outdir={}", stage.display()),
            &format!("-jobname={jobname}"),
        ])
        .arg(manuscript);
    Ok(command)
}

fn resolve_drawing(manuscript: &Path, value: &str) -> Result<PathBuf> {
    let relative = Path::new(value);
    if relative.is_absolute() {
        return Err(format!("ChemDraw path must be relative to the manuscript: {value}").into());
    }
    Ok(tools::canonicalize(
        &manuscript.parent().unwrap_or(Path::new(".")).join(relative),
    )?)
}

fn tex_search_paths(stage: &Path, project: &Path) -> std::ffi::OsString {
    let mut paths = vec![stage.to_owned(), project.to_owned()];
    if let Some(existing) = env::var_os("TEXINPUTS") {
        paths.extend(env::split_paths(&existing));
    }
    let mut joined = env::join_paths(paths).unwrap_or_default();
    joined.push(if cfg!(windows) { ";" } else { ":" });
    joined
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
