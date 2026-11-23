use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

pub(super) use super::checkpoint_git::{
    has_checkpoint_root, initialize_checkpoint_root, validate_checkpoint_root, CHECKPOINT_CONFIG,
};
use super::native::{self, Identity, Root};
use super::{invalid, permitted_name, RECOVERY_PREFIX};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) enum Contents {
    Directory,
    File([u8; 32]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct Record {
    pub(super) identity: Identity,
    pub(super) contents: Contents,
}

pub(super) type Entries = BTreeMap<PathBuf, Record>;

pub(super) struct Snapshot<'a> {
    pub(super) root: &'a Root,
    pub(super) entries: Entries,
    pub(super) dirs: BTreeMap<PathBuf, Arc<File>>,
}

impl<'a> Snapshot<'a> {
    pub(super) fn read(root: &'a Root) -> io::Result<Self> {
        let mut snapshot = Self {
            root,
            entries: BTreeMap::new(),
            dirs: BTreeMap::from([(PathBuf::new(), Arc::clone(&root.directory))]),
        };
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let directory = Arc::clone(&snapshot.dirs[&relative]);
            snapshot
                .read_dir(&directory, &relative, &mut pending)
                .map_err(|error| contextual(error, "Scan", &root.path.join(&relative)))?;
        }
        Ok(snapshot)
    }

    fn read_dir(
        &mut self, directory: &File, relative: &Path, pending: &mut Vec<PathBuf>,
    ) -> io::Result<()> {
        let mut names = BTreeSet::new();
        let volume = native::identity(directory)?.volume;
        for child in native::children(directory)? {
            let name = OsStr::new(&child.name);
            let folded = fold_name(&child.name);
            if relative.as_os_str().is_empty() && child.name.eq_ignore_ascii_case(".git") {
                continue;
            }
            let path = relative.join(name);
            if !permitted_name(name) || !names.insert(folded) {
                return Err(invalid("Ambiguous workspace name", &path));
            }
            if child.name.to_ascii_lowercase().starts_with(RECOVERY_PREFIX) {
                return Err(invalid("Unfinished workspace transaction", &path));
            }
            let identity = Identity { volume, index: child.index };
            let contents = if child.directory {
                let file = native::open_directory(directory, name, false)?;
                verify_identity(&file, identity, &path)?;
                self.dirs.insert(path.clone(), Arc::new(file));
                pending.push(path.clone());
                Contents::Directory
            } else {
                let mut file = native::read_file(directory, name, false)?;
                verify_identity(&file, identity, &path)?;
                native::verify_file(&file, false)?;
                let contents = transfer(&mut file, &mut io::sink())?;
                native::verify_file(&file, false)?;
                contents
            };
            self.entries.insert(path, Record { identity, contents });
        }
        Ok(())
    }

    pub(super) fn parent(&self, path: &Path) -> io::Result<&File> {
        let (parent, _) = split(path)?;
        self.dirs
            .get(parent)
            .map(Arc::as_ref)
            .ok_or_else(|| invalid("Missing workspace ancestor", path))
    }
}

pub(super) fn split(path: &Path) -> io::Result<(&Path, &OsStr)> {
    path.parent()
        .zip(path.file_name())
        .ok_or_else(|| invalid("Expected a relative workspace entry", path))
}

pub(super) fn contextual(error: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{action} {}: {error}", path.display()))
}

pub(super) fn same_contents(before: &Entries, after: &Entries) -> bool {
    before.len() == after.len()
        && before.iter().all(|(path, record)| {
            after.get(path).is_some_and(|other| record.contents == other.contents)
        })
}

pub(super) fn verify_identity(file: &File, id: Identity, path: &Path) -> io::Result<()> {
    if native::identity(file)? != id {
        return Err(invalid("Workspace entry changed identity during operation", path));
    }
    Ok(())
}

pub(super) fn transfer(source: &mut File, output: &mut impl Write) -> io::Result<Contents> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match source.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            return Ok(Contents::File(hash.finalize().into()));
        }
        let bytes = &buffer[..read];
        output.write_all(bytes)?;
        hash.update(bytes);
    }
}

#[derive(Clone, Debug)]
pub(super) struct Step {
    pub(super) path: PathBuf,
    pub(super) before: Option<Record>,
    pub(super) after: Option<Record>,
}

pub(super) fn plan(
    baseline: &Entries, source: &Snapshot<'_>, destination: &Snapshot<'_>, protect: bool,
) -> io::Result<Vec<Step>> {
    let entries = &destination.entries;
    let paths: BTreeSet<_> = baseline.keys().chain(source.entries.keys()).collect();
    let mut steps = Vec::new();
    for path in paths {
        let before = baseline.get(path).copied();
        let after = source.entries.get(path).copied();
        if before.map(|entry| entry.contents) == after.map(|entry| entry.contents) {
            continue;
        }
        if protect && protected_path(path) {
            return Err(invalid("Sandbox changed a protected project path", path));
        }
        if path.ancestors().any(|part| baseline.get(part) != entries.get(part)) {
            return Err(invalid("Workspace conflicts with a writer change", path));
        }
        if before.is_some_and(|record| record.contents == Contents::Directory)
            || (before.is_some()
                && after.is_some_and(|record| record.contents == Contents::Directory))
        {
            return Err(invalid(
                "Workspace directory removal or type change requires a new stage",
                path,
            ));
        }
        steps.push(Step { path: path.clone(), before, after });
    }
    Ok(steps)
}

pub(super) fn check_checkpoint_paths(steps: &[Step], paths: &BTreeSet<PathBuf>) -> io::Result<()> {
    for step in steps {
        let expected = if step.after.is_some_and(|record| record.contents == Contents::Directory) {
            paths.iter().any(|path| path.starts_with(&step.path))
        } else {
            paths.contains(&step.path)
        };
        if !expected {
            return Err(invalid("Workspace changed outside the selected checkpoint", &step.path));
        }
    }
    Ok(())
}

pub(super) fn protected_path(path: &Path) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => {
            let name = fold_name(&name.to_string_lossy());
            crate::project::is_internal_project_component(OsStr::new(&name))
                || name.starts_with(".GIT")
                || matches!(
                    name.as_str(),
                    ".LATEXMKRC" | "LATEXMKRC" | ".LATTICE" | ".ENVRC" | ".OVERLEAF" | ".PAPERS"
                )
                || name.rsplit_once('.').is_some_and(|(_, extension)| {
                    matches!(extension, "BIB" | "CDX" | "CDXML" | "MNOVA" | "MNJS" | "MNL" | "MNLT")
                })
        }
        _ => true,
    })
}

fn fold_name(name: &str) -> String {
    name.nfkc().flat_map(char::to_uppercase).collect()
}
