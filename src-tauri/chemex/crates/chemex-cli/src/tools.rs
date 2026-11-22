use std::env;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::Result;

const RDKIT_PACKAGE_VERSION: &str = "2026.9.2";

pub(crate) fn find(variable: &str, name: &str) -> Result<PathBuf> {
    find_local(variable, name)?
        .ok_or_else(|| format!("cannot find {name}; set {variable} to its executable path").into())
}

pub(crate) fn find_local(variable: &str, name: &str) -> Result<Option<PathBuf>> {
    if let Some(path) = env::var_os(variable) {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(
                format!("{variable} does not name an executable: {}", path.display()).into(),
            );
        }
        return Ok(Some(canonicalize(&path)?));
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        for extension in ["", ".com", ".exe"] {
            let path = directory.join(format!("{name}{extension}"));
            if path.is_file() {
                return Ok(Some(canonicalize(&path)?));
            }
        }
    }
    Ok(None)
}

pub(crate) fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let path = path.canonicalize()?;
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<_> = path.as_os_str().encode_wide().collect();
        let unc: Vec<_> = r"\\?\UNC\".encode_utf16().collect();
        let prefix: Vec<_> = r"\\?\".encode_utf16().collect();
        // Legacy COM/PowerShell and TeX tools do not consistently accept verbatim paths.
        if let Some(rest) = wide.strip_prefix(unc.as_slice()) {
            let normalized: Vec<_> = r"\\".encode_utf16().chain(rest.iter().copied()).collect();
            return Ok(OsString::from_wide(&normalized).into());
        }
        if let Some(rest) = wide.strip_prefix(prefix.as_slice()) {
            return Ok(OsString::from_wide(rest).into());
        }
    }
    Ok(path)
}

pub(crate) fn run(command: &mut Command, directory: &Path, name: &str) -> Result<()> {
    run_with_timeout(command, directory, name, Duration::from_secs(120))
}

pub(crate) fn run_rdkit(script: &str, request: &str) -> Result<String> {
    let uv = find("CHEMEX_UV", "uv")?;
    let mut stage = super::output::Stage::create(&env::temp_dir())?;
    let input = stage.0.join("request.json");
    fs::write(&input, request)?;
    let mut command = Command::new(uv);
    command.args([
        "run",
        "--with",
        &format!("rdkit=={RDKIT_PACKAGE_VERSION}"),
        "--python",
        "3.13",
        "--no-project",
        "--",
        "python",
        "-c",
        script,
    ]);
    command.arg(&input);
    let result = run_with_timeout(&mut command, &stage.0, "rdkit", Duration::from_secs(300));
    if let Err(error) = fs::remove_file(&input) {
        stage.retain();
        return Err(format!(
            "could not remove temporary CDXML request {}; diagnostics retained at {}: {error}",
            input.display(),
            stage.0.display()
        )
        .into());
    }
    if let Err(error) = result {
        stage.retain();
        return Err(format!(
            "{error}; expected RDKit {RDKIT_PACKAGE_VERSION}, diagnostics retained at {}",
            stage.0.display()
        )
        .into());
    }
    fs::read_to_string(stage.0.join("rdkit.stdout.log"))
        .map_err(|error| format!("cannot read RDKit JSON output: {error}").into())
}

pub(crate) fn run_with_timeout(
    command: &mut Command,
    directory: &Path,
    name: &str,
    timeout: Duration,
) -> Result<()> {
    let log = |suffix| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(format!("{name}.{suffix}.log")))
    };
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(log("stdout")?)
        .stderr(log("stderr")?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW for helper processes.
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {name}: {error}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{name} failed ({status}); see {name}.stderr.log and {name}.stdout.log"
                )
                .into())
            };
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child)?;
            return Err(format!("{name} timed out after {} seconds", timeout.as_secs_f64()).into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn terminate_process_tree(child: &mut std::process::Child) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let system_root = env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let status = Command::new(Path::new(&system_root).join("System32/taskkill.exe"))
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return child.wait().map(|_| ());
        }
    }
    match child.kill() {
        Ok(()) => child.wait().map(|_| ()),
        Err(_) if child.try_wait()?.is_some() => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn check_pdf(path: &Path) -> std::io::Result<()> {
    let mut header = [0; 5];
    fs::File::open(path)?.read_exact(&mut header)?;
    if &header != b"%PDF-" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a PDF", path.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{name}-{}-{stamp}", std::process::id()))
    }

    #[test]
    fn pdf_validation_rejects_non_pdf_outputs() {
        let path = temporary_path("chemex-not-pdf").with_extension("bin");
        fs::write(&path, "not a pdf").unwrap();
        assert!(check_pdf(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn external_paths_preserve_unicode_without_a_verbatim_prefix() {
        let directory = temporary_path("chemex-中文");
        fs::create_dir(&directory).unwrap();
        let normalized = canonicalize(&directory).unwrap();
        assert!(
            !normalized
                .as_os_str()
                .to_string_lossy()
                .starts_with(r"\\?\")
        );
        assert_eq!(
            directory.canonicalize().unwrap(),
            normalized.canonicalize().unwrap()
        );
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn failed_helpers_report_exit_status_and_preserve_standard_error() {
        let directory = temporary_path("chemex-command-error");
        fs::create_dir(&directory).unwrap();
        let system = env::var_os("SystemRoot").unwrap();
        let mut command =
            Command::new(Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Error.WriteLine('expected-failure'); exit 7",
        ]);
        let error = run(&mut command, &directory, "failing-tool").unwrap_err();
        assert!(error.to_string().contains("7"));
        assert!(
            String::from_utf8_lossy(&fs::read(directory.join("failing-tool.stderr.log")).unwrap())
                .contains("expected-failure")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn a_hung_helper_times_out_and_retains_its_logs() {
        let path = temporary_path("chemex-timeout");
        fs::create_dir(&path).unwrap();
        let system = env::var_os("SystemRoot").unwrap();
        let program = Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(program);
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 10",
        ]);
        let error = run_with_timeout(&mut command, &path, "timeout", Duration::from_millis(100))
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(path.join("timeout.stderr.log").is_file());
        fs::remove_dir_all(path).unwrap();
    }
}
