use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::text_edit::{self, ProjectTextEdit};
use super::{invalid, Workspace};
use crate::project_fs::ProjectDir;

const MAX_HISTORY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectTextUndo {
    thread_id: String,
    turn_ids: Vec<String>,
    operation_id: String,
    #[serde(default)]
    redo: bool,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectTextHistory {
    thread_id: String,
    turn_id: String,
    expires_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct TurnHistory {
    entries: BTreeMap<String, Revision>,
    #[serde(default)]
    undone_by: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct UndoReceipt {
    entries: BTreeMap<String, Revision>,
    conflicts: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Revision {
    before: String,
    after: String,
}

impl Workspace {
    pub(super) fn original_edit_paths(
        &self, control: &Path, request: &ProjectTextHistory,
    ) -> io::Result<Vec<String>> {
        self.check_host_managed()?;
        text_edit::check_deadline(request.expires_at)?;
        let log = Journal::open(control, &request.thread_id, &request.turn_id)?;
        Ok(log
            .read::<TurnHistory>()?
            .filter(|turn| turn.undone_by.is_none())
            .map_or_else(Vec::new, |turn| turn.entries.into_keys().collect()))
    }

    pub(super) fn edit_original(
        &mut self, control: &Path, edit: &ProjectTextEdit,
        before_write: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.check_host_managed()?;
        let path = edit.checked_path()?;
        let destination = text_edit::text_snapshot(&self.original, [path.as_path()])?;
        let before = text_edit::read_text(&destination, &path)?;
        let after = edit.replacement(&before, &path)?;
        let updates = BTreeMap::from([(path.clone(), after.clone())]);

        let journal = Journal::open(control, &edit.thread_id, &edit.turn_id)?;
        let mut history: TurnHistory = journal.read()?.unwrap_or_default();
        if history.undone_by.is_some() {
            return Err(invalid("This Agent turn has already been undone", &path));
        }
        let previous = serde_json::to_vec(&history)?;
        let relative = path.to_string_lossy().replace('\\', "/");
        if let Some(existing) = history.entries.get_mut(&relative) {
            if existing.after != before {
                return Err(invalid("The writer changed this manuscript during the turn", &path));
            }
            existing.after = after;
        } else {
            history.entries.insert(relative, Revision { before, after });
        }
        edit.check_deadline()?;
        before_write()?;
        journal.write(&serde_json::to_vec(&history)?)?;
        if let Err(error) =
            text_edit::publish(control, &destination, &updates, &mut self.recovery_required)
        {
            if !self.recovery_required {
                journal.write(&previous)?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn undo_original(
        &mut self, control: &Path, request: &ProjectTextUndo,
        before_write: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Vec<String>> {
        self.check_host_managed()?;
        text_edit::check_deadline(request.expires_at)?;
        if request.turn_ids.is_empty() || request.turn_ids.len() > 128 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid Undo turn count."));
        }
        let receipt = Journal::undo(control, &request.thread_id, &request.operation_id)?;
        let stored: Option<UndoReceipt> = receipt.read()?;
        let turn_journals = request
            .turn_ids
            .iter()
            .map(|turn| Journal::open(control, &request.thread_id, turn))
            .collect::<io::Result<Vec<_>>>()?;
        let revisions = if stored.is_none() && !request.redo {
            turn_journals
                .iter()
                .map(|journal| journal.read::<TurnHistory>())
                .collect::<io::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .filter(|turn| turn.undone_by.is_none())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let names = stored
            .iter()
            .flat_map(|entry| entry.entries.keys())
            .chain(revisions.iter().flat_map(|entry| entry.entries.keys()));
        let paths = names
            .map(|name| Ok((name.clone(), text_edit::checked_path(name)?)))
            .collect::<io::Result<BTreeMap<_, _>>>()?;
        let destination =
            text_edit::text_snapshot(&self.original, paths.values().map(PathBuf::as_path))?;
        let (planned, conflicts) = match stored {
            Some(stored) => (stored.entries, stored.conflicts),
            None if request.redo => return Ok(Vec::new()),
            None => {
                let planned = plan_undo(&destination, &paths, revisions)?;
                receipt.write(&serde_json::to_vec(&planned)?)?;
                (planned.entries, planned.conflicts)
            }
        };
        let mut conflicts = if request.redo { BTreeSet::new() } else { conflicts };
        let mut updates = BTreeMap::new();
        for (name, revision) in planned {
            let path = &paths[&name];
            let current = text_edit::read_text(&destination, path)?;
            let updated = if request.redo {
                merge_to(&revision.after, &current, &revision.before, &name, &mut conflicts)
            } else if current == revision.before || current == revision.after {
                revision.after
            } else {
                return Err(invalid(
                    "The writer changed the manuscript after Undo was prepared",
                    path,
                ));
            };
            if current != updated {
                updates.insert(path.clone(), updated);
            }
        }
        if request.redo && !conflicts.is_empty() {
            return Err(io::Error::other(
                "The writer changed a manuscript while its Undo was being restored.",
            ));
        }
        text_edit::check_deadline(request.expires_at)?;
        if !updates.is_empty() {
            before_write()?;
            text_edit::publish(control, &destination, &updates, &mut self.recovery_required)?;
        }
        for journal in turn_journals {
            let Some(mut history) = journal.read::<TurnHistory>()? else { continue };
            let owner = history.undone_by.as_deref();
            if request.redo && owner == Some(request.operation_id.as_str()) {
                history.undone_by = None;
            } else if !request.redo && owner.is_none() {
                history.undone_by = Some(request.operation_id.clone());
            } else {
                continue;
            }
            journal.write(&serde_json::to_vec(&history)?)?;
        }
        Ok(conflicts.into_iter().collect())
    }

    fn check_host_managed(&self) -> io::Result<()> {
        self.check_ready()?;
        if !self.host_managed {
            return Err(invalid("Host manuscript writes are disabled", &self.original.path));
        }
        Ok(())
    }
}

fn plan_undo(
    destination: &super::snapshot::Snapshot<'_>, paths: &BTreeMap<String, PathBuf>,
    turns: Vec<TurnHistory>,
) -> io::Result<UndoReceipt> {
    let mut receipt = UndoReceipt::default();
    for (name, path) in paths {
        let before = text_edit::read_text(destination, path)?;
        receipt.entries.insert(name.clone(), Revision { after: before.clone(), before });
    }
    for turn in turns {
        for (name, change) in turn.entries {
            let entry = receipt
                .entries
                .get_mut(&name)
                .ok_or_else(|| io::Error::other("Manuscript was not included in the Undo batch"))?;
            entry.after = merge_to(
                &change.after,
                &entry.after,
                &change.before,
                &name,
                &mut receipt.conflicts,
            );
        }
    }
    Ok(receipt)
}

fn merge_to(
    base: &str, current: &str, target: &str, path: &str, conflicts: &mut BTreeSet<String>,
) -> String {
    let mut merger = diffy::MergeOptions::new();
    merger.set_conflict_style(diffy::ConflictStyle::Diff3);
    match merger.merge(base, current, target) {
        Ok(merged) => merged,
        Err(marked) => {
            conflicts.insert(path.to_owned());
            marked
        }
    }
}

struct Journal<'a> {
    root: &'a Path,
    name: String,
}

impl<'a> Journal<'a> {
    fn open(root: &'a Path, thread: &str, turn: &str) -> io::Result<Self> {
        Self::named(root, "turns", thread, turn)
    }

    fn undo(root: &'a Path, thread: &str, operation: &str) -> io::Result<Self> {
        Self::named(root, "undo", thread, operation)
    }

    fn named(root: &'a Path, directory: &str, thread: &str, id: &str) -> io::Result<Self> {
        if [thread, id]
            .iter()
            .any(|value| value.is_empty() || value.len() > 256 || value.contains('\0'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid Agent turn identity.",
            ));
        }
        let key = crate::util::sha256_hex(format!("{thread}\0{id}"));
        Ok(Self { root, name: format!("manuscript-{directory}/{key}.json") })
    }

    fn read<T: serde::de::DeserializeOwned>(&self) -> io::Result<Option<T>> {
        let file = match File::open(self.root.join(&self.name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(serde_json::from_reader(file.take(MAX_HISTORY_BYTES))?))
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err(io::Error::other("The Agent turn exceeds its manuscript history limit."));
        }
        ProjectDir::open(self.root)
            .and_then(|directory| directory.atomic_write(&self.name, bytes))
            .map_err(io::Error::other)
    }
}
