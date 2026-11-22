use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Command;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
};
use windows_sys::Win32::Storage::InstallableFileSystems::{
    FilterConnectCommunicationPort, FilterSendMessage, FLT_PORT_FLAG_SYNC_HANDLE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIdOfThread, OpenProcessToken, OpenThread, ResumeThread,
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME,
};

use super::AgentProcess;
use crate::platform_windows::track_group;

const PORT: &str = r"\LatticeAgentGuard";
const ABI_VERSION: u32 = 2;
const PROBE: u32 = 0;
const ENROLL_PROCESS: u32 = 1;
const CHECK_VOLUME: u32 = 2;

#[repr(C)]
struct Request {
    version: u32,
    operation: u32,
    handle: u64,
}

const _: () = assert!(std::mem::size_of::<Request>() == 16);

fn filter_result(result: i32) -> io::Result<()> {
    match result {
        0 => Ok(()),
        _ => Err(io::Error::other(format!("Filter Manager HRESULT {:#010x}", result as u32))),
    }
}

struct Guard(OwnedHandle);

impl Guard {
    fn connect() -> io::Result<Self> {
        let name: Vec<u16> = PORT.encode_utf16().chain([0]).collect();
        let mut port = null_mut();
        // SAFETY: name is NUL-terminated and port is a writable HANDLE.
        filter_result(unsafe {
            FilterConnectCommunicationPort(
                name.as_ptr(),
                FLT_PORT_FLAG_SYNC_HANDLE,
                null(),
                0,
                null(),
                &mut port,
            )
        })?;
        if port.is_null() || port == INVALID_HANDLE_VALUE {
            return Err(io::Error::other("Filter Manager returned an invalid port"));
        }
        // SAFETY: Filter Manager transferred ownership of the non-null port.
        let guard = Self(unsafe { OwnedHandle::from_raw_handle(port) });
        guard.request(PROBE, 0)?;
        Ok(guard)
    }

    fn request(&self, operation: u32, handle: usize) -> io::Result<()> {
        let request = Request { version: ABI_VERSION, operation, handle: handle as u64 };
        let mut bytes_returned = 0;
        // SAFETY: all request bytes are initialized and the operation has no response buffer.
        filter_result(unsafe {
            FilterSendMessage(
                self.0.as_raw_handle(),
                (&request as *const Request).cast(),
                std::mem::size_of::<Request>() as u32,
                null_mut(),
                0,
                &mut bytes_returned,
            )
        })
    }

    fn check_volume(&self, workspace: &Path) -> io::Result<()> {
        if !workspace.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected an absolute directory",
            ));
        }
        let directory = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(workspace)?;
        self.request(CHECK_VOLUME, directory.as_raw_handle() as usize)
    }
}

pub(super) fn check_workspace(workspace: &Path) -> io::Result<()> {
    Guard::connect()?.check_volume(workspace)
}

fn require_standard_token() -> io::Result<()> {
    let mut token = null_mut();
    // SAFETY: the current process handle is valid and token is a writable HANDLE.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: TokenElevation writes a TOKEN_ELEVATION of the supplied size.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if elevation.TokenIsElevated != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Run Lattice without elevation",
        ));
    }
    Ok(())
}

fn primary_thread_id(pid: u32) -> io::Result<u32> {
    // SAFETY: the snapshot handle is owned until enumeration finishes.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry =
        THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
    // SAFETY: entry points to a correctly sized, writable THREADENTRY32.
    if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut primary = None;
    loop {
        if entry.th32OwnerProcessID == pid && primary.replace(entry.th32ThreadID).is_some() {
            return Err(io::Error::other("The suspended process has multiple threads"));
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        // SAFETY: snapshot and entry remain valid across enumeration calls.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return primary.ok_or_else(|| io::Error::other("The suspended thread is missing"));
            }
            return Err(error);
        }
    }
}

fn resume_suspended(pid: u32) -> io::Result<()> {
    let id = primary_thread_id(pid)?;
    // SAFETY: OpenThread returns a handle that is owned only on success.
    let thread =
        unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION, 0, id) };
    if thread.is_null() {
        return Err(io::Error::last_os_error());
    }
    let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
    // SAFETY: the handle has query and resume rights.
    let count = unsafe {
        if GetProcessIdOfThread(thread.as_raw_handle()) != pid {
            return Err(io::Error::other("The suspended thread changed ownership"));
        }
        ResumeThread(thread.as_raw_handle())
    };
    match count {
        1 => Ok(()),
        u32::MAX => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("The process suspension count changed")),
    }
}

pub(super) fn spawn(command: &mut Command, workspace: &Path) -> io::Result<AgentProcess> {
    require_standard_token()?;
    let guard = Guard::connect()?;
    guard.check_volume(workspace).map_err(|error| {
        io::Error::other(format!("Cannot verify Windows Agent project volume: {error}"))
    })?;
    if let Some(state) = command.get_current_dir() {
        guard.check_volume(state).map_err(|error| {
            io::Error::other(format!("Cannot verify Windows Agent state volume: {error}"))
        })?;
    }
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED)
        .spawn()?;
    let group = (|| {
        let group = track_group(child.id())?;
        guard.request(ENROLL_PROCESS, child.as_raw_handle() as usize)?;
        resume_suspended(child.id())?;
        Ok(group)
    })();
    match group {
        Ok(group) => Ok(AgentProcess { child, _group: group }),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

pub fn run_probe_if_requested() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--lattice-guard-probe")) {
        return;
    }
    let status = (|| {
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
            return Err(io::Error::other("Missing protected workload separator"));
        }
        let program = args.next().ok_or_else(|| io::Error::other("Missing protected workload"))?;
        let workspace = std::env::current_dir()?;
        let mut process = spawn(Command::new(program).args(args), &workspace)?;
        process.child.wait()
    })();
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
