use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};

use super::policy::{invalid, link_count, open_directory, same_path};

pub(super) fn seed_environment_id(home: &Path) -> io::Result<()> {
    let userdata = home.join("userdata");
    match fs::create_dir(&userdata) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let (resolved, _pin) = open_directory(&userdata)?;
    if !same_path(&userdata, &resolved) {
        return Err(invalid("Synara userdata resolves outside its expected location"));
    }
    let path = resolved.join("environment-id");
    match validate_environment_id(&path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut pending = tempfile::Builder::new().prefix(".environment-id-").tempfile_in(resolved)?;
    writeln!(pending, "{}", uuid::Uuid::new_v4())?;
    pending.as_file().sync_all()?;
    match pending.persist_noclobber(&path) {
        Ok(file) => drop(file),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.error),
    }
    validate_environment_id(&path)
}

fn validate_environment_id(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    const MAX_ID_BYTES: u64 = 4096;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || link_count(&file)? != 1
        || metadata.len() > MAX_ID_BYTES
    {
        return Err(invalid("Synara environment ID must be a small, unaliased regular file"));
    }
    let mut existing = String::new();
    file.take(MAX_ID_BYTES + 1).read_to_string(&mut existing)?;
    if existing.trim().is_empty() || existing.contains('\0') {
        return Err(invalid("Synara environment ID is empty or invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_id_is_created_once_and_kept_on_restart() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        seed_environment_id(home.path())?;
        let path = home.path().join("userdata/environment-id");
        let first = fs::read_to_string(&path)?;
        assert!(uuid::Uuid::parse_str(first.trim()).is_ok());
        seed_environment_id(home.path())?;
        assert_eq!(first, fs::read_to_string(path)?);
        Ok(())
    }

    #[test]
    fn aliased_environment_id_is_rejected_and_untouched() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        let userdata = home.path().join("userdata");
        fs::create_dir(&userdata)?;
        let original = home.path().join("original");
        fs::write(&original, "kept\n")?;
        fs::hard_link(&original, userdata.join("environment-id"))?;
        assert!(seed_environment_id(home.path()).is_err());
        assert_eq!("kept\n", fs::read_to_string(original)?);
        Ok(())
    }
}
