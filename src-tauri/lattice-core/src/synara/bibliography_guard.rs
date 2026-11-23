//! The bibliography write guard the agent sidecar runs inside.
//!
//! The prompt tells providers to use Lattice's bibliography tools, but prompt
//! text is not an authorization boundary. On macOS the complete sidecar runs
//! inside one Seatbelt profile, so in-process agents, provider CLIs, scripts,
//! and delayed descendants all inherit the same protected-project write denial;
//! the trusted parent app brokers the allowed bibliography mutations.
//! Windows stays off until its sidecar can be confined to exactly one
//! project's staged workspace.

use std::path::Path;
use std::process::Command;

#[cfg(target_os = "macos")]
pub(crate) const AGENT_PROTECTION_SANDBOX_PROFILE: &str = concat!(
    "(version 1)\n",
    "(allow default)\n",
    "(deny file-write* (regex #\"(^|/)[.][cC][hH][eE][mM](/|$)|.*[.][bB][iI][bB]$|.*[.][cC][dD][xX]$|.*[.][cC][dD][xX][mM][lL]$|.*[.][mM][nN][oO][vV][aA]$|.*[.][mM][nN][jJ][sS]$|.*[.][mM][nN][lL]$|.*[.][mM][nN][lL][tT]$\"))",
);
/// Whether this platform blocks direct writes to protected project assets.
pub(super) const AVAILABLE: bool = cfg!(target_os = "macos");
/// The `unsupported` state's message: English for logs and copied reports;
/// the agent surfaces show their own localized notice for the state.
pub(super) const MISSING: &str =
    "The built-in agent is unavailable until Lattice can block direct writes to protected project assets on this platform.";

/// The sidecar's launch: `node` inside the guard, with Lattice's own `ps`
/// replacement (`process_inspector`), which works only inside it.
#[cfg(target_os = "macos")]
pub(super) fn command(node: &Path, home_dir: &Path, executable: &Path) -> Result<Command, String> {
    let inspector = crate::process_inspector::install_launcher(home_dir, executable)?;
    crate::process_inspector::verify_launcher(&inspector)?;
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-p")
        .arg(AGENT_PROTECTION_SANDBOX_PROFILE)
        .arg(node)
        // Children inherit this profile. A second sandbox-exec can fail
        // sandbox_apply on older macOS before Codex even starts.
        .env("LATTICE_BIBLIOGRAPHY_SANDBOX", "1")
        .env("SYNARA_PROCESS_PS_PATH", inspector);
    Ok(command)
}

/// Keep other platforms from launching the sidecar without a guard.
#[cfg(not(target_os = "macos"))]
pub(super) fn command(
    _node: &Path, _home_dir: &Path, _executable: &Path,
) -> Result<Command, String> {
    Err(MISSING.to_string())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn run_agent_sandbox(script: &str, args: &[&PathBuf]) -> bool {
        Command::new("/usr/bin/sandbox-exec")
            .args(["-p", AGENT_PROTECTION_SANDBOX_PROFILE, "/bin/sh", "-c", script, "sandbox-test"])
            .args(args)
            .status()
            .expect("run sandboxed command")
            .success()
    }

    #[test]
    fn agent_sandbox_blocks_direct_and_indirect_bibliography_writes() {
        let root = TempDir::new("bib-sandbox");
        let path = |name: &str| root.join(name);
        let (bibliography, uppercase, ordinary) = (
            root.write("references.bib", "original"),
            root.write("OTHER.BIB", "uppercase"),
            root.write("ordinary.txt", "ordinary"),
        );
        let (replacement, renamed, alias) =
            (path("replacement.tmp"), path("renamed.tmp"), path("alias.txt"));
        let (bib_alias, hardlink_alias) = (path("alias.bib"), path("hardlink.txt"));
        crate::test_support::symlink(&bibliography, &alias).expect("symlink to bibliography");

        let attempts: [(&str, &[&PathBuf]); 9] = [
            ("printf changed >> \"$1\"", &[&bibliography]),
            ("printf changed > \"$1\"", &[&bibliography]),
            ("rm \"$1\"", &[&bibliography]),
            ("printf replacement > \"$1\" && mv -f \"$1\" \"$2\"", &[&replacement, &bibliography]),
            (
                "mv \"$1\" \"$2\" && printf changed > \"$2\" && mv \"$2\" \"$1\"",
                &[&bibliography, &renamed],
            ),
            ("printf changed > \"$1\"", &[&uppercase]),
            ("ln \"$1\" \"$2\" && printf changed > \"$2\"", &[&bibliography, &hardlink_alias]),
            ("printf changed > \"$1\"", &[&alias]),
            ("ln -s \"$1\" \"$2\" && printf changed > \"$2\"", &[&ordinary, &bib_alias]),
        ];
        for (script, args) in attempts {
            assert!(!run_agent_sandbox(script, args), "sandbox allowed: {script}");
        }

        assert_eq!(fs::read_to_string(&bibliography).unwrap(), "original");
        assert_eq!(fs::read_to_string(&uppercase).unwrap(), "uppercase");
        assert!(run_agent_sandbox("printf changed > \"$1\"", &[&ordinary]));
        assert_eq!(fs::read_to_string(&ordinary).unwrap(), "changed");

        // Background descendants that outlive the sandboxed shell inherit it.
        assert!(run_agent_sandbox(
            "(sleep 0.05; printf changed > \"$1\") >/dev/null 2>&1 &",
            &[&bibliography],
        ));
        thread::sleep(Duration::from_millis(150));
        assert_eq!(fs::read_to_string(&bibliography).unwrap(), "original");
    }

    #[test]
    fn agent_sandbox_blocks_chemistry_caches_and_native_documents() {
        let root = TempDir::new("chemistry-sandbox");
        let protected = [
            root.write(".chem/molecules.sqlite", "cache"),
            root.write(".CHEM/cache/preview.svg", "preview"),
            root.write("drawings/scheme.CDX", "cdx"),
            root.write("drawings/scheme.cdxml", "cdxml"),
            root.write("spectra/sample.MnOvA", "mnova"),
            root.write("scripts/process.MNJS", "mnjs"),
            root.write("templates/report.MNL", "mnl"),
            root.write("templates/report.mnlt", "mnlt"),
        ];
        let ordinary = root.write("drawings/notes.tex", "notes");

        for path in &protected {
            assert!(
                !run_agent_sandbox("printf changed >> \"$1\"", &[path]),
                "sandbox allowed: {}",
                path.display()
            );
        }
        assert!(run_agent_sandbox("printf changed >> \"$1\"", &[&ordinary]));
        assert_eq!(fs::read_to_string(&protected[0]).unwrap(), "cache");
        assert_eq!(fs::read_to_string(&protected[2]).unwrap(), "cdx");
        assert_eq!(fs::read_to_string(&protected[4]).unwrap(), "mnova");
    }

    #[test]
    fn agent_sandbox_blocks_creating_a_chemistry_cache_directory() {
        let root = TempDir::new("chemistry-sandbox-create");
        let directory = root.join(".CHEM");
        assert!(!run_agent_sandbox("mkdir \"$1\"", &[&directory]));
        assert!(!directory.exists());
    }

    /// The broker writes a bibliography through a temporary beside it and
    /// renames that over the `.bib`. Seatbelt decides on the path when a file
    /// is opened, so a sandboxed process that opened the temporary while it
    /// had an ordinary name would keep writing through that descriptor into
    /// the real bibliography after the rename. The temporary must already be
    /// a name the guard refuses.
    #[test]
    fn agent_sandbox_cannot_write_through_the_brokers_temporary() {
        let root = TempDir::new("bib-sandbox-temporary");
        root.write("references.bib", "original");
        let ready = root.join("ready.txt");
        // Polls for the temporary, opens it for writing, waits for the rename
        // to take it away, then writes through the descriptor it still holds.
        let script = r#"
            : > "$2"
            while :; do
              for f in "$1"/.lattice-*; do
                [ -e "$f" ] || continue
                if (
                  exec 3<>"$f" || exit 1
                  while [ -e "$f" ]; do :; done
                  printf INJECTED >&3
                ) 2>/dev/null; then exit 0; fi
              done
            done
        "#;
        let mut agent = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", AGENT_PROTECTION_SANDBOX_PROFILE, "/bin/sh", "-c", script, "sandbox-test"])
            .arg(&*root)
            .arg(&ready)
            .spawn()
            .expect("run sandboxed command");
        while !ready.exists() {
            thread::sleep(Duration::from_millis(5));
        }
        // Large enough that the temporary exists for a while on every write.
        let mut payload = b"@misc{broker,}\n".to_vec();
        payload.resize(16 << 20, b'%');
        let project = crate::project_fs::ProjectDir::open(&root).unwrap();
        let mut injected = false;
        for _ in 0..8 {
            project.atomic_write("references.bib", &payload).unwrap();
            thread::sleep(Duration::from_millis(50));
            if let Some(status) = agent.try_wait().unwrap() {
                injected = status.success();
                break;
            }
        }
        let _ = agent.kill();
        let _ = agent.wait();

        let written = fs::read(root.join("references.bib")).unwrap();
        assert!(written == payload, "the bibliography changed behind the guard");
        assert!(!injected, "the sandboxed process wrote through the broker's temporary");
    }
}
