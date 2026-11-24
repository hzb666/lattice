use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::native::{self, Root};
use super::snapshot::{self, Contents, Snapshot, Step};
use super::{files, invalid, permitted_name, transaction, Workspace};

pub(super) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FRAGMENT_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectTextEdit {
    #[serde(default)]
    pub(super) thread_id: String,
    #[serde(default)]
    pub(super) turn_id: String,
    path: String,
    old_text: String,
    new_text: String,
    expires_at: u64,
}

impl Workspace {
    #[cfg(test)]
    pub(super) fn edit_text(
        &mut self, control: &Path, edit: &ProjectTextEdit,
    ) -> Result<(), String> {
        self.edit_text_inner(control, edit).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn edit_text_inner(&mut self, control: &Path, edit: &ProjectTextEdit) -> io::Result<()> {
        self.check_ready()?;
        let path = edit.checked_path()?;
        let destination = text_snapshot(&self.staging, [path.as_path()])?;
        let updated = edit.replacement(&read_text(&destination, &path)?, &path)?;
        edit.check_deadline()?;
        publish(
            control,
            &destination,
            &BTreeMap::from([(path, updated)]),
            &mut self.recovery_required,
        )
    }
}

pub(super) fn text_snapshot<'root, 'paths>(
    root: &'root Root, paths: impl IntoIterator<Item = &'paths Path>,
) -> io::Result<Snapshot<'root>> {
    files::destination_snapshot(
        root,
        paths,
        files::Selection::ExistingManuscript { max_bytes: MAX_FILE_BYTES },
    )
}

pub(super) fn read_text(destination: &Snapshot<'_>, path: &Path) -> io::Result<String> {
    let before = destination
        .entries
        .get(path)
        .ok_or_else(|| invalid("The manuscript must already exist in the project", path))?;
    let (_, name) = snapshot::split(path)?;
    let file = native::read_file(destination.parent(path)?, name, false)?;
    snapshot::verify_identity(&file, before.identity, path)?;
    native::verify_file(&file, false)?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_FILE_BYTES
        || before.contents != Contents::File(Sha256::digest(text.as_bytes()).into())
    {
        return Err(invalid("The manuscript changed or exceeds the edit size limit", path));
    }
    Ok(text)
}

pub(super) fn publish(
    control: &Path, destination: &Snapshot<'_>, updates: &BTreeMap<PathBuf, String>,
    recovery_required: &mut bool,
) -> io::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let temporary = tempfile::tempdir_in(control)?;
    let patch = Root::open(temporary.path())?;
    for (path, bytes) in updates {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(invalid("The edited manuscript exceeds the size limit", path));
        }
        files::stage_file(&patch, path, bytes.as_bytes())?;
    }
    let source = Snapshot::read(&patch)?;
    let steps =
        updates
            .keys()
            .map(|path| {
                let before = destination.entries.get(path).copied().ok_or_else(|| {
                    invalid("The manuscript must already exist in the project", path)
                })?;
                let after = source.entries.get(path).copied().ok_or_else(|| {
                    invalid("The manuscript replacement could not be staged", path)
                })?;
                Ok(Step { path: path.clone(), before: Some(before), after: Some(after) })
            })
            .collect::<io::Result<Vec<_>>>()?;
    transaction::run(&source, destination, steps, recovery_required)?;
    Ok(())
}

pub(super) fn checked_path(value: &str) -> io::Result<PathBuf> {
    let parts = value.split('/');
    let path: PathBuf = parts.clone().collect();
    if value.len() > 1_024
        || parts
            .into_iter()
            .any(|part| matches!(part, "" | "." | "..") || !permitted_name(part.as_ref()))
        || snapshot::protected_path(&path)
        || !snapshot::manuscript(&path)
    {
        return Err(invalid("Only existing project manuscript files can be edited", &path));
    }
    Ok(path)
}

pub(super) fn check_deadline(expires_at: u64) -> io::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    if u128::from(expires_at) <= now.as_millis() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "The manuscript request expired."));
    }
    Ok(())
}

impl ProjectTextEdit {
    pub(super) fn checked_path(&self) -> io::Result<PathBuf> {
        self.check_deadline()?;
        let path = checked_path(&self.path)?;
        if self.old_text.is_empty()
            || self.old_text.len() > MAX_FRAGMENT_BYTES
            || self.new_text.len() > MAX_FRAGMENT_BYTES
            || self.old_text == self.new_text
            || self.old_text.contains('\0')
            || self.new_text.contains('\0')
        {
            return Err(invalid("Invalid manuscript replacement", &path));
        }
        Ok(path)
    }

    pub(super) fn check_deadline(&self) -> io::Result<()> {
        check_deadline(self.expires_at)
    }

    pub(super) fn replacement(&self, original: &str, path: &Path) -> io::Result<String> {
        let mut text = original.to_owned();
        let Some(offset) = text.find(&self.old_text) else {
            return Err(invalid("The quoted manuscript text changed or was not found", path));
        };
        let first_char_end = offset + self.old_text.chars().next().map_or(0, char::len_utf8);
        if text[first_char_end..].contains(&self.old_text) {
            return Err(invalid(
                "Quote enough manuscript text to identify a single occurrence",
                path,
            ));
        }
        text.replace_range(offset..offset + self.old_text.len(), &self.new_text);
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(invalid("The edited manuscript exceeds the size limit", path));
        }
        Ok(text)
    }
}
