use super::{err, read_asset};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn export(source: &str, revision: &str, group_id: i32) -> Result<Vec<u8>, String> {
    let mut slot = WORKER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(worker) = slot.as_mut() {
        if worker.child.try_wait().map_err(err)?.is_some() {
            *slot = None;
        }
    }
    let worker = match slot.as_mut() {
        Some(worker) => worker,
        None => slot.insert(Worker::start()?),
    };
    match worker.export(source, revision, group_id) {
        Ok(Reply { error: Some(error) }) => Err(error),
        Ok(_) => read_asset(&worker.scratch.path().join("group.svg")),
        Err(error) => {
            *slot = None;
            Err(error)
        }
    }
}

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<String>,
    scratch: tempfile::TempDir,
    revision: String,
}

#[derive(serde::Deserialize)]
struct Reply {
    error: Option<String>,
}

impl Worker {
    fn start() -> Result<Self, String> {
        let scratch = tempfile::tempdir().map_err(err)?;
        let script = scratch.path().join("worker.ps1");
        fs::write(&script, include_bytes!("preview_worker.ps1")).map_err(err)?;
        let system = std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?;
        let mut command =
            Command::new(Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        crate::platform_windows::configure(&mut command);
        let mut child = command
            .args(["-NoProfile", "-NonInteractive", "-STA", "-File"])
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(err)?;
        let Some((input, stdout)) = child.stdin.take().zip(child.stdout.take()) else {
            stop(&mut child);
            return Err("ChemDraw worker has no command pipes".into());
        };
        let (send, replies) = mpsc::sync_channel(1);
        let reader = std::thread::Builder::new().name("chemdraw-replies".into());
        if let Err(error) = reader.spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        }) {
            stop(&mut child);
            return Err(err(error));
        }
        Ok(Self { child, input: Some(input), replies, scratch, revision: String::new() })
    }

    fn export(&mut self, source: &str, revision: &str, group_id: i32) -> Result<Reply, String> {
        let dir = self.scratch.path();
        let changed = self.revision != revision;
        if changed {
            fs::write(dir.join(format!("{revision}.cdxml")), source).map_err(err)?;
        }
        let started = Instant::now();
        writeln!(
            self.input.as_mut().ok_or("ChemDraw worker is shutting down")?,
            "{revision} {group_id}"
        )
        .map_err(err)?;
        let line = self
            .replies
            .recv_timeout(EXPORT_TIMEOUT)
            .map_err(|error| format!("ChemDraw worker did not reply: {error}"))?;
        let reply: Reply = serde_json::from_str(&line).map_err(err)?;
        if changed {
            if !self.revision.is_empty() {
                let _ = fs::remove_file(dir.join(format!("{}.cdxml", self.revision)));
            }
            self.revision = revision.to_owned();
        }
        if reply.error.is_none() {
            log::debug!(
                "ChemDraw group {group_id} exported in {} ms (new document: {changed})",
                started.elapsed().as_millis()
            );
        }
        Ok(reply)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.input.take();
        let started = Instant::now();
        while started.elapsed() < SHUTDOWN_TIMEOUT {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        stop(&mut self.child);
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
