use std::ffi::{c_void, OsStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_FILES, HANDLE, NTSTATUS,
    UNICODE_STRING,
};
use windows_sys::Win32::Security::{
    GetKernelObjectSecurity, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileCaseSensitiveInfo, FileDispositionInfo, FileIdBothDirectoryInfo,
    FileIdBothDirectoryRestartInfo, FileStreamInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetVolumeInformationByHandleW,
    SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_CASE_SENSITIVE_INFO, FILE_DISPOSITION_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_RENAME_INFO, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STREAM_INFO, FILE_TRAVERSE, FILE_WRITE_DATA, READ_CONTROL, SYNCHRONIZE,
    VOLUME_NAME_GUID,
};

use super::{invalid, permitted_name};

const DIR_ACCESS: u32 = FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES;
const DIR_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const OBJ_DONT_REPARSE: u32 = 0x1000;
const FILE_RENAME_INFORMATION: i32 = 10;

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: HANDLE,
    object_name: *const UNICODE_STRING,
    attributes: u32,
    security_descriptor: *const c_void,
    security_qos: *const c_void,
}

#[repr(C)]
#[derive(Default)]
struct IoStatusBlock {
    status_or_pointer: usize,
    information: usize,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateFile(
        file: *mut HANDLE, access: u32, attributes: *const ObjectAttributes,
        status: *mut IoStatusBlock, allocation_size: *const i64, file_attributes: u32, share: u32,
        disposition: u32, options: u32, ea_buffer: *const c_void, ea_length: u32,
    ) -> NTSTATUS;
    fn NtSetInformationFile(
        file: HANDLE, status: *mut IoStatusBlock, info: *const c_void, length: u32, class: i32,
    ) -> NTSTATUS;
    fn RtlNtStatusToDosError(status: NTSTATUS) -> u32;
}

fn nt_result(status: NTSTATUS) -> io::Result<()> {
    if status < 0 {
        // SAFETY: Conversion accepts any native status value.
        return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub(super) struct Identity {
    pub(super) volume: u32,
    pub(super) index: u64,
}

pub(super) struct Root {
    pub(super) path: PathBuf,
    pub(super) directory: Arc<File>,
    ancestors: Vec<(Identity, Arc<File>)>,
}

impl Root {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let mut parts = path.components();
        let drive = match parts.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
                _ => return Err(invalid("Workspace requires a local NTFS path", path)),
            },
            _ => return Err(invalid("Workspace requires an absolute path", path)),
        };
        if parts.next() != Some(Component::RootDir) || parts.as_path().as_os_str().is_empty() {
            return Err(invalid("Workspace requires a directory below the volume root", path));
        }
        let volume = OpenOptions::new()
            .access_mode(DIR_ACCESS)
            .share_mode(DIR_SHARE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(format!(r"\\?\{}:\", char::from(drive)))?;
        verify_volume_root(&volume)?;
        let volume_id = identity(&volume)?;
        let mut directory = Arc::new(volume);
        let mut ancestors = vec![(volume_id, Arc::clone(&directory))];
        for part in parts {
            let Component::Normal(name) = part else {
                return Err(invalid("Invalid workspace ancestor", path));
            };
            let child = open_directory(&directory, name, false)?;
            directory = Arc::new(child);
            ancestors.push((identity(&directory)?, Arc::clone(&directory)));
        }
        Ok(Self { path: path.to_owned(), directory, ancestors })
    }

    pub(super) fn overlaps(&self, other: &Self) -> bool {
        self.is_inside(other) || other.is_inside(self)
    }

    fn is_inside(&self, other: &Self) -> bool {
        let other_root = other.ancestors.last().map(|(id, _)| id);
        self.ancestors.iter().any(|(id, _)| Some(id) == other_root)
    }

    pub(super) fn ancestry(&self) -> Vec<Identity> {
        self.ancestors.iter().map(|(id, _)| *id).collect()
    }

    pub(super) fn create_staging(path: &Path, original: &Self) -> io::Result<Self> {
        let (parent, name) = path.parent().zip(path.file_name()).ok_or_else(|| {
            invalid("Staging requires an existing parent and a directory name", path)
        })?;
        let mut root = Self::open(parent)?;
        if root.is_inside(original) {
            return Err(invalid("Staging parent is inside the project", parent));
        }
        let file = open_child(
            &root.directory,
            name,
            Open {
                access: DIR_ACCESS,
                share: DIR_SHARE,
                disposition: FILE_CREATE,
                kind: FILE_DIRECTORY_FILE,
            },
            &[],
        )?;
        verify_directory(&file)?;
        let directory = Arc::new(file);
        root.ancestors.push((identity(&directory)?, Arc::clone(&directory)));
        root.directory = directory;
        root.path = path.to_owned();
        Ok(root)
    }
}

fn verify_volume_root(file: &File) -> io::Result<()> {
    let mut resolved = [0u16; 64];
    let mut filesystem = [0u16; 16];
    // SAFETY: The buffers are writable and the handle names a directory.
    let (length, found) = unsafe {
        (
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                resolved.as_mut_ptr(),
                64,
                VOLUME_NAME_GUID,
            ),
            GetVolumeInformationByHandleW(
                file.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                filesystem.as_mut_ptr(),
                16,
            ),
        )
    };
    let volume = resolved.get(..length as usize).filter(|name| !name.is_empty());
    let direct = volume.and_then(|name| String::from_utf16(name).ok()).is_some_and(|name| {
        name.strip_prefix(r"\\?\Volume{")
            .is_some_and(|suffix| suffix.split_once("}\\").is_some_and(|(_, rest)| rest.is_empty()))
    });
    if !direct || found == 0 || filesystem[..5] != [78, 84, 70, 83, 0] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Workspace needs a direct local NTFS volume",
        ));
    }
    verify_directory(file)
}

struct Open {
    access: u32,
    share: u32,
    disposition: u32,
    kind: u32,
}

fn open_child(parent: &File, name: &OsStr, open: Open, security: &[u64]) -> io::Result<File> {
    if !permitted_name(name) {
        return Err(invalid("Unsafe Windows name", Path::new(name)));
    }
    let name: Vec<u16> = name.encode_wide().collect();
    let length = (name.len() * size_of::<u16>()) as u16;
    let unicode =
        UNICODE_STRING { Length: length, MaximumLength: length, Buffer: name.as_ptr().cast_mut() };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: &unicode,
        attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        security_descriptor: if security.is_empty() { null() } else { security.as_ptr().cast() },
        security_qos: null(),
    };
    let mut handle = null_mut();
    let mut status = IoStatusBlock::default();
    // SAFETY: This synchronous call uses a live parent and a single counted UTF-16 name.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            open.access | SYNCHRONIZE,
            &attributes,
            &mut status,
            null(),
            0,
            open.share,
            open.disposition,
            open.kind | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    nt_result(result)?;
    // SAFETY: Successful creation transfers ownership of the output file handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn open_directory(parent: &File, name: &OsStr, create: bool) -> io::Result<File> {
    let file = open_child(
        parent,
        name,
        Open {
            access: DIR_ACCESS | if create { DELETE } else { 0 },
            share: DIR_SHARE,
            disposition: if create { FILE_CREATE } else { FILE_OPEN },
            kind: FILE_DIRECTORY_FILE,
        },
        &[],
    )?;
    verify_directory(&file)?;
    Ok(file)
}

pub(super) fn read_file(parent: &File, name: &OsStr, movable: bool) -> io::Result<File> {
    open_child(
        parent,
        name,
        Open {
            access: FILE_READ_DATA
                | FILE_READ_ATTRIBUTES
                | if movable { DELETE | READ_CONTROL } else { 0 },
            share: FILE_SHARE_READ,
            disposition: FILE_OPEN,
            kind: FILE_NON_DIRECTORY_FILE,
        },
        &[],
    )
}

pub(super) fn new_file(parent: &File, name: &OsStr, security: &[u64]) -> io::Result<File> {
    open_child(
        parent,
        name,
        Open {
            access: FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE,
            share: FILE_SHARE_READ,
            disposition: FILE_CREATE,
            kind: FILE_NON_DIRECTORY_FILE,
        },
        security,
    )
}

pub(super) fn file_security(file: &File) -> io::Result<Vec<u64>> {
    let information =
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut bytes = 0;
    // SAFETY: The sizing call supplies a null buffer and a writable length.
    let first = unsafe {
        GetKernelObjectSecurity(file.as_raw_handle(), information, null_mut(), 0, &mut bytes)
    };
    if first != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(io::Error::last_os_error());
    }
    let mut descriptor = vec![0u64; (bytes as usize).div_ceil(size_of::<u64>())];
    // SAFETY: The descriptor storage is aligned and covers the requested byte count.
    let found = unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            information,
            descriptor.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    };
    if found == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(descriptor)
}

fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The structure is writable for the duration of the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

pub(super) fn identity(file: &File) -> io::Result<Identity> {
    let info = info(file)?;
    Ok(Identity {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

fn verify_directory(file: &File) -> io::Result<()> {
    let flags = info(file)?.dwFileAttributes;
    if flags & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != FILE_ATTRIBUTE_DIRECTORY
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Workspace directory is a reparse point",
        ));
    }
    let mut sensitivity = FILE_CASE_SENSITIVE_INFO::default();
    // SAFETY: This information class writes exactly FILE_CASE_SENSITIVE_INFO.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileCaseSensitiveInfo,
            (&mut sensitivity as *mut FILE_CASE_SENSITIVE_INFO).cast(),
            size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if sensitivity.Flags != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Case-sensitive workspace directory",
        ));
    }
    Ok(())
}

pub(super) fn verify_file(file: &File, movable: bool) -> io::Result<()> {
    let info = info(file)?;
    let forbidden = FILE_ATTRIBUTE_REPARSE_POINT
        | FILE_ATTRIBUTE_DIRECTORY
        | FILE_ATTRIBUTE_ENCRYPTED
        | FILE_ATTRIBUTE_OFFLINE
        | if movable { FILE_ATTRIBUTE_READONLY } else { 0 };
    if info.nNumberOfLinks != 1 || info.dwFileAttributes & forbidden != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsafe workspace file attributes or hard link",
        ));
    }
    let mut buffer = [0u64; 128];
    // SAFETY: The aligned buffer is writable and larger than the fixed header.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            size_of_val(&buffer) as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The header and expected stream name fit in the initialized buffer.
    let (header, name) = unsafe {
        let bytes = buffer.as_ptr().cast::<u8>();
        (
            &*bytes.cast::<FILE_STREAM_INFO>(),
            std::slice::from_raw_parts(
                bytes.add(offset_of!(FILE_STREAM_INFO, StreamName)).cast::<u16>(),
                7,
            ),
        )
    };
    if header.NextEntryOffset != 0
        || header.StreamNameLength != 14
        || !name.iter().copied().eq("::$DATA".encode_utf16())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Workspace file has alternate data streams",
        ));
    }
    Ok(())
}

pub(super) struct Child {
    pub(super) name: String,
    pub(super) index: u64,
    pub(super) directory: bool,
}

pub(super) fn children(directory: &File) -> io::Result<Vec<Child>> {
    let mut children = Vec::new();
    let mut buffer = vec![0u64; 8192];
    let mut class = FileIdBothDirectoryRestartInfo;
    loop {
        // SAFETY: The directory owns its enumeration cursor and the output buffer is aligned.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                size_of_val(&*buffer) as u32,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_NO_MORE_FILES | ERROR_HANDLE_EOF) => Ok(children),
                _ => Err(error),
            };
        }
        // SAFETY: All bytes in the allocation are initialized.
        let bytes = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), size_of_val(&*buffer))
        };
        append_children(bytes, &mut children)?;
        class = FileIdBothDirectoryInfo;
    }
}

fn append_children(mut bytes: &[u8], children: &mut Vec<Child>) -> io::Result<()> {
    let malformed =
        || io::Error::new(io::ErrorKind::InvalidData, "Malformed workspace directory record");
    loop {
        if bytes.len() < size_of::<FILE_ID_BOTH_DIR_INFO>() {
            return Err(malformed());
        }
        // SAFETY: The header fits; the records contain integers only.
        let entry = unsafe { bytes.as_ptr().cast::<FILE_ID_BOTH_DIR_INFO>().read_unaligned() };
        let next = entry.NextEntryOffset as usize;
        let end = if next == 0 { bytes.len() } else { next };
        let start = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let length = entry.FileNameLength as usize;
        if length == 0 || length % 2 != 0 || start + length > end || end > bytes.len() {
            return Err(malformed());
        }
        let name: Vec<u16> = bytes[start..start + length]
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect();
        let name = String::from_utf16(&name).map_err(|_| malformed())?;
        if name != "." && name != ".." {
            children.push(Child {
                name,
                index: entry.FileId as u64,
                directory: entry.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            });
        }
        if next == 0 {
            return Ok(());
        }
        bytes = &bytes[next..];
    }
}

pub(super) fn move_no_replace(file: &File, parent: &File, name: &OsStr) -> io::Result<()> {
    if !permitted_name(name) {
        return Err(invalid("Unsafe Windows rename target", Path::new(name)));
    }
    let name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = size_of_val(&*name);
    let bytes = size_of::<FILE_RENAME_INFO>() + name_bytes;
    let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
    let mut status = IoStatusBlock::default();
    // SAFETY: The aligned buffer has room for the fixed header and the counted name.
    let result = unsafe {
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).RootDirectory = parent.as_raw_handle();
        (*info).FileNameLength = name_bytes as u32;
        let target = buffer.as_mut_ptr().cast::<u8>().add(offset_of!(FILE_RENAME_INFO, FileName));
        std::ptr::copy_nonoverlapping(name.as_ptr(), target.cast::<u16>(), name.len());
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            info.cast(),
            bytes as u32,
            FILE_RENAME_INFORMATION,
        )
    };
    nt_result(result)
}

pub(super) fn unlink(file: &File) -> io::Result<()> {
    let mut info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: The live handle was opened with DELETE, and the fixed input type matches its class.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&mut info as *mut FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
