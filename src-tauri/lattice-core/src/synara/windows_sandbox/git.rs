use std::ffi::c_void;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use super::policy::{
    contains, dos_path, invalid, link_count, open_directory, path_by_handle, same_path,
};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GENERIC_ALL, GENERIC_WRITE, MAX_PATH,
};
use windows_sys::Win32::Security::{
    GetAce, GetKernelObjectSecurity, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    GetSidIdentifierAuthority, GetSidSubAuthority, GetSidSubAuthorityCount, IsWellKnownSid,
    WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE, ACE_HEADER,
    DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
    READ_CONTROL, VOLUME_NAME_GUID, WRITE_DAC, WRITE_OWNER,
};

const CSIDL_PROGRAM_FILES: i32 = 0x26;
const SHGFP_TYPE_DEFAULT: u32 = 1;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const MUTATE: u32 = GENERIC_ALL
    | GENERIC_WRITE
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES;

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
    let installed = match fs::canonicalize(program_files.join("Git")) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            log::warn!(target: "lattice::synara", "Ignoring unavailable Git for Windows: {error}");
            return Ok(None);
        }
    };
    if !contains(&program_files, &installed) {
        log::warn!(target: "lattice::synara", "Ignoring Git for Windows outside Program Files");
        return Ok(None);
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
        verify_acl(&pin, MUTATE)?;
        for ancestor in root.ancestors().skip(1) {
            let handle = open_entry(ancestor)?;
            verify_acl(&handle, MUTATE)?;
        }
    }
    verify_layout(&root)?;
    verify_tree(&root, !bundled)?;
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

fn verify_tree(root: &Path, check_acl: bool) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        let file = open_entry(&path)?;
        let metadata = file.metadata()?;
        if check_acl {
            verify_acl(&file, MUTATE)?;
        }
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
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
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

fn verify_acl(file: &File, mutating_rights: u32) -> io::Result<()> {
    let flags = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut needed = 0;
    let handle = file.as_raw_handle();
    let result = unsafe { GetKernelObjectSecurity(handle, flags, null_mut(), 0, &mut needed) };
    if result != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(invalid("Could not read Git installation ownership and ACL"));
    }
    let mut storage = vec![0_usize; (needed as usize).div_ceil(size_of::<usize>())];
    let descriptor = storage.as_mut_ptr().cast();
    if unsafe { GetKernelObjectSecurity(handle, flags, descriptor, needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } == 0
        || !trusted_sid(owner)
        || unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            == 0
        || present == 0
        || acl.is_null()
    {
        return Err(invalid("Git install requires administrator-controlled ownership and ACL"));
    }
    for index in 0..u32::from(unsafe { (*acl).AceCount }) {
        let mut entry = null_mut();
        if unsafe { GetAce(acl, index, &mut entry) } == 0 || !trusted_ace(entry, mutating_rights) {
            return Err(invalid("Git install grants writes to an untrusted principal"));
        }
    }
    Ok(())
}

fn trusted_ace(entry: *mut c_void, mutating_rights: u32) -> bool {
    let header = unsafe { &*entry.cast::<ACE_HEADER>() };
    if header.AceType == ACCESS_DENIED_ACE_TYPE || header.AceFlags & (INHERIT_ONLY_ACE as u8) != 0 {
        return true;
    }
    if header.AceType != ACCESS_ALLOWED_ACE_TYPE
        || usize::from(header.AceSize) < offset_of!(ACCESS_ALLOWED_ACE, SidStart) + 8
    {
        return false;
    }
    let ace = entry.cast::<ACCESS_ALLOWED_ACE>();
    unsafe { (*ace).Mask & mutating_rights == 0 || trusted_sid((&raw mut (*ace).SidStart).cast()) }
}

fn trusted_sid(sid: PSID) -> bool {
    const TRUSTED_INSTALLER: [u32; 6] =
        [80, 956008885, 3418522649, 1831038044, 1853292631, 2271478464];
    if sid.is_null() {
        return false;
    }
    unsafe {
        IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
            || IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || ((*GetSidIdentifierAuthority(sid)).Value == [0, 0, 0, 0, 0, 5]
                && usize::from(*GetSidSubAuthorityCount(sid)) == TRUSTED_INSTALLER.len()
                && TRUSTED_INSTALLER
                    .iter()
                    .enumerate()
                    .all(|(index, expected)| *GetSidSubAuthority(sid, index as u32) == *expected))
    }
}
