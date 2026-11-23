use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::write::ZlibEncoder;
use flate2::Compression;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::super::invalid;
use super::super::native;
use super::super::snapshot::{self, Contents, Snapshot};
use super::GitOid;

pub(super) fn write_baseline_commit(
    workspace: &Snapshot<'_>, objects: &File,
) -> io::Result<GitOid> {
    let mut cache = HashSet::new();
    let mut blobs = HashMap::new();
    for (path, record) in &workspace.entries {
        if record.contents != Contents::Directory {
            blobs.insert(path.clone(), write_blob(workspace, objects, path, &mut cache)?);
        }
    }

    let mut children = HashMap::<PathBuf, Vec<PathBuf>>::new();
    for path in workspace.entries.keys() {
        let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
        children.entry(parent).or_default().push(path.clone());
    }
    let mut directories = workspace.dirs.keys().cloned().collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut trees = HashMap::<PathBuf, GitOid>::new();
    for directory in directories {
        let mut entries = children
            .remove(&directory)
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                let (_, name) = snapshot::split(&path)?;
                let is_directory = workspace.entries[&path].contents == Contents::Directory;
                let oid = if is_directory {
                    *trees.get(&path).ok_or_else(|| invalid("Checkpoint tree is missing", &path))?
                } else {
                    *blobs.get(&path).ok_or_else(|| invalid("Checkpoint blob is missing", &path))?
                };
                Ok((
                    name.to_str()
                        .ok_or_else(|| invalid("Checkpoint path is not Unicode", &path))?
                        .as_bytes()
                        .to_vec(),
                    is_directory,
                    oid,
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort_by(|left, right| git_tree_name_cmp(&left.0, left.1, &right.0, right.1));
        let mut body = Vec::new();
        for (name, is_directory, oid) in entries {
            body.extend_from_slice(if is_directory { b"40000 " } else { b"100644 " });
            body.extend_from_slice(&name);
            body.push(0);
            body.extend_from_slice(&oid);
        }
        let mut source = &body[..];
        let oid = write_object(objects, "tree", &mut source, body.len() as u64, &mut cache)
            .map_err(|error| snapshot::contextual(error, "Write checkpoint tree", &directory))?;
        trees.insert(directory, oid);
    }
    let tree = trees
        .get(Path::new(""))
        .ok_or_else(|| invalid("Checkpoint root tree is missing", &workspace.root.path))?;
    let body = format!(
        "tree {}\nauthor Lattice <lattice@users.noreply.github.com> 0 +0000\ncommitter Lattice <lattice@users.noreply.github.com> 0 +0000\n\nLattice isolated workspace baseline\n",
        encode_oid(tree),
    );
    let mut source = body.as_bytes();
    write_object(objects, "commit", &mut source, body.len() as u64, &mut cache).map_err(|error| {
        snapshot::contextual(error, "Write checkpoint commit", &workspace.root.path)
    })
}

fn write_blob(
    workspace: &Snapshot<'_>, objects: &File, path: &Path, cache: &mut HashSet<GitOid>,
) -> io::Result<GitOid> {
    let (parent, name) = snapshot::split(path)?;
    let directory = workspace
        .dirs
        .get(parent)
        .ok_or_else(|| invalid("Missing checkpoint source directory", path))?;
    let mut file = native::read_file(directory, name, false)?;
    let expected = workspace
        .entries
        .get(path)
        .ok_or_else(|| invalid("Missing checkpoint source entry", path))?;
    snapshot::verify_identity(&file, expected.identity, path)?;
    native::verify_file(&file, false)?;
    let size = file.metadata()?.len();
    let (oid, contents) = {
        let mut source = HashedFile { file: &mut file, hash: Sha256::new() };
        let oid = write_object(objects, "blob", &mut source, size, cache)
            .map_err(|error| snapshot::contextual(error, "Write checkpoint blob", path))?;
        (oid, source.hash.finalize().into())
    };
    if expected.contents != Contents::File(contents) {
        return Err(invalid("Checkpoint source changed while initializing Git", path));
    }
    snapshot::verify_identity(&file, expected.identity, path)?;
    native::verify_file(&file, false)?;
    Ok(oid)
}

struct HashedFile<'a> {
    file: &'a mut File,
    hash: Sha256,
}

impl Read for HashedFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.file.read(buffer)?;
        self.hash.update(&buffer[..read]);
        Ok(read)
    }
}

fn write_object(
    objects: &File, kind: &str, source: &mut impl Read, size: u64, cache: &mut HashSet<GitOid>,
) -> io::Result<GitOid> {
    let header = format!("{kind} {size}\0");
    let temporary_name = format!("tmp-{}", uuid::Uuid::new_v4());
    let mut temporary =
        native::new_file(objects, OsStr::new(&temporary_name), &[]).map_err(|error| {
            snapshot::contextual(error, "Create checkpoint object", Path::new(&temporary_name))
        })?;
    let mut hash = Sha1::new();
    let oid;
    {
        let mut encoder = ZlibEncoder::new(&mut temporary, Compression::default());
        hash.update(header.as_bytes());
        encoder.write_all(header.as_bytes())?;
        let mut remaining = source.take(size);
        let mut buffer = [0u8; 64 * 1024];
        let mut copied = 0;
        loop {
            let read = match remaining.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if read == 0 {
                break;
            }
            let bytes = &buffer[..read];
            encoder.write_all(bytes)?;
            hash.update(bytes);
            copied += read as u64;
        }
        if copied != size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Checkpoint blob changed while hashing",
            ));
        }
        oid = hash.finalize().into();
        encoder.finish()?;
    }
    temporary.sync_all().map_err(|error| {
        snapshot::contextual(error, "Sync checkpoint object", Path::new(&temporary_name))
    })?;
    let encoded = encode_oid(&oid);
    let folder = open_object_directory(objects, OsStr::new(&encoded[..2])).map_err(|error| {
        snapshot::contextual(error, "Open checkpoint object directory", Path::new(&encoded[..2]))
    })?;
    let name = OsStr::new(&encoded[2..]);
    if cache.insert(oid) {
        native::move_no_replace(&temporary, &folder, name).map_err(|error| {
            snapshot::contextual(error, "Publish checkpoint object", Path::new(&encoded))
        })?;
    } else {
        native::unlink(&temporary)?;
    }
    Ok(oid)
}

fn open_object_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    match native::open_directory(parent, name, false) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match native::open_directory(parent, name, true) {
                Ok(directory) => Ok(directory),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    native::open_directory(parent, name, false)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn encode_oid(oid: &GitOid) -> String {
    let mut encoded = String::with_capacity(40);
    for byte in oid {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn git_tree_name_cmp(
    left: &[u8], left_dir: bool, right: &[u8], right_dir: bool,
) -> std::cmp::Ordering {
    left.iter()
        .copied()
        .chain([if left_dir { b'/' } else { 0 }])
        .cmp(right.iter().copied().chain([if right_dir { b'/' } else { 0 }]))
}
