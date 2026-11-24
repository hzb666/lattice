use std::path::Path;
use std::process::{Command, Output};

use crate::synara::ProjectBuild;

use super::{
    cancelled_build, choose_compile_root, commands, compiled_document, diagnose, diagnostic_paths,
    engine_args, error_mode, is_one_off_root, latexmk_into, log_conference_template, missing_root,
    pages_written, parse_diagnostics, project, record_result, search_paths, stop_requested,
    trim_log, BuildResult, BuildSession,
};
use crate::latex::aux_files::{self, AuxFiles};
use crate::latex::chemex::{self, Preparation};

pub(crate) fn isolated_project_build(
    request: &ProjectBuild<'_>, run: impl Fn(Command, &Path) -> Result<(Output, bool), String>,
    publish: impl FnOnce(&Path, &str, AuxFiles) -> Result<Vec<String>, String>,
) -> Result<BuildResult, String> {
    let _session = BuildSession::open(request.active);
    let root = request.root;
    choose_compile_root(root, request.open_document)?;
    let layout = super::beside::layout_for(root, request.layout);
    let manifest = project::read_manifest(root)?;
    let document = compiled_document(root, &manifest)?;
    if !project::safe_path(root, &document)?.is_file() {
        return Err(missing_root(&document));
    }
    let chemistry = if chemex::enabled_project(root, &document)? {
        Some(
            Preparation {
                root,
                document: &document,
                engine: &manifest.engine,
                active: request.active,
                steps: request.steps,
            }
            .run_isolated(&run)?,
        )
    } else {
        None
    };
    let output_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut command = latexmk_into(root, AuxFiles::Beside, &document);
    command.args(["-interaction=nonstopmode", "-synctex=1", "-file-line-error", "-recorder"]);
    command.arg(error_mode(&manifest)).args(engine_args(root, &manifest));
    if request.force {
        command.arg("-g");
    }
    search_paths::search_project_folder(&mut command, &document);
    if let Some(chemistry) = chemistry {
        chemistry.configure(&mut command)?;
    }
    command.arg(&document);

    let (output, interrupted) = run(command, output_dir.path())?;
    let log = commands::combined_output(&output);
    let mut result = if stop_requested(request.active) {
        cancelled_build(request.steps.started(), &log, &document)
    } else if interrupted {
        super::super::build_stall::stalled_build(request.steps.started(), &log, &document)
    } else {
        complete(request, &document, &log, output.status.success(), || {
            publish(output_dir.path(), &document, layout)
        })?
    };
    result.one_off = is_one_off_root(root);
    result.steps = request.steps.take();
    result.aux_folder = aux_files::relative_folder(&document, layout);
    record_result(&result);
    Ok(result)
}

fn complete(
    request: &ProjectBuild<'_>, document: &str, log: &str, success: bool,
    publish: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<BuildResult, String> {
    let (outputs, diagnostics, tex_log) = if success {
        let outputs = publish()?;
        let (diagnostics, tex_log) = diagnose(request.root, document, log, None, None);
        (outputs, diagnostics, tex_log)
    } else {
        let mut diagnostics = parse_diagnostics(log);
        diagnostic_paths::locate_in_project(request.root, document, &mut diagnostics);
        (Vec::new(), diagnostics, String::new())
    };
    Ok(BuildResult {
        success,
        has_pdf: success,
        log: trim_log(log),
        duration_ms: request.steps.started().elapsed().as_millis(),
        diagnostics,
        root_document: document.to_owned(),
        one_off: false,
        venue: log_conference_template(&tex_log).or_else(|| log_conference_template(log)),
        steps: Vec::new(),
        pages: pages_written(&tex_log).or_else(|| pages_written(log)),
        synctex: success.then_some(true),
        outputs,
        unchanged: false,
        aux_folder: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    use crate::latex::{abort, ActiveBuild, BuildSteps};
    use crate::test_support::TempDir;

    use super::*;

    #[test]
    fn a_stopped_isolated_build_never_publishes_artifacts() {
        let root = TempDir::new("isolated-build");
        project::write_manifest(&root, &project::default_manifest("paper")).unwrap();
        root.write("main.tex", "\\documentclass{article}\n\\begin{document}Test\\end{document}");
        let active = ActiveBuild::default();
        let steps = BuildSteps::silent();
        let request = ProjectBuild {
            root: &root,
            force: false,
            unattended: false,
            active: &active,
            open_document: Some("main.tex"),
            steps: &steps,
            layout: AuxFiles::Beside,
        };
        let result = isolated_project_build(
            &request,
            |_, _| {
                assert!(abort(&active)?);
                Ok((
                    Output { status: ExitStatus::from_raw(0), stdout: vec![], stderr: vec![] },
                    true,
                ))
            },
            |_, _, _| panic!("stopped build reached artifact publication"),
        )
        .unwrap();
        assert_eq!(Some("build-cancelled"), result.diagnostics[0].code);
        assert_eq!("", result.aux_folder);
    }
}
