//! Host-side filesystem boundary for an isolated MXC project workspace.

mod checkpoint_git;
mod files;
mod host_edits;
mod native;
mod snapshot;
pub(super) mod tex_outputs;
mod text_edit;
mod transaction;

pub use host_edits::{ProjectTextHistory, ProjectTextUndo};
pub(super) use native::identity as file_identity;
pub use text_edit::ProjectTextEdit;

#[cfg(test)]
mod tests;

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use native::{Identity, Root};
use snapshot::{Entries, Snapshot};

const RECOVERY_PREFIX: &str = ".lattice-mxc-";

pub(super) struct Workspace {
    original: Root,
    staging: Root,
    baseline: Entries,
    recovery_required: bool,
    host_managed: bool,
    _lease: Lease,
}

impl Workspace {
    /// Copy the project into a fresh stage. The caller owns stage cleanup.
    #[cfg(test)]
    pub(super) fn prepare(original: &Path, staging: &Path) -> Result<Self, String> {
        Self::open(original, staging, false, false).map_err(|error| error.to_string())
    }

    /// Reuse an existing stage only when its project contents match the writer's tree.
    /// The stage's root `.git` belongs to the sandbox and is reserved across restarts.
    #[cfg(test)]
    pub(super) fn reopen(original: &Path, staging: &Path) -> Result<Self, String> {
        Self::open(original, staging, true, false).map_err(|error| error.to_string())
    }

    pub(super) fn open_host_managed(original: &Path, staging: &Path) -> Result<Self, String> {
        Self::open(original, staging, true, true).map_err(|error| error.to_string())
    }

    fn open(original: &Path, path: &Path, reuse: bool, host_managed: bool) -> io::Result<Self> {
        let original = Root::open(original)?;
        let (staging, created) = match Root::open(path) {
            Ok(root) => (root, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (Root::create_staging(path, &original)?, true)
            }
            Err(error) => return Err(error),
        };
        if original.overlaps(&staging) {
            return Err(invalid("Project and staging roots overlap", &staging.path));
        }
        let lease = Lease::acquire(&original, &staging)?;
        let mut recovery_required = false;
        let (baseline, initialize_checkpoint) = {
            let initial = if host_managed {
                Snapshot::read_manuscripts(&original)?
            } else {
                Snapshot::read_project(&original)?
            };
            let stage = if host_managed {
                Snapshot::read_manuscripts(&staging)?
            } else {
                Snapshot::read(&staging)?
            };
            let initialize_checkpoint =
                (host_managed || created || (!reuse && stage.entries.is_empty()))
                    && !snapshot::has_checkpoint_root(&staging)?;
            if !initialize_checkpoint {
                snapshot::validate_checkpoint_root(&staging)?;
            }
            if host_managed {
                let steps = snapshot::plan_mirror(&initial, &stage)?;
                transaction::run(&initial, &stage, steps, &mut recovery_required)?;
            } else if reuse && !created {
                if !snapshot::same_contents(&initial.entries, &stage.entries) {
                    return Err(invalid(
                        "Existing staging differs from the project and needs recovery",
                        &staging.path,
                    ));
                }
            } else {
                if !stage.entries.is_empty() {
                    return Err(invalid(
                        "Staging must be empty before preparing the workspace",
                        &staging.path,
                    ));
                }
                let steps = snapshot::plan(&stage.entries, &initial, &stage, false)?;
                transaction::run(&initial, &stage, steps, &mut recovery_required)?;
            }
            (initial.entries, initialize_checkpoint)
        };
        if initialize_checkpoint {
            snapshot::initialize_checkpoint_root(&staging).map_err(|error| {
                snapshot::contextual(error, "Initialize checkpoint Git", &staging.path)
            })?;
        }
        Ok(Self { original, staging, baseline, recovery_required, host_managed, _lease: lease })
    }

    #[cfg(test)]
    pub(super) fn reconcile(&mut self) -> Result<Vec<PathBuf>, String> {
        self.reconcile_inner(None, || Ok(())).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(super) fn reconcile_checkpoint(
        &mut self, targets: &BTreeMap<PathBuf, String>,
        before_publish: impl FnOnce() -> io::Result<()>,
    ) -> Result<Vec<PathBuf>, String> {
        self.reconcile_inner(Some(targets), before_publish).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn reconcile_inner(
        &mut self, targets: Option<&BTreeMap<PathBuf, String>>,
        before_publish: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Vec<PathBuf>> {
        self.check_ready()?;
        if self.host_managed {
            return Err(invalid(
                "Checkpoint contents cannot write to the project",
                &self.original.path,
            ));
        }
        let stage = Snapshot::read(&self.staging)?;
        let original = Snapshot::read_project(&self.original)?;
        let steps = snapshot::plan(&self.baseline, &stage, &original, true)?;
        if let Some(targets) = targets {
            snapshot::check_checkpoint_paths(&steps, targets)?;
            checkpoint_git::verify_checkpoint_contents(&stage, targets)?;
        }
        if !steps.is_empty() {
            before_publish()?;
        }
        let applied = transaction::run(&stage, &original, steps, &mut self.recovery_required)?;
        let mut paths = Vec::with_capacity(applied.len());
        for (path, after) in applied {
            if let Some(record) = after {
                self.baseline.insert(path.clone(), record);
            } else {
                self.baseline.remove(&path);
            }
            paths.push(path);
        }
        Ok(paths)
    }

    /// Import writer changes before a turn, while staging is quiescent.
    pub(super) fn refresh(&mut self) -> Result<(), String> {
        self.refresh_inner().map_err(|error| error.to_string())
    }

    fn refresh_inner(&mut self) -> io::Result<()> {
        self.check_ready()?;
        if self.host_managed {
            let stage = Snapshot::read_manuscripts(&self.staging)?;
            let original = Snapshot::read_manuscripts(&self.original)?;
            let steps = snapshot::plan_mirror(&original, &stage)?;
            transaction::run(&original, &stage, steps, &mut self.recovery_required)?;
            self.baseline = original.entries;
            return Ok(());
        }
        let stage = Snapshot::read(&self.staging)?;
        if !snapshot::same_contents(&self.baseline, &stage.entries) {
            return Err(invalid("Staging has unpublished changes", &self.staging.path));
        }
        let original = Snapshot::read_project(&self.original)?;
        let steps = snapshot::plan(&stage.entries, &original, &stage, false)?;
        transaction::run(&original, &stage, steps, &mut self.recovery_required)?;
        self.baseline = original.entries;
        Ok(())
    }

    fn check_ready(&self) -> io::Result<()> {
        if self.recovery_required {
            return Err(invalid("Workspace needs writeback recovery", &self.original.path));
        }
        Ok(())
    }
}

static ACTIVE: LazyLock<Mutex<HashMap<Identity, Vec<Identity>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Lease([Identity; 2]);

impl Lease {
    fn acquire(original: &Root, staging: &Root) -> io::Result<Self> {
        let ids = [native::identity(&original.directory)?, native::identity(&staging.directory)?];
        let ancestries = [original.ancestry(), staging.ancestry()];
        let mut active = ACTIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (id, ancestors) in ids.iter().zip(&ancestries) {
            if active.iter().any(|(root, chain)| ancestors.contains(root) || chain.contains(id)) {
                return Err(invalid(
                    "Workspace root overlaps another active workspace",
                    &original.path,
                ));
            }
        }
        active.extend(ids.into_iter().zip(ancestries));
        Ok(Self(ids))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for id in self.0 {
            active.remove(&id);
        }
    }
}

fn invalid(message: &str, path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{message}: {}", path.display()))
}

fn permitted_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|text| {
        let stem = text.split('.').next().unwrap_or_default();
        let trimmed_stem = stem.trim_end_matches(' ');
        !text.contains(['/', '\\'])
            && text.encode_utf16().count() <= 255
            && crate::platform_windows::valid_name(name)
            && (trimmed_stem == stem
                || crate::platform_windows::valid_name(OsStr::new(trimmed_stem)))
    })
}
