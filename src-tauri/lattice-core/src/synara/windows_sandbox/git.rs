use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{FILE_READ_ATTRIBUTES, MAX_PATH};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use super::policy::{
    contains, dos_path, invalid, link_count, open_directory, path_by_handle, same_path,
};

const CSIDL_PROGRAM_FILES: i32 = 0x26;
const SHGFP_TYPE_DEFAULT: u32 = 1;

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetFolderPathW(
        hwnd: *mut std::ffi::c_void, csidl: i32, token: *mut std::ffi::c_void, flags: u32,
        path: *mut u16,
    ) -> i32;
}

pub(super) struct Installation {
    root: PathBuf,
    bundled: bool,
    _pin: File,
}

impl Installation {
    pub(super) fn additional_grant(&self) -> Option<&PathBuf> {
        (!self.bundled).then_some(&self.root)
    }

    pub(super) fn command_dir(&self) -> PathBuf {
        self.root.join("cmd")
    }
}

pub(super) fn discover(
    runtime: &Path, policy_roots: &[PathBuf], profile: &Path,
) -> io::Result<Option<Installation>> {
    let bundled = runtime.join("tools/git");
    match fs::symlink_metadata(&bundled) {
        Ok(_) => return verify_installation(&bundled, true, policy_roots, profile).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let program_files = fs::canonicalize(program_files()?)?;
    let installed = fs::canonicalize(program_files.join("Git"))?;
    if !contains(&program_files, &installed) {
        return Err(invalid("Git for Windows must resolve under Program Files"));
    }
    match verify_installation(&installed, false, policy_roots, profile) {
        Ok(git) => Ok(Some(git)),
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!(target: "lattice::synara", "Ignoring unavailable or untrusted Git for Windows: {error}");
            }
            Ok(None)
        }
    }
}

fn verify_installation(
    path: &Path, bundled: bool, policy_roots: &[PathBuf], profile: &Path,
) -> io::Result<Installation> {
    let (root, pin) = open_directory(path)?;
    if !same_path(&root, path) {
        return Err(invalid("Git install must resolve to its expected directory"));
    }
    if !bundled {
        let canonical = path_by_handle(&pin, VOLUME_NAME_GUID)?;
        if policy_roots
            .iter()
            .any(|other| contains(&canonical, other) || contains(other, &canonical))
            || contains(profile, &root)
            || contains(&root, profile)
        {
            return Err(invalid("Git install overlaps a workspace, runtime or user profile"));
        }
    }
    verify_layout(&root)?;
    verify_tree(&root)?;
    Ok(Installation { root, bundled, _pin: pin })
}

fn verify_layout(root: &Path) -> io::Result<()> {
    for relative in ["cmd/git.exe", "mingw64/bin/git.exe"] {
        if !fs::symlink_metadata(root.join(relative))?.is_file() {
            return Err(invalid("Expected an x64 Git for Windows executable"));
        }
    }
    if !fs::symlink_metadata(root.join("mingw64/libexec/git-core"))?.is_dir() {
        return Err(invalid("Expected the Git for Windows helper directory"));
    }
    Ok(())
}

fn verify_tree(root: &Path) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        let file = open_entry(&path)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if !metadata.is_file() || link_count(&file)? != 1 {
            return Err(invalid("Git install contains a file alias"));
        }
    }
    Ok(())
}

fn open_entry(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid("Git install contains a Windows reparse point"));
    }
    Ok(file)
}

fn program_files() -> io::Result<PathBuf> {
    let mut path = [0_u16; MAX_PATH as usize];
    let result = unsafe {
        SHGetFolderPathW(
            null_mut(),
            CSIDL_PROGRAM_FILES,
            null_mut(),
            SHGFP_TYPE_DEFAULT,
            path.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Err(io::Error::other(format!("Windows Program Files lookup failed ({result:#x})")));
    }
    let end = path.iter().position(|unit| *unit == 0).unwrap_or(path.len());
    dos_path(Path::new(&OsString::from_wide(&path[..end])))
}
