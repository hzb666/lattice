use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn publish_with(
    destination: &Path,
    file: Option<&str>,
    build: impl FnOnce(&Path) -> crate::Result<()>,
) -> crate::Result<()> {
    ensure_new(destination)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = crate::tools::canonicalize(parent)?;
    let name = destination
        .file_name()
        .ok_or("output needs a directory name")?;
    let destination = parent.join(name);
    let mut stage = Stage::create(&parent)?;
    let result = build(&stage.0).and_then(|()| {
        if let Some(file) = file {
            let mut publication = Stage::create(&parent)?;
            fs::copy(stage.0.join(file), publication.0.join(file))?;
            publication.publish(&destination)?;
        } else {
            stage.publish(&destination)?;
        }
        Ok(())
    });
    if let Err(error) = result {
        stage.1 = true;
        return Err(format!(
            "build failed; diagnostics retained at {}: {error}",
            stage.0.display()
        )
        .into());
    }
    Ok(())
}

fn ensure_new(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "output already exists: {}; choose a new directory",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
pub(crate) fn write(path: &Path, content: &str) -> io::Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

pub(crate) struct Stage(pub(crate) PathBuf, bool);

impl Stage {
    pub(crate) fn create(parent: &Path) -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = OsString::from(format!(".chemex-{}-{stamp}.tmp", std::process::id()));
        let path = parent.join(name);
        fs::create_dir(&path)?;
        Ok(Self(path, false))
    }

    pub(crate) fn retain(&mut self) {
        self.1 = true;
    }

    fn publish(&mut self, destination: &Path) -> io::Result<()> {
        ensure_new(destination)?;
        fs::rename(&self.0, destination)?;
        self.1 = true;
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        // This absolute path names only the scratch directory created by Stage.
        if !self.1 {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn project() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "chemex-publish-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn failed_build_retains_logs_without_publishing_or_touching_last_success() {
        let directory = project();
        let previous = directory.join("previous.pdf");
        fs::write(&previous, "last successful PDF").unwrap();
        let result = publish_with(&directory.join("new"), Some("manuscript.pdf"), |stage| {
            write(&stage.join("latexmk.stderr.log"), "compiler failure")?;
            Err("compilation failed".into())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("diagnostics retained")
        );
        assert!(!directory.join("new").exists());
        assert_eq!("last successful PDF", fs::read_to_string(previous).unwrap());
        let stage = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            "compiler failure",
            fs::read_to_string(stage.join("latexmk.stderr.log")).unwrap()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn successful_build_publishes_all_artifacts_together() {
        let directory = project();
        let destination = directory.join("complete");
        publish_with(&destination, None, |stage| {
            write(&stage.join("drawing.cdxml"), "drawing")?;
            write(&stage.join("manuscript.pdf"), "%PDF-test")?;
            Ok(())
        })
        .unwrap();
        for file in ["drawing.cdxml", "manuscript.pdf"] {
            assert!(destination.join(file).is_file(), "{file}");
        }
        assert_eq!(1, fs::read_dir(&directory).unwrap().count());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_output_publishes_only_the_pdf_and_cleans_staging() {
        let directory = project();
        let destination = directory.join("complete");
        publish_with(&destination, Some("manuscript.pdf"), |stage| {
            write(&stage.join("discovery.pdf"), "%PDF-intermediate")?;
            write(&stage.join("manuscript.pdf"), "%PDF-final")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            "%PDF-final",
            fs::read_to_string(destination.join("manuscript.pdf")).unwrap()
        );
        assert_eq!(1, fs::read_dir(destination).unwrap().count());
        assert_eq!(1, fs::read_dir(&directory).unwrap().count());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_existing_destination_prevents_external_build_actions() {
        let directory = project();
        let result = publish_with(&directory, Some("manuscript.pdf"), |_| {
            panic!("external tools must not run for an existing output")
        });
        assert!(result.is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
