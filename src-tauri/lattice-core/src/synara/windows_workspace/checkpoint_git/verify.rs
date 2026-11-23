use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::super::snapshot::{Contents, Snapshot};
use super::super::{invalid, native, permitted_name, snapshot};
use super::objects::encode_oid;
use super::{read_packed_refs, supported_checkpoint_ref, valid_loose_oid, GitOid};

const METADATA_LIMIT: u64 = 16 * 1024 * 1024;

pub(in crate::synara::windows_workspace) fn verify_checkpoint_contents(
    stage: &Snapshot<'_>, targets: &BTreeMap<PathBuf, String>,
) -> io::Result<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut reader = Checkpoints::open(stage)?;
    for (path, reference) in targets {
        let expected = reader.contents(reference, path, stage)?;
        if expected != stage.entries.get(path).map(|entry| entry.contents) {
            return Err(invalid("Staged contents differ from the Git checkpoint", path));
        }
    }
    Ok(())
}

struct Checkpoints {
    git: File,
    objects: File,
    refs: HashMap<String, GitOid>,
    packed_refs: Option<BTreeMap<String, String>>,
    trees: HashMap<GitOid, BTreeMap<String, TreeEntry>>,
}

#[derive(Clone, Copy)]
struct TreeEntry {
    mode: u32,
    oid: GitOid,
}

impl Checkpoints {
    fn open(stage: &Snapshot<'_>) -> io::Result<Self> {
        let git = native::open_directory(&stage.root.directory, OsStr::new(".git"), false)?;
        let objects = native::open_directory(&git, OsStr::new("objects"), false)?;
        Ok(Self { git, objects, refs: HashMap::new(), packed_refs: None, trees: HashMap::new() })
    }

    fn contents(
        &mut self, reference: &str, path: &Path, stage: &Snapshot<'_>,
    ) -> io::Result<Option<Contents>> {
        let root_tree = self.root_tree(reference)?;
        let Some(entry) = self.lookup(root_tree, path)? else { return Ok(None) };
        if !matches!(entry.mode, 0o100644 | 0o100755) {
            return Err(invalid("Checkpoint path is not a regular file", path));
        }
        let Some(record) = stage.entries.get(path) else {
            return Err(invalid("Staged file is missing from the checkpoint path", path));
        };
        let (_, name) = snapshot::split(path)?;
        let current = native::read_file(stage.parent(path)?, name, false)?;
        snapshot::verify_identity(&current, record.identity, path)?;
        let size = current.metadata()?.len();
        let mut object = self.object(entry.oid, "blob", size)?;
        if object.size != size {
            return Err(invalid("Staged file size differs from the Git checkpoint", path));
        }
        let digest = object.blob_digest()?;
        Ok(Some(Contents::File(digest)))
    }

    fn root_tree(&mut self, reference: &str) -> io::Result<GitOid> {
        if let Some(&tree) = self.refs.get(reference) {
            return Ok(tree);
        }
        if !reference.starts_with("refs/lattice/checkpoints/")
            || !supported_checkpoint_ref(reference)
        {
            return Err(invalid("Unsupported checkpoint ref", Path::new(reference)));
        }
        let commit = match loose_ref(&self.git, reference) {
            Ok(commit) => commit,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if self.packed_refs.is_none() {
                    self.packed_refs = Some(read_packed_refs(&self.git, Path::new(".git"))?);
                }
                self.packed_refs
                    .as_ref()
                    .and_then(|refs| refs.get(reference))
                    .and_then(|oid| parse_oid(oid))
                    .ok_or_else(|| invalid("Checkpoint ref is missing", Path::new(reference)))?
            }
            Err(error) => return Err(error),
        };
        let body = self.object(commit, "commit", METADATA_LIMIT)?.metadata()?;
        let first = body.split(|&byte| byte == b'\n').next().unwrap_or_default();
        let tree = first
            .strip_prefix(b"tree ")
            .and_then(|oid| std::str::from_utf8(oid).ok())
            .and_then(parse_oid)
            .ok_or_else(|| invalid("Invalid checkpoint commit tree", Path::new(reference)))?;
        self.refs.insert(reference.to_owned(), tree);
        Ok(tree)
    }

    fn lookup(&mut self, mut tree: GitOid, path: &Path) -> io::Result<Option<TreeEntry>> {
        let mut parts = path.components().peekable();
        while let Some(part) = parts.next() {
            let std::path::Component::Normal(name) = part else {
                return Err(invalid("Expected a project-relative checkpoint path", path));
            };
            let name = name
                .to_str()
                .filter(|_| permitted_name(name))
                .ok_or_else(|| invalid("Invalid checkpoint path name", path))?;
            let Some(entry) = self.tree(tree)?.get(name).copied() else {
                return Ok(None);
            };
            if parts.peek().is_none() {
                return Ok(Some(entry));
            }
            if entry.mode != 0o40000 {
                return Err(invalid("Checkpoint path ancestor is not a directory", path));
            }
            tree = entry.oid;
        }
        Err(invalid("Missing checkpoint path", path))
    }

    fn tree(&mut self, oid: GitOid) -> io::Result<&BTreeMap<String, TreeEntry>> {
        if !self.trees.contains_key(&oid) {
            let bytes = self.object(oid, "tree", METADATA_LIMIT)?.metadata()?;
            self.trees.insert(oid, parse_tree(&bytes)?);
        }
        Ok(&self.trees[&oid])
    }

    fn object(&self, oid: GitOid, kind: &str, limit: u64) -> io::Result<Object> {
        let hex = encode_oid(&oid);
        let file = native::open_directory(&self.objects, OsStr::new(&hex[..2]), false)
            .and_then(|folder| native::read_file(&folder, OsStr::new(&hex[2..]), false))
            .map_err(|error| {
                snapshot::contextual(error, "Read loose checkpoint object", Path::new(&hex))
            })?;
        native::verify_file(&file, false)?;
        Object::open(file, oid, kind, limit)
    }
}

fn loose_ref(git: &File, reference: &str) -> io::Result<GitOid> {
    let path = Path::new(reference);
    let (parent, name) = snapshot::split(path)?;
    let mut folder = git.try_clone()?;
    for part in parent {
        folder = native::open_directory(&folder, part, false)?;
    }
    let file = native::read_file(&folder, name, false)?;
    native::verify_file(&file, false)?;
    let mut text = String::new();
    file.take(42).read_to_string(&mut text)?;
    if !valid_loose_oid(&text) {
        return Err(invalid("Invalid loose checkpoint ref", path));
    }
    parse_oid(&text[..40]).ok_or_else(|| invalid("Invalid checkpoint object ID", path))
}

fn parse_oid(text: &str) -> Option<GitOid> {
    if text.len() != 40 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut oid = [0; 20];
    for (index, byte) in oid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(oid)
}

fn parse_tree(mut bytes: &[u8]) -> io::Result<BTreeMap<String, TreeEntry>> {
    let malformed = || invalid("Malformed Git checkpoint tree", Path::new(".git/objects"));
    let mut entries = BTreeMap::new();
    while !bytes.is_empty() {
        let end = bytes.iter().position(|&byte| byte == 0).ok_or_else(malformed)?;
        let header = std::str::from_utf8(&bytes[..end]).map_err(|_| malformed())?;
        let (mode, name) = header.split_once(' ').ok_or_else(malformed)?;
        let mode = u32::from_str_radix(mode, 8).map_err(|_| malformed())?;
        let oid = bytes.get(end + 1..end + 21).ok_or_else(malformed)?;
        if name.is_empty() || name.contains('/') {
            return Err(malformed());
        }
        let entry = TreeEntry { mode, oid: oid.try_into().map_err(|_| malformed())? };
        if entries.insert(name.to_owned(), entry).is_some() {
            return Err(malformed());
        }
        bytes = &bytes[end + 21..];
    }
    Ok(entries)
}

struct Object {
    body: BufReader<ZlibDecoder<File>>,
    size: u64,
    hash: Sha1,
    oid: GitOid,
}

impl Object {
    fn open(file: File, oid: GitOid, kind: &str, limit: u64) -> io::Result<Self> {
        let mut body = BufReader::new(ZlibDecoder::new(file));
        let mut header = Vec::new();
        (&mut body).take(64).read_until(0, &mut header)?;
        let size = std::str::from_utf8(&header)
            .ok()
            .and_then(|header| header.strip_suffix('\0'))
            .and_then(|header| header.split_once(' '))
            .filter(|(actual, _)| *actual == kind)
            .and_then(|(_, size)| size.parse::<u64>().ok())
            .filter(|&size| size <= limit)
            .ok_or_else(|| invalid("Invalid checkpoint object type or size", Path::new(".git")))?;
        Ok(Self { body, size, hash: Sha1::new_with_prefix(header), oid })
    }

    fn metadata(self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.consume(|part| bytes.extend_from_slice(part))?;
        Ok(bytes)
    }

    fn blob_digest(self) -> io::Result<[u8; 32]> {
        let mut hash = Sha256::new();
        self.consume(|part| hash.update(part))?;
        Ok(hash.finalize().into())
    }

    fn consume(mut self, mut consume: impl FnMut(&[u8])) -> io::Result<()> {
        let mut buffer = [0; 64 * 1024];
        let mut remaining = (&mut self.body).take(self.size);
        while remaining.limit() > 0 {
            let read = remaining.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            self.hash.update(&buffer[..read]);
            consume(&buffer[..read]);
        }
        if remaining.limit() != 0
            || self.body.read(&mut buffer[..1])? != 0
            || GitOid::from(self.hash.finalize()) != self.oid
        {
            return Err(invalid("Git checkpoint object failed verification", Path::new(".git")));
        }
        Ok(())
    }
}
