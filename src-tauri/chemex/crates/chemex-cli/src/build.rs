use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chemex_cdxml::{DrawingDocument, FieldValues};

use crate::{Result, export, output, tools};
use chemex_cli as tex;
use chemex_cli::{DrawingRegistration, FigureRegistration, FigureRequest, PACKAGE};

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
        let requests = discover(&latexmk, stage, &project, &manuscript)?;
        if requests.is_empty() {
            return Err("manuscript contains no Chemex figure references".into());
        }
        let figures = load_figures(requests, &manuscript)?;
        let mut registrations = registrations(&figures);
        write_registrations(stage, &registrations)?;
        run_tex_mode(
            &latexmk,
            stage,
            &project,
            &manuscript,
            "numbering",
            "\\ChemexNumberingMode",
        )?;
        let numbered_figures = tex::read_figures(&stage.join("chemex-figures.dat"))?;
        tex::validate_figures(
            &figures
                .iter()
                .map(|figure| figure.request.clone())
                .collect::<Vec<_>>(),
            &numbered_figures,
            "numbering",
        )?;
        let numbers = tex::read_numbers(&stage.join("chemex-numbers.dat"))?;
        let all_fields: Vec<_> = registrations
            .iter()
            .filter_map(|registration| match registration {
                FigureRegistration::ChemDraw(drawing) => Some(drawing.fields.as_slice()),
                FigureRegistration::Mnova { .. } => None,
            })
            .flatten()
            .cloned()
            .collect();
        let values = tex::field_values(&all_fields, &numbers)?;
        if !matches!(kind, OutputKind::Render) {
            let requests: Vec<_> = figures
                .iter()
                .map(|figure| figure.request.clone())
                .collect();
            let sources: Vec<_> = figures
                .iter()
                .filter_map(|figure| {
                    figure
                        .document
                        .as_ref()
                        .map(|document| document.source().to_owned())
                })
                .collect();
            tex::attach_mnova_formulas(&requests, &mut registrations, &sources, &numbers)
                .map_err(|error| error.to_string())?;
        }
        let directories = render_figures(stage, &figures, &values)?;
        if !matches!(kind, OutputKind::Render) {
            export_figures(&project, stage, &figures, &directories, &mut registrations)?;
            write_new(
                &stage.join("chemex-register.tex"),
                tex::prepared_registrations(&registrations, &numbers)?.as_bytes(),
            )?;
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
            let final_figures = tex::read_figures(&stage.join("chemex-figures.dat"))?;
            tex::validate_figures(
                &figures
                    .iter()
                    .map(|figure| figure.request.clone())
                    .collect::<Vec<_>>(),
                &final_figures,
                "final",
            )?;
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
) -> Result<Vec<FigureRequest>> {
    run_tex_mode(
        latexmk,
        stage,
        project,
        manuscript,
        "discovery",
        "\\ChemexDiscoveryMode",
    )?;
    tex::read_figures(&stage.join("chemex-figures.dat"))
}

struct LoadedFigure {
    request: FigureRequest,
    path: PathBuf,
    document: Option<DrawingDocument>,
}

fn load_figures(requests: Vec<FigureRequest>, manuscript: &Path) -> Result<Vec<LoadedFigure>> {
    requests
        .into_iter()
        .map(|request| {
            let path = resolve_figure(manuscript, &request)?;
            let document = match request {
                FigureRequest::ChemDraw { .. } => Some(crate::load(&path)?),
                FigureRequest::Mnova { .. } => None,
            };
            Ok(LoadedFigure {
                request,
                path,
                document,
            })
        })
        .collect()
}

fn registrations(figures: &[LoadedFigure]) -> Vec<FigureRegistration> {
    figures
        .iter()
        .map(|figure| match &figure.request {
            FigureRequest::ChemDraw { counter, .. } => {
                let fields = figure
                    .document
                    .as_ref()
                    .map(|document| {
                        document
                            .fields()
                            .iter()
                            .map(|field| field.compound_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                FigureRegistration::ChemDraw(DrawingRegistration {
                    counter: counter.clone(),
                    fields,
                    asset: String::new(),
                })
            }
            FigureRequest::Mnova {
                spectrum,
                compound,
                x,
                y,
                size,
                ..
            } => FigureRegistration::Mnova {
                spectrum: *spectrum,
                compound: compound.clone(),
                x: *x,
                y: *y,
                size: size.clone(),
                formula: None,
                asset: String::new(),
            },
        })
        .collect()
}

fn write_registrations(stage: &Path, registrations: &[FigureRegistration]) -> Result<()> {
    write_new(
        &stage.join("chemex-register.tex"),
        tex::registrations(registrations).as_bytes(),
    )?;
    Ok(())
}

fn render_figures(
    stage: &Path,
    figures: &[LoadedFigure],
    values: &FieldValues,
) -> Result<Vec<PathBuf>> {
    let drawing_directory = stage.join("drawings");
    fs::create_dir(&drawing_directory)?;
    figures
        .iter()
        .enumerate()
        .map(|(index, figure)| {
            let directory = drawing_directory.join(format!("drawing-{:03}", index + 1));
            fs::create_dir(&directory)?;
            match &figure.request {
                FigureRequest::ChemDraw { .. } => {
                    let document = figure
                        .document
                        .as_ref()
                        .ok_or("ChemDraw figure has no parsed document")?;
                    let rendered = document.render_fields(values)?;
                    write_new(&directory.join("drawing.cdxml"), rendered.as_bytes())?;
                }
                FigureRequest::Mnova { .. } => {
                    fs::copy(&figure.path, directory.join("spectrum.mnova"))?;
                }
            }
            Ok(directory)
        })
        .collect()
}

fn export_figures(
    project: &Path,
    stage: &Path,
    figures: &[LoadedFigure],
    directories: &[PathBuf],
    registrations: &mut [FigureRegistration],
) -> Result<()> {
    let cache = export::cache_directory(project)?;
    let spectrum_cache = export::spectrum_cache_directory(project)?;
    for (index, ((figure, directory), registration)) in figures
        .iter()
        .zip(directories)
        .zip(registrations)
        .enumerate()
    {
        let pdf = match &figure.request {
            FigureRequest::ChemDraw { .. } => export::drawing_pdf(&cache, directory)?,
            FigureRequest::Mnova { spectrum, .. } => chemex_cli::export_spectrum_pdf(
                &spectrum_cache,
                &figure.path,
                *spectrum,
                |mut command, cwd| {
                    tools::run(&mut command, cwd, "mestrelab").map_err(|error| error.to_string())
                },
            )?,
        };
        let asset = format!("drawings/drawing-{:03}.pdf", index + 1);
        fs::copy(pdf, stage.join(&asset))?;
        match registration {
            FigureRegistration::ChemDraw(drawing) => drawing.asset = asset,
            FigureRegistration::Mnova { asset: target, .. } => *target = asset,
        }
    }
    Ok(())
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

fn resolve_figure(manuscript: &Path, request: &FigureRequest) -> Result<PathBuf> {
    let relative = Path::new(request.path());
    let extension = match request {
        FigureRequest::ChemDraw { .. } => "cdxml",
        FigureRequest::Mnova { .. } => "mnova",
    };
    if relative.is_absolute() {
        return Err(format!(
            "Chemex figure path must be relative to the manuscript: {}",
            request.path()
        )
        .into());
    }
    if !relative
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
    {
        return Err(format!("expected a .{extension} file: {}", request.path()).into());
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
