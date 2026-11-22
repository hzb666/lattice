use std::ffi::OsString;
use std::io;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, HANDLE,
    INVALID_HANDLE_VALUE, NO_ERROR, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::AF_INET;
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetProcessIdOfThread, OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

pub(super) struct Job(OwnedHandle);

impl Job {
    fn new() -> io::Result<Self> {
        let handle = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        win_ok(unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Ok(Self(handle))
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        win_ok(unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            win_ok(unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            })?;
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "MXC process tree did not exit",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn verify_listener(
        &self, launcher: &Child, pid: u32, port: u16, node: &Path,
    ) -> io::Result<bool> {
        if port == 0 || pid == 0 || pid == launcher.id() || !running(launcher.as_raw_handle())? {
            return Ok(false);
        }
        let process = match owned(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid)
        }) {
            Ok(process) => process,
            Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        let mut member = 0;
        win_ok(unsafe {
            IsProcessInJob(process.as_raw_handle(), self.0.as_raw_handle(), &mut member)
        })?;
        if member == 0 || !running(process.as_raw_handle())? || !same_image(&process, node)? {
            return Ok(false);
        }
        Ok(loopback_listener(pid, port)?
            && running(process.as_raw_handle())?
            && running(launcher.as_raw_handle())?)
    }
}

pub(super) fn spawn_in_job(mut command: Command) -> io::Result<(Child, Job)> {
    let job = Job::new()?;
    let mut child = command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED).spawn()?;
    let launch =
        win_ok(unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), child.as_raw_handle()) })
            .and_then(|()| resume_primary_thread(child.id()));
    if let Err(error) = launch {
        drop(job);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok((child, job))
}

fn resume_primary_thread(pid: u32) -> io::Result<()> {
    let thread_id = suspended_thread_id(pid)?;
    let thread = owned(unsafe {
        OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_SUSPEND_RESUME, 0, thread_id)
    })?;
    if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != pid {
        return Err(io::Error::other("MXC launcher thread changed ownership"));
    }
    match unsafe { ResumeThread(thread.as_raw_handle()) } {
        1 => Ok(()),
        u32::MAX => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("MXC launcher thread had an unexpected suspend count")),
    }
}

fn suspended_thread_id(pid: u32) -> io::Result<u32> {
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry =
        THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    win_ok(unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) })?;
    let mut thread = None;
    loop {
        if (entry.dwSize as usize)
            < offset_of!(THREADENTRY32, th32OwnerProcessID) + size_of::<u32>()
        {
            return Err(io::Error::other("Windows thread snapshot entry is incomplete"));
        }
        if entry.th32OwnerProcessID == pid && thread.replace(entry.th32ThreadID).is_some() {
            return Err(io::Error::other("Suspended MXC launcher has multiple threads"));
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(error);
            }
            return thread
                .ok_or_else(|| io::Error::other("Suspended MXC launcher thread was not found"));
        }
    }
}

fn same_image(process: &OwnedHandle, node: &Path) -> io::Result<bool> {
    let mut image = vec![0_u16; 32_768];
    let mut length = image.len() as u32;
    win_ok(unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, image.as_mut_ptr(), &mut length)
    })?;
    let image = OsString::from_wide(&image[..length as usize]);
    Ok(super::policy::same_path(Path::new(&image), node))
}

fn loopback_listener(pid: u32, port: u16) -> io::Result<bool> {
    const MAX_TABLE_BYTES: u32 = 8 * 1024 * 1024;
    let mut bytes = 0;
    let mut table = Vec::<u32>::new();
    for _ in 0..4 {
        let result = unsafe {
            GetExtendedTcpTable(
                if table.is_empty() { null_mut() } else { table.as_mut_ptr().cast() },
                &mut bytes,
                0,
                AF_INET.into(),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match result {
            NO_ERROR => return listener_matches(&table, pid, port),
            ERROR_INSUFFICIENT_BUFFER if bytes <= MAX_TABLE_BYTES => {
                let words = (bytes as usize).div_ceil(size_of::<u32>());
                table.resize(words, 0);
            }
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
    Err(io::Error::other("Windows TCP listener table changed during MXC readiness check"))
}

fn listener_matches(table: &[u32], pid: u32, port: u16) -> io::Result<bool> {
    let count = table.first().copied().unwrap_or_default() as usize;
    let first_row = offset_of!(MIB_TCPTABLE_OWNER_PID, table);
    let length = size_of_val(table);
    let row_size = size_of::<MIB_TCPROW_OWNER_PID>();
    if first_row > length || count > (length - first_row) / row_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid Windows TCP listener table",
        ));
    }
    let rows = unsafe {
        std::slice::from_raw_parts(
            table.as_ptr().cast::<u8>().add(first_row).cast::<MIB_TCPROW_OWNER_PID>(),
            count,
        )
    };
    let mut exact = false;
    for row in rows.iter().filter(|row| u16::from_be(row.dwLocalPort as u16) == port) {
        let loopback = row.dwLocalAddr.to_ne_bytes() == [127, 0, 0, 1];
        if (loopback || row.dwLocalAddr == 0) && row.dwOwningPid != pid {
            return Ok(false);
        }
        exact |= loopback;
    }
    Ok(exact)
}

fn running(handle: HANDLE) -> io::Result<bool> {
    match unsafe { WaitForSingleObject(handle, 0) } {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn win_ok(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
