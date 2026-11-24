use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE, FILE_SHARE_READ,
};

mod git;
mod policy;
mod process;
mod state;
pub(super) mod tex;

pub(super) fn paths_overlap(first: &Path, second: &Path) -> bool {
    policy::contains(first, second) || policy::contains(second, first)
}

pub(super) fn server_entry(path: &Path) -> io::Result<PathBuf> {
    policy::dos_path(&path.canonicalize()?)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Component, Prefix};

    use super::server_entry;

    #[test]
    fn server_entry_strips_windows_verbatim_prefix() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let dist = directory.path().join("server/dist");
        fs::create_dir_all(&dist)?;
        let entry = dist.join("index.mjs");
        fs::write(&entry, "")?;
        let extended = entry.canonicalize()?;
        assert!(matches!(
            extended.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_))
        ));

        let normalized = server_entry(&extended)?;

        assert!(normalized.is_file());
        assert!(matches!(
            normalized.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        ));
        assert_eq!(server_entry(&entry)?, normalized);
        Ok(())
    }
}

pub(super) struct SandboxConfig<'a> {
    pub runtime_root: &'a Path,
    pub host_executable: &'a Path,
    pub agent_home: &'a Path,
    pub staging_parent: &'a Path,
    pub workspace_root: &'a Path,
    pub control_dir: &'a Path,
    pub original: Option<&'a Path>,
    pub stdout: File,
    pub stderr: File,
}

pub(super) struct SandboxedProcess {
    pub child: Child,
    job: process::Job,
    node: PathBuf,
    _policy: File,
    _executor: File,
    _roots: policy::Roots,
}

impl SandboxedProcess {
    pub(super) fn verify_listener(&self, pid: u32, port: u16) -> io::Result<bool> {
        self.job.verify_listener(&self.child, pid, port, &self.node)
    }

    pub(super) fn terminate(&mut self) -> io::Result<()> {
        self.job.terminate()?;
        self.child.wait().map(|_| ())
    }
}

pub(super) fn spawn(node: &Command, config: SandboxConfig<'_>) -> io::Result<SandboxedProcess> {
    if !cfg!(target_arch = "x86_64") {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "MXC requires Windows x64"));
    }
    let roots = policy::Roots::open(&config)?;
    roots.check_log(&config.stdout)?;
    roots.check_log(&config.stderr)?;
    let (executor, executor_pin) = policy::verified_executor(&roots.runtime)?;
    let (node, request) = roots.request(node)?;
    let (policy_path, policy_file) = write_policy(&roots.control, &request)?;
    let launcher = || roots.launcher(&executor);

    let mut probe = launcher();
    probe.arg("--probe").arg(&policy_path).stdout(Stdio::piped()).stderr(Stdio::null());
    check_capability(probe)?;
    state::seed_environment_id(roots.home())?;

    let mut command = launcher();
    command.arg(policy_path).stdout(Stdio::piped()).stderr(Stdio::piped());
    let (mut child, job) = process::spawn_in_job(command)?;
    let logs = forward_log(child.stdout.take(), config.stdout)
        .and_then(|()| forward_log(child.stderr.take(), config.stderr));
    if let Err(error) = logs {
        if job.terminate().is_ok() {
            let _ = child.wait();
        }
        return Err(error);
    }
    Ok(SandboxedProcess {
        child,
        job,
        node,
        _policy: policy_file,
        _executor: executor_pin,
        _roots: roots,
    })
}

fn forward_log(pipe: Option<impl Read + Send + 'static>, mut log: File) -> io::Result<()> {
    let mut pipe = pipe.ok_or_else(|| io::Error::other("MXC output pipe is missing"))?;
    thread::Builder::new().name("synara-mxc-log".into()).spawn(move || {
        if io::copy(&mut pipe, &mut log).is_err() {
            let _ = io::copy(&mut pipe, &mut io::sink());
        }
    })?;
    Ok(())
}

fn write_policy(directory: &Path, request: &serde_json::Value) -> io::Result<(PathBuf, File)> {
    let path = directory.join(format!("mxc-{}.json", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .attributes(FILE_ATTRIBUTE_TEMPORARY)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(&path)?;
    serde_json::to_writer(&mut file, request)?;
    file.flush()?;
    Ok((path, file))
}

fn check_capability(command: Command) -> io::Result<()> {
    let (mut probe, job) = process::spawn_in_job(command)?;
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if let Some(status) = probe.try_wait()? {
            return if status.success() {
                let output = probe
                    .stdout
                    .take()
                    .ok_or_else(|| io::Error::other("Missing MXC probe output"))?;
                let result: serde_json::Value = serde_json::from_reader(output.take(16 * 1024))?;
                check_probe_result(&result)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("Windows MXC rejected the requested isolation policy ({status})"),
                ))
            };
        }
        if Instant::now() >= deadline {
            job.terminate()?;
            probe.wait()?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Windows MXC capability probe timed out",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn check_probe_result(result: &serde_json::Value) -> io::Result<()> {
    let facts = &result["probes"];
    let ui = &facts["uiCapabilities"];
    let supported = result["tier"].as_str() == Some("base-container")
        && result["needsDaclAugmentation"].as_bool() == Some(false)
        && [
            "baseContainerSupportsDenyPaths",
            "baseContainerSupportsEnumeratePaths",
            "baseContainerSupportsIngressHostLoopbackAllow",
        ]
        .into_iter()
        .all(|feature| facts[feature].as_bool() == Some(true))
        && ["canBlockClipboardRead", "canBlockClipboardWrite", "canBlockInputInjection"]
            .into_iter()
            .all(|feature| ui[feature].as_bool() == Some(true));
    if !supported {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows MXC lacks required isolation capabilities",
        ));
    }
    Ok(())
}
