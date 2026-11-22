use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::windows_sandbox::paths_overlap;
use super::windows_workspace::Workspace;
use super::SynaraRuntime;

#[derive(Default)]
pub(super) struct Projects {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    lock: Option<File>,
    workspaces: BTreeMap<PathBuf, Staged>,
}

struct Staged {
    root: PathBuf,
    workspace: Workspace,
}

impl SynaraRuntime {
    pub(super) fn lock_workspace_store(&self) -> Result<(), String> {
        let mut state = self.projects.state.lock().map_err(|error| error.to_string())?;
        if state.lock.is_none() {
            fs::create_dir_all(self.control_dir()).map_err(|error| error.to_string())?;
            state.lock = Some(
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(self.control_dir().join("workspaces.lock"))
                    .map_err(|error| {
                        format!("Could not lock the agent workspace store: {error}")
                    })?,
            );
        }
        Ok(())
    }

    pub(super) fn prepare_workspace(&self, root: &Path) -> Result<PathBuf, String> {
        let original = root.canonicalize().map_err(|error| error.to_string())?;
        let mut state = self.projects.state.lock().map_err(|error| error.to_string())?;
        if state.lock.is_none() {
            return Err("The isolated agent workspace store is unavailable.".into());
        }
        if let Some(staged) = state.workspaces.get(&original) {
            return Ok(staged.root.clone());
        }
        let parent = self.workspace_parent().canonicalize().map_err(|error| error.to_string())?;
        let home = self.home_dir.canonicalize().map_err(|error| error.to_string())?;
        let control = self.control_dir().canonicalize().map_err(|error| error.to_string())?;
        let runtime = self
            .server_entry
            .ancestors()
            .nth(3)
            .ok_or("The bundled agent runtime has no directory.")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if [&parent, &home, &control, &runtime]
            .into_iter()
            .any(|directory| paths_overlap(directory, &original))
        {
            return Err("The project overlaps a directory reserved for the isolated agent.".into());
        }
        let original_key = original.to_str().ok_or("The project location is not Unicode.")?;
        let key = format!("{:x}", Sha256::digest(original_key.as_bytes()));
        let staging = parent.join(key);
        let workspace = Workspace::reopen(&original, &staging)?;
        let root = crate::sidecar::node_script_path(&staging);
        state.workspaces.insert(original, Staged { root: root.clone(), workspace });
        Ok(root)
    }

    pub(super) fn refresh_workspace(&self, root: &Path) -> Result<(), String> {
        self.with_workspace(root, Workspace::refresh)
    }

    pub(super) fn apply_workspace(
        &self, root: &Path, paths: &BTreeSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, String> {
        self.with_workspace(root, |workspace| workspace.reconcile_checkpoint(paths))
    }

    fn with_workspace<T>(
        &self, root: &Path, action: impl FnOnce(&mut Workspace) -> Result<T, String>,
    ) -> Result<T, String> {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut state = self.projects.state.lock().map_err(|error| error.to_string())?;
        let staged = state
            .workspaces
            .get_mut(&root)
            .ok_or("The isolated agent workspace is unavailable.")?;
        action(&mut staged.workspace)
    }
}
