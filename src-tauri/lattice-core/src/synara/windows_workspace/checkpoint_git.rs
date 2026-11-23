mod objects;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::invalid;
use super::native::{self, Identity, Root};
use super::snapshot::{self, Snapshot};

pub(super) const CHECKPOINT_CONFIG: &[u8] =
    b"[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n\tlogallrefupdates = false\n\tignorecase = true\n";

type GitOid = [u8; 20];

pub(super) fn validate_checkpoint_root(root: &Root) -> io::Result<()> {
    let entry = checkpoint_root_entry(root)?
        .ok_or_else(|| invalid("Checkpoint repository is missing", &root.path))?;
    let directory = native::open_directory(&root.directory, OsStr::new(&entry.name), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint repository", &root.path))?;
    snapshot::verify_identity(
        &directory,
        Identity { volume: native::identity(&root.directory)?.volume, index: entry.index },
        Path::new(".git"),
    )?;
    let head = native::read_file(&directory, OsStr::new("HEAD"), false)
        .map_err(|error| snapshot::contextual(error, "Read checkpoint HEAD", &root.path))?;
    let mut head_text = String::new();
    head.take(256).read_to_string(&mut head_text)?;
    if head_text != "ref: refs/heads/lattice\n" {
        return Err(invalid("Checkpoint repository has an unexpected HEAD", &root.path));
    }
    let config = native::read_file(&directory, OsStr::new("config"), false)
        .map_err(|error| snapshot::contextual(error, "Read checkpoint config", &root.path))?;
    let mut config_bytes = Vec::new();
    config.take((CHECKPOINT_CONFIG.len() + 1) as u64).read_to_end(&mut config_bytes)?;
    if config_bytes != CHECKPOINT_CONFIG {
        return Err(invalid("Checkpoint repository config was modified", &root.path));
    }
    let objects = native::open_directory(&directory, OsStr::new("objects"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint objects", &root.path))?;
    let info = native::open_directory(&objects, OsStr::new("info"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint objects/info", &root.path))?;
    if native::children(&info)?.iter().any(|entry| {
        ["alternates", "http-alternates"].iter().any(|name| entry.name.eq_ignore_ascii_case(name))
    }) {
        return Err(invalid("Checkpoint repository cannot use external object stores", &root.path));
    }
    native::open_directory(&objects, OsStr::new("pack"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint objects/pack", &root.path))?;
    let refs = native::open_directory(&directory, OsStr::new("refs"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint refs", &root.path))?;
    validate_checkpoint_refs(&refs, &directory, &root.path)?;
    validate_checkpoint_tree(&directory, &root.path)
}

fn validate_checkpoint_refs(refs: &File, git: &File, root: &Path) -> io::Result<()> {
    let mut pending = vec![(refs.try_clone()?, PathBuf::from("refs"))];
    let mut branch_found = false;
    while let Some((directory, relative)) = pending.pop() {
        for entry in native::children(&directory)? {
            let path = relative.join(&entry.name);
            let name = path.to_string_lossy().replace('\\', "/");
            if entry.directory {
                let checkpoint_namespace = name
                    .strip_prefix("refs/lattice/checkpoints/")
                    .is_some_and(valid_checkpoint_ref_path);
                if !matches!(
                    name.as_str(),
                    "refs/heads" | "refs/lattice" | "refs/lattice/checkpoints"
                ) && !checkpoint_namespace
                {
                    return Err(invalid(
                        "Checkpoint repository contains an unexpected ref namespace",
                        &root.join(path),
                    ));
                }
                let child = native::open_directory(&directory, OsStr::new(&entry.name), false)?;
                snapshot::verify_identity(
                    &child,
                    Identity { volume: native::identity(&directory)?.volume, index: entry.index },
                    &path,
                )?;
                pending.push((child, path));
                continue;
            }
            if !supported_checkpoint_ref(&name) {
                return Err(invalid(
                    "Checkpoint repository contains an unexpected ref",
                    &root.join(path),
                ));
            }
            let file = native::read_file(&directory, OsStr::new(&entry.name), false)?;
            snapshot::verify_identity(
                &file,
                Identity { volume: native::identity(&directory)?.volume, index: entry.index },
                &path,
            )?;
            native::verify_file(&file, false)?;
            let mut oid = String::new();
            file.take(42).read_to_string(&mut oid)?;
            if !valid_loose_oid(&oid) {
                return Err(invalid(
                    "Checkpoint repository contains an invalid ref",
                    &root.join(path),
                ));
            }
            branch_found |= name == "refs/heads/lattice";
        }
    }
    branch_found |= validate_packed_refs(git, root)?;
    if !branch_found {
        return Err(invalid("Checkpoint repository branch is missing", root));
    }
    Ok(())
}

fn validate_packed_refs(git: &File, root: &Path) -> io::Result<bool> {
    let Some(entry) = native::children(git)?
        .into_iter()
        .find(|entry| entry.name.eq_ignore_ascii_case("packed-refs"))
    else {
        return Ok(false);
    };
    if entry.directory {
        return Err(invalid("Checkpoint packed refs are not a file", root));
    }
    let file = native::read_file(git, OsStr::new(&entry.name), false)?;
    snapshot::verify_identity(
        &file,
        Identity { volume: native::identity(git)?.volume, index: entry.index },
        Path::new(".git/packed-refs"),
    )?;
    native::verify_file(&file, false)?;
    let mut contents = String::new();
    file.take(16 * 1024 * 1024 + 1).read_to_string(&mut contents)?;
    if contents.len() > 16 * 1024 * 1024 {
        return Err(invalid("Checkpoint packed refs exceed the size limit", root));
    }
    let mut branch_found = false;
    let mut seen = HashSet::new();
    for line in contents.lines().filter(|line| !line.starts_with('#')) {
        let Some((oid, name)) = line.split_once(' ') else {
            return Err(invalid("Checkpoint packed refs are malformed", root));
        };
        if oid.len() != 40
            || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !supported_checkpoint_ref(name)
            || !seen.insert(name)
        {
            return Err(invalid("Checkpoint packed refs contain an unsupported ref", root));
        }
        branch_found |= name == "refs/heads/lattice";
    }
    Ok(branch_found)
}

fn supported_checkpoint_ref(name: &str) -> bool {
    name == "refs/heads/lattice"
        || name.strip_prefix("refs/lattice/checkpoints/").is_some_and(valid_checkpoint_ref_path)
}

fn valid_checkpoint_ref_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(".lock")
                && !part.contains("..")
                && !part.contains("@{")
                && part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'@')
                })
        })
}

fn valid_loose_oid(oid: &str) -> bool {
    oid.len() == 41 && oid.ends_with('\n') && oid.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit)
}

fn validate_checkpoint_tree(root: &File, path: &Path) -> io::Result<()> {
    let mut pending = vec![(root.try_clone()?, PathBuf::from(".git"))];
    while let Some((directory, relative)) = pending.pop() {
        let volume = native::identity(&directory)?.volume;
        for entry in native::children(&directory)? {
            let entry_path = relative.join(&entry.name);
            if entry.directory {
                let child = native::open_directory(&directory, OsStr::new(&entry.name), false)?;
                snapshot::verify_identity(
                    &child,
                    Identity { volume, index: entry.index },
                    &entry_path,
                )?;
                pending.push((child, entry_path));
            } else {
                let file = native::read_file(&directory, OsStr::new(&entry.name), false)?;
                snapshot::verify_identity(
                    &file,
                    Identity { volume, index: entry.index },
                    &entry_path,
                )?;
                native::verify_file(&file, false).map_err(|error| {
                    snapshot::contextual(
                        error,
                        "Verify checkpoint metadata",
                        &path.join(entry_path),
                    )
                })?;
            }
        }
    }
    Ok(())
}

pub(super) fn has_checkpoint_root(root: &Root) -> io::Result<bool> {
    Ok(checkpoint_root_entry(root)?.is_some())
}

fn checkpoint_root_entry(root: &Root) -> io::Result<Option<native::Child>> {
    Ok(native::children(&root.directory)?
        .into_iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(".git")))
}

pub(super) fn initialize_checkpoint_root(root: &Root) -> io::Result<()> {
    {
        let directory =
            native::open_directory(&root.directory, OsStr::new(".git"), true).map_err(|error| {
                snapshot::contextual(error, "Create checkpoint directory", &root.path.join(".git"))
            })?;
        let objects =
            native::open_directory(&directory, OsStr::new("objects"), true).map_err(|error| {
                snapshot::contextual(
                    error,
                    "Create checkpoint directory",
                    &root.path.join(".git/objects"),
                )
            })?;
        native::open_directory(&objects, OsStr::new("info"), true).map_err(|error| {
            snapshot::contextual(
                error,
                "Create checkpoint directory",
                &root.path.join(".git/objects/info"),
            )
        })?;
        native::open_directory(&objects, OsStr::new("pack"), true).map_err(|error| {
            snapshot::contextual(
                error,
                "Create checkpoint directory",
                &root.path.join(".git/objects/pack"),
            )
        })?;
        let refs =
            native::open_directory(&directory, OsStr::new("refs"), true).map_err(|error| {
                snapshot::contextual(
                    error,
                    "Create checkpoint directory",
                    &root.path.join(".git/refs"),
                )
            })?;
        native::open_directory(&refs, OsStr::new("heads"), true).map_err(|error| {
            snapshot::contextual(
                error,
                "Create checkpoint directory",
                &root.path.join(".git/refs/heads"),
            )
        })?;
        for (name, value) in
            [("HEAD", b"ref: refs/heads/lattice\n".as_slice()), ("config", CHECKPOINT_CONFIG)]
        {
            let path = root.path.join(".git").join(name);
            let mut file = native::new_file(&directory, OsStr::new(name), &[])
                .map_err(|error| snapshot::contextual(error, "Create checkpoint file", &path))?;
            file.write_all(value)
                .map_err(|error| snapshot::contextual(error, "Write checkpoint file", &path))?;
            file.sync_all()
                .map_err(|error| snapshot::contextual(error, "Sync checkpoint file", &path))?;
        }
    }
    let workspace = Snapshot::read(root)
        .map_err(|error| snapshot::contextual(error, "Scan checkpoint baseline", &root.path))?;
    let git = native::open_directory(&root.directory, OsStr::new(".git"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint repository", &root.path))?;
    let objects = native::open_directory(&git, OsStr::new("objects"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint objects", &root.path))?;
    let baseline = objects::write_baseline_commit(&workspace, &objects)
        .map_err(|error| snapshot::contextual(error, "Write checkpoint baseline", &root.path))?;
    let refs = native::open_directory(&git, OsStr::new("refs"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint refs", &root.path))?;
    let heads = native::open_directory(&refs, OsStr::new("heads"), false)
        .map_err(|error| snapshot::contextual(error, "Open checkpoint branches", &root.path))?;
    let mut reference = native::new_file(&heads, OsStr::new("lattice"), &[])
        .map_err(|error| snapshot::contextual(error, "Create checkpoint branch", &root.path))?;
    writeln!(reference, "{}", objects::encode_oid(&baseline))
        .map_err(|error| snapshot::contextual(error, "Write checkpoint branch", &root.path))?;
    reference
        .sync_all()
        .map_err(|error| snapshot::contextual(error, "Sync checkpoint branch", &root.path))?;
    drop(reference);
    validate_checkpoint_root(root)
        .map_err(|error| snapshot::contextual(error, "Validate checkpoint repository", &root.path))
}
