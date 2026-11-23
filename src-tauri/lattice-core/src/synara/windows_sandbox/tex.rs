use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{check_capability, paths_overlap, policy, process, write_policy};
use crate::latex::BuildSteps;

const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::synara) struct Preview<'a> {
    pub runtime: &'a Path,
    pub control: &'a Path,
    pub project: &'a Path,
}

impl Preview<'_> {
    pub(in crate::synara) fn run(
        &self, command: Command, timeout: Duration,
    ) -> io::Result<(Output, bool)> {
        self.run_in(command, None, timeout, || false, None)
    }

    pub(in crate::synara) fn run_build(
        &self, command: Command, scratch: &Path, timeout: Duration, cancelled: impl Fn() -> bool,
        steps: &BuildSteps,
    ) -> io::Result<(Output, bool)> {
        let result = self.run_in(command, Some(scratch), timeout, cancelled, Some(steps));
        steps.finish();
        result
    }

    fn run_in(
        &self, mut command: Command, scratch: Option<&Path>, timeout: Duration,
        cancelled: impl Fn() -> bool, steps: Option<&BuildSteps>,
    ) -> io::Result<(Output, bool)> {
        let executable = Path::new(command.get_program());
        let tex_root = tex_live_root(executable)?;
        let cwd = command
            .get_current_dir()
            .ok_or_else(|| policy::invalid("MXC TeX needs a dedicated working directory"))?;
        let (tex_root, _tex_pin) = policy::open_directory(tex_root)?;
        let (project, _project_pin) = policy::open_directory(self.project)?;
        let (cwd, _cwd_pin) = policy::open_directory(cwd)?;
        let (runtime, _runtime_pin) = policy::open_directory(self.runtime)?;
        let (control, _control_pin) = policy::open_directory(self.control)?;
        let scratch = scratch.map(policy::open_directory).transpose()?;
        let writable = scratch.as_ref().map_or(cwd.as_path(), |(path, _)| path.as_path());
        check_disjoint([&tex_root, &project, writable, &runtime, &control])?;

        let exe = policy::dos_path(&fs::canonicalize(executable)?)?;
        if !policy::contains(&tex_root, &exe) || !exe.is_file() {
            return Err(policy::invalid("The TeX executable resolves outside its installation"));
        }
        if scratch.is_some() {
            if !policy::contains(&project, &cwd) {
                return Err(policy::invalid(
                    "MXC TeX build working directory must be in the project",
                ));
            }
            require_x64_latexmk(&exe)?;
            policy::reject_existing_aliases(writable)?;
            let destination = policy::unicode(writable.as_os_str())?.replace('\\', "/");
            command
                .arg("-cd")
                .arg(format!("-outdir={destination}"))
                .arg(format!("-auxdir={destination}"));
        }
        let (executor, _executor_pin) = policy::verified_executor(&runtime)?;
        let system = policy::windows_directory()?;
        let request = request(
            &command,
            &exe,
            &Paths {
                tex: &tex_root,
                project: &project,
                cwd: &cwd,
                writable,
                control: &control,
                system: &system,
            },
        )?;
        let (request_path, _request_pin) = write_policy(&control, &request)?;
        let launcher = || policy::launcher(&executor, &system, &control);
        let mut probe = launcher();
        probe.arg("--probe").arg(&request_path).stdout(Stdio::piped()).stderr(Stdio::null());
        check_capability(probe)?;

        let mut launch = launcher();
        launch.arg(request_path);
        let output = run_to_completion(launch, &control, timeout, cancelled, steps)?;
        policy::reject_existing_aliases(writable)?;
        Ok(output)
    }
}

fn require_x64_latexmk(executable: &Path) -> io::Result<()> {
    let unsupported = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "MXC TeX builds require x64 TeX Live latexmk.exe",
        )
    };
    if !executable.file_name().is_some_and(|name| name.eq_ignore_ascii_case("latexmk.exe")) {
        return Err(unsupported());
    }
    let file = File::open(executable)?;
    let mut offset = [0; 4];
    crate::platform_fs::read_exact_at(&file, &mut offset, 0x3c)?;
    let mut signature = [0; 6];
    crate::platform_fs::read_exact_at(&file, &mut signature, u32::from_le_bytes(offset).into())?;
    if signature != *b"PE\0\0\x64\x86" {
        return Err(unsupported());
    }
    Ok(())
}

fn tex_live_root(executable: &Path) -> io::Result<&Path> {
    executable
        .parent()
        .and_then(Path::parent)
        .filter(|bin| bin.file_name().is_some_and(|name| name.eq_ignore_ascii_case("bin")))
        .and_then(Path::parent)
        .filter(|root| root.join("texmf-dist").is_dir())
        .ok_or_else(|| policy::invalid("Isolated TeX requires a TeX Live installation"))
}

fn check_disjoint(roots: [&Path; 5]) -> io::Result<()> {
    if roots
        .iter()
        .enumerate()
        .any(|(index, root)| roots[..index].iter().any(|other| paths_overlap(root, other)))
    {
        return Err(policy::invalid("MXC TeX policy roots overlap"));
    }
    Ok(())
}

struct Paths<'a> {
    tex: &'a Path,
    project: &'a Path,
    cwd: &'a Path,
    writable: &'a Path,
    control: &'a Path,
    system: &'a Path,
}

fn request(command: &Command, exe: &Path, paths: &Paths<'_>) -> io::Result<Value> {
    let command_line = std::iter::once(exe.as_os_str())
        .chain(command.get_args())
        .map(|arg| policy::unicode(arg).map(policy::quote_arg))
        .collect::<io::Result<Vec<_>>>()?
        .join(" ");
    let enumerate: BTreeSet<_> = [paths.tex, paths.project, paths.writable]
        .into_iter()
        .flat_map(|path| path.ancestors().skip(1))
        .collect();
    Ok(json!({
        "version": "1.0.0",
        "containment": "processcontainer",
        "process": {
            "commandLine": command_line, "cwd": paths.cwd, "timeout": 0,
            "inheritDefaultEnv": false, "env": environment(command, exe, paths)?,
        },
        "filesystem": {
            "readonlyPaths": [paths.tex, paths.project],
            "readwritePaths": [paths.writable],
            "deniedPaths": [paths.control],
        },
        "processContainer": {
            "filesystem": { "enumeratePaths": enumerate },
            "ui": { "isolation": "container" },
        },
        "network": { "egress": { "default": "deny" }, "ingress": { "default": "deny" } },
        "ui": { "disable": false, "clipboard": "none", "injection": false },
    }))
}

fn environment(command: &Command, exe: &Path, paths: &Paths<'_>) -> io::Result<Vec<String>> {
    let bin = exe.parent().ok_or_else(|| policy::invalid("Missing TeX Live bin"))?;
    let system_bin = paths.system.join("System32");
    let search = std::env::join_paths([bin, &system_bin]).map_err(io::Error::other)?;
    let shell = system_bin.join("cmd.exe");
    let mut env = Vec::new();
    for (name, path) in [
        ("SYSTEMROOT", paths.system.as_os_str()),
        ("WINDIR", paths.system.as_os_str()),
        ("PATH", search.as_os_str()),
        ("COMSPEC", shell.as_os_str()),
    ] {
        env.push(format!("{name}={}", policy::unicode(path)?));
    }
    let writable = policy::unicode(paths.writable.as_os_str())?;
    for name in ["TMP", "TEMP", "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        env.push(format!("{name}={writable}"));
    }
    for (name, directory) in
        [("TEXMFHOME", "texmf-home"), ("TEXMFVAR", "texmf-var"), ("TEXMFCONFIG", "texmf-config")]
    {
        env.push(format!("{name}={writable}\\{directory}"));
    }
    for (name, value) in command.get_envs() {
        if let (Some(value), Some(name @ ("TEXINPUTS" | "openout_any"))) = (value, name.to_str()) {
            env.push(format!("{name}={}", policy::unicode(value)?));
        }
    }
    Ok(env)
}

fn run_to_completion(
    mut launch: Command, control: &Path, timeout: Duration, cancelled: impl Fn() -> bool,
    steps: Option<&BuildSteps>,
) -> io::Result<(Output, bool)> {
    let stdout_file = tempfile::NamedTempFile::new_in(control)?;
    let stderr_file = tempfile::NamedTempFile::new_in(control)?;
    let mut stdout = BuildLog::new(stdout_file.reopen()?);
    let mut stderr = BuildLog::new(stderr_file.reopen()?);
    launch
        .stdout(Stdio::from(stdout_file.as_file().try_clone()?))
        .stderr(Stdio::from(stderr_file.as_file().try_clone()?));
    let (mut child, job) = process::spawn_in_job(launch)?;
    let deadline = Instant::now() + timeout;
    let interrupted: io::Result<bool> = (|| loop {
        if child.try_wait()?.is_some() {
            break Ok(false);
        }
        if cancelled() || Instant::now() >= deadline {
            break Ok(true);
        }
        if steps.is_some() {
            stdout.read_new(steps, false)?;
            stderr.read_new(steps, false)?;
        }
        std::thread::sleep(Duration::from_millis(20));
    })();
    job.terminate()?;
    let status = child.wait()?;
    let interrupted = interrupted?;
    stdout.read_new(steps, true)?;
    stderr.read_new(steps, true)?;
    Ok((Output { status, stdout: stdout.bytes, stderr: stderr.bytes }, interrupted))
}

struct BuildLog {
    reader: File,
    bytes: Vec<u8>,
    line_start: usize,
}

impl BuildLog {
    fn new(reader: File) -> Self {
        Self { reader, bytes: Vec::new(), line_start: 0 }
    }

    fn read_new(&mut self, steps: Option<&BuildSteps>, finished: bool) -> io::Result<()> {
        let start = self.bytes.len();
        (&mut self.reader).take((MAX_LOG_BYTES - start) as u64).read_to_end(&mut self.bytes)?;
        if let Some(steps) = steps {
            for (offset, byte) in self.bytes[start..].iter().enumerate() {
                if *byte == b'\n' {
                    let end = start + offset + 1;
                    steps.line(&self.bytes[self.line_start..end]);
                    self.line_start = end;
                }
            }
            if finished && self.line_start < self.bytes.len() {
                steps.line(&self.bytes[self.line_start..]);
                self.line_start = self.bytes.len();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_lines_as_the_log_grows_and_keeps_the_original_output() {
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut log = BuildLog::new(file.reopen().unwrap());
        let reports = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reports);
        let steps = BuildSteps::new(move |_, _| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        file.write_all(b"Run number 1 of rule 'pdf").unwrap();
        log.read_new(Some(&steps), false).unwrap();
        assert_eq!(0, reports.load(Ordering::Relaxed));

        file.write_all(b"latex'\r\nRun number 1 of rule 'bibtex paper'").unwrap();
        log.read_new(Some(&steps), false).unwrap();
        assert_eq!(1, reports.load(Ordering::Relaxed));
        log.read_new(Some(&steps), true).unwrap();
        assert_eq!(2, reports.load(Ordering::Relaxed));
        assert_eq!(
            b"Run number 1 of rule 'pdflatex'\r\nRun number 1 of rule 'bibtex paper'",
            log.bytes.as_slice()
        );
        let steps = steps.take();
        assert_eq!("pdflatex", steps[0].tool);
        assert_eq!("bibtex", steps[1].tool);
    }

    #[test]
    fn figure_policy_gives_only_the_working_directory_write_access() {
        let paths = Paths {
            tex: Path::new(r"D:\tools\texlive"),
            project: Path::new(r"D:\writing\project"),
            cwd: Path::new(r"D:\temp\preview"),
            writable: Path::new(r"D:\temp\preview"),
            control: Path::new(r"D:\state\control"),
            system: Path::new(r"C:\Windows"),
        };
        let exe = paths.tex.join(r"bin\windows\pdflatex.exe");
        let mut command = Command::new(&exe);
        command.env("EXAMPLE_API_KEY", "ignored");
        let policy = request(&command, &exe, &paths).unwrap();
        assert_eq!(json!([paths.writable]), policy["filesystem"]["readwritePaths"]);
        assert_eq!(json!([paths.tex, paths.project]), policy["filesystem"]["readonlyPaths"]);
        assert_eq!(json!([paths.control]), policy["filesystem"]["deniedPaths"]);
        assert_eq!("deny", policy["network"]["egress"]["default"]);
        assert_eq!("container", policy["processContainer"]["ui"]["isolation"]);
        assert!(!policy.to_string().contains("EXAMPLE_API_KEY"));
    }

    #[test]
    fn build_policy_keeps_the_project_cwd_read_only() {
        let paths = Paths {
            tex: Path::new(r"D:\tools\texlive"),
            project: Path::new(r"D:\writing\project"),
            cwd: Path::new(r"D:\writing\project\manuscript"),
            writable: Path::new(r"D:\temp\build"),
            control: Path::new(r"D:\state\control"),
            system: Path::new(r"C:\Windows"),
        };
        let exe = paths.tex.join(r"bin\windows\latexmk.exe");
        let policy = request(&Command::new(&exe), &exe, &paths).unwrap();
        assert_eq!(json!(paths.cwd), policy["process"]["cwd"]);
        assert_eq!(json!([paths.tex, paths.project]), policy["filesystem"]["readonlyPaths"]);
        assert_eq!(json!([paths.writable]), policy["filesystem"]["readwritePaths"]);
        assert!(policy["process"]["env"]
            .as_array()
            .unwrap()
            .contains(&json!(r"TEMP=D:\temp\build")));
    }

    #[test]
    #[ignore = "requires bundled MXC and x64 TeX Live on Windows"]
    fn x64_latexmk_produces_pdf_and_synctex_in_its_build_directory() {
        let test = tempfile::tempdir().unwrap();
        let project = test.path().join("original paper");
        let output = test.path().join("build output");
        let control = test.path().join("control");
        for folder in [&project, &output, &control] {
            fs::create_dir(folder).unwrap();
        }
        let tex = br"\documentclass{article}
\begin{document}
\input{section}
\end{document}
";
        fs::write(project.join("main.tex"), tex).unwrap();
        fs::write(project.join("section.tex"), "Isolated test input.").unwrap();
        let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../synara-runtime");
        let sandbox = Preview { runtime: &runtime, control: &control, project: &project };
        let mut command = crate::commands::command("latexmk");
        command.current_dir(&project).args([
            "-norc",
            "-pdf",
            "-synctex=1",
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-no-shell-escape",
            "main.tex",
        ]);
        let steps = BuildSteps::silent();
        let (result, interrupted) =
            sandbox.run_build(command, &output, Duration::from_secs(30), || false, &steps).unwrap();
        assert!(!interrupted);
        assert!(result.status.success(), "{}", crate::commands::combined_output(&result));
        assert!(fs::read(output.join("main.pdf")).unwrap().starts_with(b"%PDF-"));
        assert!(output.join("main.synctex.gz").is_file());
        assert_eq!(2, fs::read_dir(&project).unwrap().count());
        assert_eq!(&tex[..], fs::read(project.join("main.tex")).unwrap().as_slice());
    }
}
