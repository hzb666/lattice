use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, Stdio};
use std::ptr::null_mut;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    GetVolumeInformationByHandleW, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, VOLUME_NAME_DOS,
    VOLUME_NAME_GUID,
};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

use super::{git, paths_overlap, SandboxConfig};

const MXC_SHA256: &str = "6d466c6b0109bab4bcace6738e11f886d801ea977346db49707488fcbd3afc8a";
const DRIVE_FIXED: u32 = 3;
const MIGRATION_DIGEST_ENV: &str = "SYNARA_MIGRATION_RUNTIME_SOURCE_DIGEST";
const PREPARE_TEMP: &str = include_str!("prepare_temp.mjs");

pub(super) struct Roots {
    pub runtime: PathBuf,
    pub control: PathBuf,
    host_executable: PathBuf,
    home: PathBuf,
    staging: PathBuf,
    workspace: PathBuf,
    original: Option<PathBuf>,
    system: PathBuf,
    git: Option<git::Installation>,
    _pins: Vec<File>,
    _host_pin: File,
}

impl Roots {
    pub(super) fn home(&self) -> &Path {
        &self.home
    }

    pub(super) fn open(config: &SandboxConfig<'_>) -> io::Result<Self> {
        let mut pins = Vec::new();
        let mut open = |path: &Path| {
            let (path, pin) = open_directory(path)?;
            pins.push(pin);
            Ok::<_, io::Error>(path)
        };
        let runtime = open(config.runtime_root)?;
        let home = open(config.agent_home)?;
        let staging = open(config.staging_parent)?;
        let control = open(config.control_dir)?;
        let original = config.original.map(&mut open).transpose()?;
        let (workspace, workspace_pin) = open_directory(config.workspace_root)?;
        let (host_executable, host_pin) = open_host_executable(config.host_executable)?;
        let system = windows_directory()?;
        let roots = pins
            .iter()
            .map(|pin| path_by_handle(pin, VOLUME_NAME_GUID))
            .collect::<io::Result<Vec<_>>>()?;
        if roots.iter().any(|root| contains(root, &host_executable)) {
            return Err(invalid("The Lattice key helper must be outside MXC policy roots"));
        }
        if !contains(&staging, &workspace)
            || same_path(&staging, &workspace)
            || [&runtime, &home, &control].into_iter().any(|root| paths_overlap(root, &workspace))
            || original.as_ref().is_some_and(|root| paths_overlap(root, &workspace))
            || contains(&workspace, &host_executable)
            || contains(&host_executable, &workspace)
        {
            return Err(invalid("MXC workspace must be isolated under its staging parent"));
        }
        for (index, path) in roots.iter().enumerate() {
            if roots[..index].iter().any(|other| contains(path, other) || contains(other, path)) {
                return Err(invalid("MXC policy roots overlap"));
            }
        }
        if [&home, &staging].into_iter().any(|root| {
            root.parent()
                .zip(control.parent())
                .is_none_or(|(root, control)| !same_path(root, control))
        }) {
            return Err(invalid("MXC agent, staging and control directories must be siblings"));
        }
        #[allow(deprecated)]
        let profile =
            std::env::home_dir().ok_or_else(|| invalid("Windows user profile is unknown"))?;
        let profile = dos_path(&fs::canonicalize(profile)?)?;
        for root in [&home, &staging, &workspace, &control] {
            if contains(root, &profile) || contains(root, &system) || contains(&system, root) {
                return Err(invalid("MXC requires dedicated agent and host control directories"));
            }
        }
        let runtime_bin = runtime.join("bin");
        let (bin, bin_pin) = open_directory(&runtime_bin)?;
        if !same_path(&bin, &runtime_bin) {
            return Err(invalid("MXC runtime bin resolves outside its packaged location"));
        }
        pins.push(bin_pin);
        reject_existing_aliases(&home)?;
        reject_existing_aliases(&workspace)?;
        let git = git::discover(&runtime, &roots, &profile)?;
        pins.push(workspace_pin);
        Ok(Self {
            runtime,
            control,
            host_executable,
            home,
            staging,
            workspace,
            original,
            system,
            git,
            _pins: pins,
            _host_pin: host_pin,
        })
    }

    pub(super) fn check_log(&self, file: &File) -> io::Result<()> {
        let path = final_path(file)?;
        if !contains(&self.control, &path) || !file.metadata()?.is_file() || link_count(file)? != 1
        {
            return Err(invalid("MXC logs require ordinary files in the host control directory"));
        }
        Ok(())
    }

    pub(super) fn request(&self, command: &Command) -> io::Result<(PathBuf, Value)> {
        let node = bundle_file(&self.runtime, Path::new("bin/node.exe"))?;
        let requested = Path::new(command.get_program());
        if !requested.is_absolute() || !same_path(&dos_path(&fs::canonicalize(requested)?)?, &node)
        {
            return Err(invalid("MXC requires the bundled Synara Node executable"));
        }
        let args = command.get_args().map(unicode).collect::<io::Result<Vec<_>>>()?;
        let preload =
            format!("--import=data:text/javascript;base64,{}", STANDARD.encode(PREPARE_TEMP));
        let cmdline = std::iter::once(unicode(node.as_os_str())?)
            .chain([preload.as_str()])
            .chain(args)
            .map(quote_arg)
            .collect::<Vec<_>>()
            .join(" ");
        let environment = self.child_environment(command)?;
        // MXC enforces read-only access even for per-user relocated Program Files.
        let mut readonly: Vec<_> = std::iter::once(&self.runtime)
            .chain(self.git.iter().filter_map(git::Installation::additional_grant))
            .chain(std::iter::once(&self.host_executable))
            .collect();
        let mut enumerate = BTreeSet::new();
        for root in readonly.iter().copied().chain([&self.home, &self.workspace]) {
            enumerate.extend(root.ancestors().skip(1));
        }
        let denied: Vec<_> = std::iter::once(&self.control).chain(self.original.iter()).collect();
        let request = json!({
            "version": "1.0.0",
            "containment": "processcontainer",
            "process": {
                "commandLine": cmdline, "cwd": self.home, "timeout": 0,
                "inheritDefaultEnv": false, "env": environment,
            },
            "filesystem": {
                "readonlyPaths": readonly,
                "readwritePaths": [&self.home, &self.workspace],
                "deniedPaths": denied,
            },
            "processContainer": { "filesystem": { "enumeratePaths": enumerate } },
            "network": {
                "egress": { "default": "allow" },
                "ingress": { "default": "deny", "hostLoopback": "allow" },
            },
            "ui": { "disable": false },
        });
        Ok((node, request))
    }

    pub(super) fn launcher(&self, executable: &Path) -> Command {
        launcher(executable, &self.system, &self.control)
    }

    fn child_environment(&self, command: &Command) -> io::Result<Vec<String>> {
        let mut vars = BTreeMap::new();
        for (name, value) in command.get_envs() {
            let Some(value) = value else { continue };
            let Some(name) = name.to_str() else { continue };
            let name = name.to_ascii_uppercase();
            if allowed_env(&name) {
                vars.insert(name, unicode(value)?.to_owned());
            }
        }
        let tokens = vars.get("SYNARA_AUTH_TOKEN").zip(vars.get("SYNARA_DESKTOP_SHUTDOWN_TOKEN"));
        if tokens.is_none_or(|(auth, shutdown)| {
            auth.is_empty() || shutdown.is_empty() || auth == shutdown
        }) {
            return Err(invalid("MXC requires distinct Synara authentication and shutdown tokens"));
        }
        let digest = migration_digest(&self.runtime)?;
        if vars.get(MIGRATION_DIGEST_ENV).is_some_and(|requested| requested != &digest) {
            return Err(invalid(
                "Synara migration runtime identity differs from the bundled manifest",
            ));
        }
        vars.insert(MIGRATION_DIGEST_ENV.into(), digest);
        let home = unicode(self.home.as_os_str())?;
        let system = unicode(self.system.as_os_str())?;
        let temp = self.home.join("temp");
        match fs::create_dir(&temp) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let (temp, _pin) = open_directory(&temp)?;
        if !contains(&self.home, &temp) || same_path(&self.home, &temp) {
            return Err(invalid("MXC temporary directory must be inside the isolated home"));
        }
        for name in ["HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "SYNARA_HOME"] {
            vars.insert(name.into(), home.into());
        }
        for name in ["TEMP", "TMP", "TMPDIR"] {
            vars.insert(name.into(), unicode(temp.as_os_str())?.into());
        }
        for name in ["SYSTEMROOT", "WINDIR"] {
            vars.insert(name.into(), system.into());
        }
        let bin = self.runtime.join("bin");
        let system_bin = self.system.join("System32");
        let path = std::env::join_paths(
            std::iter::once(bin)
                .chain(self.git.iter().map(git::Installation::command_dir))
                .chain([system_bin]),
        )
        .map_err(io::Error::other)?;
        vars.insert("PATH".into(), unicode(&path)?.into());
        vars.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        vars.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        vars.insert(
            "PI_CODING_AGENT_DIR".into(),
            unicode(self.home.join("pi-agent").as_os_str())?.into(),
        );
        vars.insert("LATTICE_BIN".into(), unicode(self.host_executable.as_os_str())?.into());
        vars.insert("NODE_ENV".into(), "production".into());
        vars.insert("SYNARA_MODE".into(), "desktop".into());
        vars.insert("SYNARA_HOST".into(), "127.0.0.1".into());
        vars.insert("SYNARA_NO_BROWSER".into(), "true".into());
        vars.insert("SYNARA_TELEMETRY_ENABLED".into(), "false".into());
        vars.insert("AGENT_HOST_PROFILE".into(), "lattice".into());
        Ok(vars.into_iter().map(|(name, value)| format!("{name}={value}")).collect())
    }
}

fn migration_digest(runtime: &Path) -> io::Result<String> {
    let manifest = bundle_file(runtime, Path::new("manifest.json"))?;
    let document: Value = serde_json::from_reader(File::open(manifest)?.take(64 * 1024))?;
    document["migrationRuntimeSourceDigest"]
        .as_str()
        .filter(|digest| valid_migration_digest(digest))
        .map(str::to_owned)
        .ok_or_else(|| {
            invalid("The bundled Synara manifest has no valid migration runtime identity")
        })
}

fn valid_migration_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn verified_executor(runtime: &Path) -> io::Result<(PathBuf, File)> {
    let expected = runtime.join("bin/wxc-exec.exe");
    let mut pin = OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&expected)?;
    let path = final_path(&pin)?;
    if !same_path(&path, &expected) {
        return Err(invalid("MXC executor must be in the bundled runtime"));
    }
    let mut hash = Sha256::new();
    io::copy(&mut pin.by_ref().take(128 * 1024 * 1024 + 1), &mut hash)?;
    if format!("{:x}", hash.finalize()) != MXC_SHA256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected the pinned Windows x64 wxc-exec.exe from Microsoft MXC SDK 1.1.0",
        ));
    }
    Ok((path, pin))
}

fn bundle_file(runtime: &Path, relative: &Path) -> io::Result<PathBuf> {
    let expected = runtime.join(relative);
    let path = dos_path(&fs::canonicalize(&expected)?)?;
    if !same_path(&path, &expected) || !path.is_file() {
        return Err(invalid("MXC runtime file resolves outside its packaged location"));
    }
    Ok(path)
}

fn open_host_executable(path: &Path) -> io::Result<(PathBuf, File)> {
    let expected = dos_path(&fs::canonicalize(path)?)?;
    let pin = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&expected)?;
    let metadata = pin.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || link_count(&pin)? != 1
        || !same_path(&final_path(&pin)?, &expected)
    {
        return Err(invalid("The Lattice key helper must be a pinned, ordinary executable file"));
    }
    Ok((expected, pin))
}

fn allowed_env(name: &str) -> bool {
    matches!(
        name,
        "SYNARA_AUTH_TOKEN"
            | "SYNARA_DESKTOP_SHUTDOWN_TOKEN"
            | MIGRATION_DIGEST_ENV
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "ALL_PROXY"
            | "NO_PROXY"
    ) || ["_API_KEY", "_BASE_URL", "_API_BASE"]
        .iter()
        .any(|suffix| name.ends_with(suffix) && super::super::environment::is_inherited(name))
}

pub(super) fn reject_existing_aliases(root: &Path) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(invalid("MXC writable roots contain a reparse point"));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let file = OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(entry.path())?;
            if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || link_count(&file)? != 1
            {
                return Err(invalid("MXC writable roots contain a file alias"));
            }
        }
    }
    Ok(())
}

pub(super) fn link_count(file: &File) -> io::Result<u32> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.nNumberOfLinks)
}

pub(super) fn open_directory(path: &Path) -> io::Result<(PathBuf, File)> {
    if !path.is_absolute() {
        return Err(invalid("MXC requires absolute local directory paths"));
    }
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid("MXC policy roots must be ordinary directories"));
    }
    let path = final_path(&file)?;
    let root = path.ancestors().last().ok_or_else(|| invalid("Missing MXC drive root"))?;
    let drive: Vec<_> = root.as_os_str().encode_wide().chain([0]).collect();
    let mut fs_name = [0_u16; 16];
    let local_ntfs =
        unsafe {
            GetDriveTypeW(drive.as_ptr()) == DRIVE_FIXED
                && GetVolumeInformationByHandleW(
                    file.as_raw_handle(),
                    null_mut(),
                    0,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    fs_name.as_mut_ptr(),
                    fs_name.len() as u32,
                ) != 0
        } && fs_name.starts_with(&[b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0]);
    if !local_ntfs || same_path(&path, root) {
        return Err(invalid("MXC requires subdirectories on local fixed NTFS volumes"));
    }
    Ok((path, file))
}

fn final_path(file: &File) -> io::Result<PathBuf> {
    dos_path(&path_by_handle(file, VOLUME_NAME_DOS)?)
}

pub(super) fn path_by_handle(file: &File, volume: u32) -> io::Result<PathBuf> {
    let mut path = vec![0_u16; 32_768];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            path.as_mut_ptr(),
            path.len() as u32,
            FILE_NAME_NORMALIZED | volume,
        )
    } as usize;
    if written == 0 || written >= path.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&path[..written])))
}

pub(super) fn launcher(executable: &Path, system: &Path, control: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("SystemRoot", system)
        .env("WINDIR", system)
        .env("PATH", system.join("System32"))
        .env("TEMP", control)
        .env("TMP", control)
        .current_dir(control)
        .stdin(Stdio::null());
    command
}

pub(super) fn windows_directory() -> io::Result<PathBuf> {
    let mut path = vec![0_u16; 32_768];
    let written =
        unsafe { GetSystemWindowsDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
    if written == 0 || written >= path.len() {
        return Err(io::Error::last_os_error());
    }
    dos_path(&fs::canonicalize(OsString::from_wide(&path[..written]))?)
}

pub(super) fn dos_path(path: &Path) -> io::Result<PathBuf> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(invalid("MXC requires a local DOS drive path")),
        },
        _ => return Err(invalid("MXC requires a local DOS drive path")),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(invalid("MXC requires a rooted drive path"));
    }
    let mut normalized = PathBuf::from(format!("{}:\\", char::from(drive)));
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid("MXC requires a normalized DOS path"));
        };
        let name = unicode(name)?;
        if name.contains(':') || name.ends_with(['.', ' ']) {
            return Err(invalid("MXC path has ambiguous DOS path semantics"));
        }
        normalized.push(name);
    }
    Ok(normalized)
}

pub(super) fn contains(parent: &Path, child: &Path) -> bool {
    let mut child = child.components();
    parent.components().all(|parent| {
        child.next().is_some_and(|child| windows_eq(parent.as_os_str(), child.as_os_str()))
    })
}

pub(super) fn same_path(first: &Path, second: &Path) -> bool {
    contains(first, second) && first.components().count() == second.components().count()
}

fn windows_eq(left: &OsStr, right: &OsStr) -> bool {
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    (unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    }) == CSTR_EQUAL
}

pub(super) fn unicode(value: &OsStr) -> io::Result<&str> {
    value
        .to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| invalid("MXC input contains invalid Windows text"))
}

pub(super) fn quote_arg(argument: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else {
            quoted.extend(std::iter::repeat_n(
                '\\',
                backslashes * if character == '"' { 2 } else { 1 },
            ));
            if character == '"' {
                quoted.push('\\');
            }
            quoted.push(character);
            backslashes = 0;
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::{
        allowed_env, contains, quote_arg, valid_migration_digest, Path, MIGRATION_DIGEST_ENV,
    };

    #[test]
    fn command_line_preserves_quotes_and_trailing_backslashes() {
        assert_eq!("\"\"", quote_arg(""));
        assert_eq!(r#""C:\a b\\""#, quote_arg(r"C:\a b\"));
        assert_eq!(r#""a\\\"b""#, quote_arg(r#"a\"b"#));
    }

    #[test]
    fn path_overlap_compares_windows_components() {
        assert!(contains(Path::new(r"C:\A\Édit"), Path::new(r"c:\a\édit\one")));
        assert!(!contains(Path::new(r"C:\A"), Path::new(r"C:\AB")));
    }

    #[test]
    fn provider_environment_omits_host_bridge_and_code_injection_switches() {
        assert!(allowed_env("OPENAI_API_KEY"));
        for key in
            ["LATTICE_BIN", "NODE_OPTIONS", "CLAUDE_CODE_GIT_BASH_PATH", "SYNARA_HOME", "PATH"]
        {
            assert!(!allowed_env(key));
        }
    }

    #[test]
    fn migration_identity_requires_a_sha256_digest() {
        assert!(allowed_env(MIGRATION_DIGEST_ENV));
        assert!(valid_migration_digest(&"a1".repeat(32)));
        assert!(!valid_migration_digest(&"a1".repeat(31)));
        assert!(!valid_migration_digest(&"x1".repeat(32)));
    }
}
