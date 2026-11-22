use std::path::PathBuf;

use crate::{Result, tools};

const OVERRIDE: &str = "CHEMEX_MESTRENOVA";
const NAME: &str = "MestReNova";

pub(super) fn executable() -> Result<PathBuf> {
    if let Some(path) = tools::find_local(OVERRIDE, NAME)? {
        return Ok(path);
    }
    #[cfg(windows)]
    if let Some((path, _)) = windows_probe(None)? {
        return Ok(path);
    }
    Err(format!("cannot find {NAME}; set {OVERRIDE} to its executable path").into())
}

/// Find the installation used by Chemex and read its version without launching it.
#[cfg(windows)]
pub fn mestrenova_status() -> Result<Option<(PathBuf, Option<String>)>> {
    windows_probe(tools::find_local(OVERRIDE, NAME)?.as_deref())
}

#[cfg(windows)]
fn windows_probe(path: Option<&std::path::Path>) -> Result<Option<(PathBuf, Option<String>)>> {
    use std::{env, fs, path::Path, process::Command, time::Duration};

    let system = env::var_os("SystemRoot").ok_or("SystemRoot is not set")?;
    let shell = Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let stage = crate::output::Stage::create(&env::temp_dir())?;
    let mut command = Command::new(shell);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("installation.ps1"),
        ])
        .env(
            "CHEMEX_MNOVA_PROBE_PATH",
            path.unwrap_or_else(|| Path::new("")),
        );
    if let Err(error) = tools::run_with_timeout(
        &mut command,
        &stage.0,
        "mestrenova-probe",
        Duration::from_secs(3),
    ) {
        let stderr =
            fs::read_to_string(stage.0.join("mestrenova-probe.stderr.log")).unwrap_or_default();
        return if stderr.trim().is_empty() {
            Err(error)
        } else {
            Err(format!("{error}: {}", stderr.trim()).into())
        };
    }
    let output = fs::read(stage.0.join("mestrenova-probe.stdout.log"))?;
    let result: Option<(PathBuf, Option<String>)> = serde_json::from_slice(&output)?;
    result
        .map(|(path, version)| Ok((tools::canonicalize(&path)?, version)))
        .transpose()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reads_executable_version_without_starting_it() {
        let system = std::env::var_os("SystemRoot").unwrap();
        let path = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let (found, version) = windows_probe(Some(&path)).unwrap().unwrap();
        assert_eq!(tools::canonicalize(&path).unwrap(), found);
        assert!(version.is_some_and(|version| !version.is_empty()));
    }
}
