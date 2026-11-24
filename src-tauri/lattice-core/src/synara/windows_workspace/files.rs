use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::native::{self, Identity, Root};
use super::snapshot::{self, Contents, Record, Snapshot};
use super::{invalid, RECOVERY_PREFIX};

pub(super) fn selected_file(
    parent: &File, name: &OsStr, movable: bool,
) -> io::Result<Option<File>> {
    let Some(entry) = selected_child(parent, name)? else {
        return Ok(None);
    };
    let file = native::read_file(parent, OsStr::new(&entry.name), movable)?;
    native::verify_file(&file, movable)?;
    if native::identity(&file)?.index != entry.index {
        return Err(invalid("Workspace file changed during inspection", Path::new(name)));
    }
    Ok(Some(file))
}

pub(super) fn selected_directory(
    parent: &File, name: &OsStr,
) -> io::Result<Option<(File, Identity)>> {
    let Some(entry) = selected_child(parent, name)? else {
        return Ok(None);
    };
    let file = native::open_directory(parent, OsStr::new(&entry.name), false)?;
    let identity = native::identity(&file)?;
    if identity.index != entry.index {
        return Err(invalid("Workspace directory changed during inspection", Path::new(name)));
    }
    Ok(Some((file, identity)))
}

fn selected_child(parent: &File, name: &OsStr) -> io::Result<Option<native::Child>> {
    let children = native::children(parent)?;
    if let Some(entry) =
        children.iter().find(|entry| entry.name.to_ascii_lowercase().starts_with(RECOVERY_PREFIX))
    {
        return Err(invalid("Unfinished workspace transaction", Path::new(&entry.name)));
    }
    Ok(children.into_iter().find(|entry| name.eq_ignore_ascii_case(OsStr::new(&entry.name))))
}

pub(super) fn stage_file(root: &Root, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (parent, name) = snapshot::split(path)?;
    let mut directory = Arc::clone(&root.directory);
    for part in parent {
        let child = match native::open_directory(&directory, part, false) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                native::open_directory(&directory, part, true)?
            }
            result => result?,
        };
        directory = Arc::new(child);
    }
    native::new_file(&directory, name, &[])?.write_all(bytes)
}

#[derive(Clone, Copy)]
pub(super) enum Selection {
    BuildArtifacts,
    ExistingManuscript { max_bytes: u64 },
}

pub(super) fn destination_snapshot<'root, 'paths>(
    root: &'root Root, paths: impl IntoIterator<Item = &'paths Path>, selection: Selection,
) -> io::Result<Snapshot<'root>> {
    let mut snapshot = Snapshot::empty(root);
    'file: for path in paths {
        let (parent, name) = snapshot::split(path)?;
        let mut relative = PathBuf::new();
        let mut directory = Arc::clone(&root.directory);
        for part in parent {
            if matches!(selection, Selection::ExistingManuscript { .. }) {
                require_exact_name(&directory, part)?;
            }
            relative.push(part);
            if let Some(open) = snapshot.dirs.get(&relative) {
                directory = Arc::clone(open);
                continue;
            }
            let Some((file, identity)) = selected_directory(&directory, part)? else {
                continue 'file;
            };
            directory = Arc::new(file);
            snapshot.dirs.insert(relative.clone(), Arc::clone(&directory));
            snapshot
                .entries
                .insert(relative.clone(), Record { identity, contents: Contents::Directory });
        }
        if let Some(mut file) = selected_file(&directory, name, false)? {
            if let Selection::ExistingManuscript { max_bytes } = selection {
                require_exact_name(&directory, name)?;
                if file.metadata()?.len() > max_bytes {
                    return Err(invalid("Workspace file exceeds its size limit", path));
                }
            }
            let identity = native::identity(&file)?;
            let contents = snapshot::transfer(&mut file, &mut io::sink())?;
            native::verify_file(&file, false)?;
            snapshot.entries.insert(path.to_owned(), Record { identity, contents });
        }
    }
    Ok(snapshot)
}

fn require_exact_name(parent: &File, name: &OsStr) -> io::Result<()> {
    if selected_child(parent, name)?.is_some_and(|entry| OsStr::new(&entry.name) != name) {
        return Err(invalid("Use the existing spelling of the workspace path", Path::new(name)));
    }
    Ok(())
}
