use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use super::native;
use super::snapshot::{self, Contents, Record, Snapshot, Step};
use super::{invalid, RECOVERY_PREFIX};

type Applied = Vec<(PathBuf, Option<Record>)>;

pub(super) fn run(
    source: &Snapshot<'_>, destination: &Snapshot<'_>, steps: Vec<Step>,
    recovery_required: &mut bool,
) -> io::Result<Applied> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }
    check_recovery(destination.root)?;
    let mut batch = Batch::new(destination)?;
    let result = batch
        .write_journal(&steps)
        .and_then(|()| batch.prepare(source, &steps))
        .and_then(|()| {
            if Snapshot::read(source.root)?.entries != source.entries {
                return Err(invalid(
                    "Source workspace changed before publication",
                    &source.root.path,
                ));
            }
            Ok(())
        })
        .and_then(|()| batch.publish(&steps));
    match result {
        Ok(applied) => match batch.finish() {
            Ok(()) => Ok(applied),
            Err(error) => {
                *recovery_required = true;
                Err(batch
                    .recovery_error("Workspace changes were applied but cleanup failed", &error))
            }
        },
        Err(error) => match batch.rollback() {
            Ok(()) => Err(error),
            Err(rollback) => {
                *recovery_required = true;
                Err(batch.recovery_error(
                    &format!("Writeback failed ({error}) and rollback was incomplete"),
                    &rollback,
                ))
            }
        },
    }
}

pub(super) fn check_recovery(root: &super::native::Root) -> io::Result<()> {
    if native::children(&root.directory)?
        .iter()
        .any(|entry| entry.name.to_ascii_lowercase().starts_with(RECOVERY_PREFIX))
    {
        return Err(invalid("Unfinished workspace transaction", &root.path));
    }
    Ok(())
}

struct Batch {
    prefix: String,
    journal_path: PathBuf,
    journal: File,
    directories: BTreeMap<PathBuf, Arc<File>>,
    created_dirs: Vec<PathBuf>,
    files: Vec<PendingFile>,
}

#[derive(Serialize)]
struct JournalEntry<'a> {
    relative_path: &'a Path,
    before: Option<Record>,
    after: Option<Record>,
    backup_name: Option<String>,
    temporary_name: Option<String>,
}

impl Batch {
    fn new(destination: &Snapshot<'_>) -> io::Result<Self> {
        let prefix = format!("{RECOVERY_PREFIX}{}", Uuid::new_v4().simple());
        let name = format!("{prefix}.journal");
        let journal = native::new_file(&destination.root.directory, OsStr::new(&name), &[])?;
        Ok(Self {
            prefix,
            journal_path: destination.root.path.join(name),
            journal,
            directories: destination.dirs.clone(),
            created_dirs: Vec::new(),
            files: Vec::new(),
        })
    }

    fn write_journal(&mut self, steps: &[Step]) -> io::Result<()> {
        let entries = steps
            .iter()
            .enumerate()
            .map(|(index, step)| JournalEntry {
                relative_path: &step.path,
                before: step.before,
                after: step.after,
                backup_name: step.before.map(|_| format!("{}-{index}.bak", self.prefix)),
                temporary_name: step
                    .after
                    .filter(|record| record.contents != Contents::Directory)
                    .map(|_| format!("{}-{index}.new", self.prefix)),
            })
            .collect::<Vec<_>>();
        serde_json::to_writer(&mut self.journal, &entries).map_err(io::Error::other)?;
        self.journal.write_all(b"\n")?;
        self.journal.sync_all()
    }

    fn prepare(&mut self, source: &Snapshot<'_>, steps: &[Step]) -> io::Result<()> {
        for (index, step) in steps.iter().enumerate() {
            self.prepare_step(source, step, index).map_err(|error| {
                snapshot::contextual(error, "Prepare writeback for", &step.path)
            })?;
        }
        Ok(())
    }

    fn prepare_step(&mut self, source: &Snapshot<'_>, step: &Step, index: usize) -> io::Result<()> {
        let (parent, name) = snapshot::split(&step.path)?;
        let directory = Arc::clone(
            self.directories
                .get(parent)
                .ok_or_else(|| invalid("Missing destination directory", &step.path))?,
        );
        if step.after.is_some_and(|record| record.contents == Contents::Directory) {
            let child = native::open_directory(&directory, name, true)?;
            self.directories.insert(step.path.clone(), Arc::new(child));
            self.created_dirs.push(step.path.clone());
            return Ok(());
        }
        let previous = step
            .before
            .map(|record| open_expected(&directory, name, &step.path, record, true))
            .transpose()?;
        let next = step
            .after
            .map(|record| {
                open_expected(source.parent(&step.path)?, name, &step.path, record, false)
            })
            .transpose()?;
        let pending = PendingFile {
            path: step.path.clone(),
            name: name.to_owned(),
            parent: directory,
            backup: format!("{}-{index}.bak", self.prefix),
            previous,
            next_source: next,
            next: None,
            previous_moved: false,
        };
        self.files.push(pending);
        if let Some(pending) = self.files.last_mut() {
            pending.copy(step.after, &format!("{}-{index}.new", self.prefix))?;
        }
        Ok(())
    }

    fn publish(&mut self, steps: &[Step]) -> io::Result<Applied> {
        let mut applied = Vec::with_capacity(steps.len());
        let mut files = self.files.iter_mut();
        for step in steps {
            let record = if step.after.is_some_and(|record| record.contents == Contents::Directory)
            {
                let file = self
                    .directories
                    .get(&step.path)
                    .ok_or_else(|| invalid("Missing new directory", &step.path))?;
                Some(Record { identity: native::identity(file)?, contents: Contents::Directory })
            } else {
                let pending =
                    files.next().ok_or_else(|| invalid("Missing prepared file", &step.path))?;
                pending
                    .publish(step.after)
                    .map_err(|error| snapshot::contextual(error, "Publish", &step.path))?
            };
            applied.push((step.path.clone(), record));
        }
        Ok(applied)
    }

    fn finish(&mut self) -> io::Result<()> {
        self.journal.write_all(b"committed\n")?;
        self.journal.sync_all()?;
        for pending in &mut self.files {
            if let Some(previous) = pending.previous.take() {
                native::unlink(&previous).map_err(|error| {
                    snapshot::contextual(error, "Remove backup for", &pending.path)
                })?;
            }
        }
        native::unlink(&self.journal)
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut first_error = None;
        for mut pending in self.files.drain(..).rev() {
            if let Err(error) = pending.rollback() {
                first_error.get_or_insert(error);
            }
        }
        for path in self.created_dirs.drain(..).rev() {
            if let Some(directory) = self.directories.remove(&path) {
                if let Err(error) = native::unlink(&directory) {
                    first_error.get_or_insert_with(|| {
                        snapshot::contextual(error, "Undo new directory", &path)
                    });
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        native::unlink(&self.journal)
    }

    fn recovery_error(&self, message: &str, error: &io::Error) -> io::Error {
        io::Error::other(format!(
            "{message}. Recovery journal: {}. {error}",
            self.journal_path.display()
        ))
    }
}

struct PendingFile {
    path: PathBuf,
    name: OsString,
    parent: Arc<File>,
    backup: String,
    previous: Option<File>,
    next_source: Option<File>,
    next: Option<File>,
    previous_moved: bool,
}

impl PendingFile {
    fn copy(&mut self, after: Option<Record>, temporary_name: &str) -> io::Result<()> {
        let Some((source, after)) = self.next_source.as_mut().zip(after) else {
            return Ok(());
        };
        let security =
            self.previous.as_ref().map(native::file_security).transpose()?.unwrap_or_default();
        let file = native::new_file(&self.parent, OsStr::new(temporary_name), &security)?;
        let temporary = self.next.insert(file);
        let contents = snapshot::transfer(source, temporary)?;
        native::verify_file(source, false)?;
        if contents != after.contents {
            return Err(invalid("Source changed before writeback", &self.path));
        }
        temporary.sync_all()
    }

    fn publish(&mut self, after: Option<Record>) -> io::Result<Option<Record>> {
        if let Some(previous) = &self.previous {
            native::verify_file(previous, true)?;
            native::move_no_replace(previous, &self.parent, OsStr::new(&self.backup))?;
            self.previous_moved = true;
        }
        let Some((next, after)) = self.next.as_ref().zip(after) else {
            return Ok(None);
        };
        native::verify_file(next, false)?;
        let identity = native::identity(next)?;
        native::move_no_replace(next, &self.parent, &self.name)?;
        Ok(Some(Record { identity, contents: after.contents }))
    }

    fn rollback(&mut self) -> io::Result<()> {
        let cleanup = self.next.take().map(|next| native::unlink(&next)).transpose();
        let restore = if self.previous_moved {
            self.previous
                .as_ref()
                .ok_or_else(|| invalid("Missing rollback backup", &self.path))
                .and_then(|previous| native::move_no_replace(previous, &self.parent, &self.name))
        } else {
            Ok(())
        };
        restore
            .and(cleanup.map(|_| ()))
            .map_err(|error| snapshot::contextual(error, "Restore", &self.path))
    }
}

fn open_expected(
    parent: &File, name: &OsStr, path: &Path, record: Record, movable: bool,
) -> io::Result<File> {
    let mut file = native::read_file(parent, name, movable)?;
    snapshot::verify_identity(&file, record.identity, path)?;
    native::verify_file(&file, movable)?;
    if movable {
        let current = snapshot::transfer(&mut file, &mut io::sink())?;
        if current != record.contents {
            return Err(invalid("Writer changed the file before publication", path));
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synara::windows_workspace::native::Root;
    use std::fs;

    #[test]
    fn rollback_keeps_the_original_backup_when_a_writer_claims_the_path() {
        let scratch = tempfile::tempdir().unwrap();
        let original_path = scratch.path().join("writer");
        let stage_path = scratch.path().join("stage");
        fs::create_dir(&original_path).unwrap();
        fs::create_dir(&stage_path).unwrap();
        fs::write(original_path.join("paper.tex"), "before").unwrap();
        let original = Root::open(&original_path).unwrap();
        let stage = Root::open(&stage_path).unwrap();
        let destination = Snapshot::read(&original).unwrap();
        let source = Snapshot::read(&stage).unwrap();
        let steps = snapshot::plan(&destination.entries, &source, &destination, true).unwrap();
        let mut batch = Batch::new(&destination).unwrap();
        batch.write_journal(&steps).unwrap();
        batch.prepare(&source, &steps).unwrap();
        batch.publish(&steps).unwrap();
        fs::write(original_path.join("paper.tex"), "writer edit").unwrap();
        assert!(batch.rollback().is_err());
        let backup = original_path.join(format!("{}-0.bak", batch.prefix));
        let journal = batch.journal_path.clone();
        drop(batch);
        assert_eq!("writer edit", fs::read_to_string(original_path.join("paper.tex")).unwrap());
        assert_eq!("before", fs::read_to_string(backup).unwrap());
        assert!(journal.is_file());
    }
}
