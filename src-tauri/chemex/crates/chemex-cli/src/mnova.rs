use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use crate::{output::Stage, tools};

const MAX_SPECTRUM_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;
const SCRIPT: &str = include_str!("mnova_export.qs");

static EXPORT: Mutex<()> = Mutex::new(());

pub fn export_spectrum_pdf(
    cache: &Path,
    source: &Path,
    spectrum: NonZeroUsize,
    run: impl FnOnce(Command, &Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    if !source
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mnova"))
    {
        return Err(format!("expected a .mnova file: {}", source.display()));
    }
    let source_hash = hash_file(source)?;
    let executable =
        tools::find("CHEMEX_MESTRENOVA", "MestReNova").map_err(|error| error.to_string())?;
    let key = cache_key(&source_hash, spectrum, &executable)?;
    fs::create_dir_all(cache).map_err(|error| format!("cannot create Mnova cache: {error}"))?;
    let cached = cache.join(format!("mnova-{key}.pdf"));
    if cached_pdf(&cached)? {
        if hash_file(source)? != source_hash {
            return Err(format!(
                "{} changed while its cached export was read",
                source.display()
            ));
        }
        return Ok(cached);
    }

    let _turn = EXPORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cached_pdf(&cached)? {
        if hash_file(source)? != source_hash {
            return Err(format!(
                "{} changed while its cached export was read",
                source.display()
            ));
        }
        return Ok(cached);
    }

    let mut stage = Stage::create(cache).map_err(|error| error.to_string())?;
    let source_copy = stage.0.join("source.mnova");
    fs::copy(source, &source_copy)
        .map_err(|error| format!("cannot copy Mnova source {}: {error}", source.display()))?;
    if hash_file(&source_copy)? != source_hash || hash_file(source)? != source_hash {
        return Err(format!("{} changed while being prepared", source.display()));
    }

    let executable_stamp = fs::metadata(&executable)
        .map_err(|error| format!("cannot inspect MestReNova executable: {error}"))?;
    let script = stage.0.join("export-spectrum.qs");
    fs::write(
        &script,
        script_contents(&source_copy, &stage.0.join("spectrum.pdf"), spectrum)?,
    )
    .map_err(|error| format!("cannot write Mnova export script: {error}"))?;
    let mut command = Command::new(&executable);
    command.arg(&script).args(["-sf", "exportSpectrum", "-w"]);
    if let Err(error) = run(command, &stage.0) {
        stage.retain();
        return Err(format!(
            "MestReNova could not export spectrum {} from {}: {error}; diagnostics retained at {}",
            spectrum,
            source.display(),
            stage.0.display()
        ));
    }
    if hash_file(source)? != source_hash {
        stage.retain();
        return Err(format!(
            "{} changed during Mnova export; diagnostics retained at {}",
            source.display(),
            stage.0.display()
        ));
    }

    let generated = stage.0.join("spectrum.pdf");
    if !is_valid_spectrum_pdf(&generated)? {
        stage.retain();
        return Err(format!(
            "MestReNova did not produce a valid PDF; diagnostics retained at {}",
            stage.0.display()
        ));
    }
    let source_stamp = fs::metadata(source).map_err(|error| error.to_string())?;
    let cached_key =
        cache_key_with_metadata(&source_hash, spectrum, &executable, &executable_stamp)?;
    if key != cached_key {
        stage.retain();
        return Err(format!(
            "MestReNova changed during export; diagnostics retained at {}",
            stage.0.display()
        ));
    }
    if source_stamp.len() > MAX_SPECTRUM_BYTES {
        return Err(format!(
            "{} exceeds the Mnova source size limit",
            source.display()
        ));
    }

    if cached.exists() {
        fs::remove_file(&cached).map_err(|error| {
            format!(
                "cannot replace invalid Mnova PDF cache {}: {error}",
                cached.display()
            )
        })?;
    }
    match fs::rename(&generated, &cached) {
        Ok(()) => Ok(cached),
        Err(_error) if cached_pdf(&cached)? => Ok(cached),
        Err(error) => {
            stage.retain();
            Err(format!(
                "cannot publish Mnova PDF cache {}: {error}; diagnostics retained at {}",
                cached.display(),
                stage.0.display()
            ))
        }
    }
}

pub fn spectrum_renderer_fingerprint() -> String {
    let unavailable = || format!("unavailable-{:x}", Sha256::digest(SCRIPT.as_bytes()));
    let Ok(executable) = tools::find("CHEMEX_MESTRENOVA", "MestReNova") else {
        return unavailable();
    };
    let Ok(metadata) = fs::metadata(&executable) else {
        return unavailable();
    };
    renderer_fingerprint(&executable, &metadata)
}

fn cache_key(
    source_hash: &str,
    spectrum: NonZeroUsize,
    executable: &Path,
) -> Result<String, String> {
    let metadata = fs::metadata(executable)
        .map_err(|error| format!("cannot inspect MestReNova executable: {error}"))?;
    cache_key_with_metadata(source_hash, spectrum, executable, &metadata)
}

fn cache_key_with_metadata(
    source_hash: &str,
    spectrum: NonZeroUsize,
    executable: &Path,
    metadata: &fs::Metadata,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(source_hash.as_bytes());
    hash.update(spectrum.get().to_le_bytes());
    hash.update(renderer_fingerprint(executable, metadata).as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

fn renderer_fingerprint(executable: &Path, metadata: &fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos());
    let mut hash = Sha256::new();
    hash.update(b"chemex-mnova-renderer-v2\0");
    hash.update(SCRIPT.as_bytes());
    hash.update(executable.to_string_lossy().as_bytes());
    hash.update(metadata.len().to_le_bytes());
    hash.update(modified.to_le_bytes());
    format!("{:x}", hash.finalize())
}

fn script_contents(source: &Path, output: &Path, spectrum: NonZeroUsize) -> Result<String, String> {
    let source = source
        .to_str()
        .ok_or_else(|| format!("Mnova cannot open a non-Unicode path: {}", source.display()))?;
    let output = output.to_str().ok_or_else(|| {
        format!(
            "Mnova cannot write a non-Unicode path: {}",
            output.display()
        )
    })?;
    let source = serde_json::to_string(source).map_err(|error| error.to_string())?;
    let output = serde_json::to_string(output).map_err(|error| error.to_string())?;
    Ok(SCRIPT
        .replace("__SOURCE_PATH__", &source)
        .replace("__OUTPUT_PATH__", &output)
        .replace("__SPECTRUM_INDEX__", &spectrum.to_string()))
}

fn hash_file(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_SPECTRUM_BYTES {
        return Err(format!(
            "{} exceeds the Mnova source size limit",
            path.display()
        ));
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn cached_pdf(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => is_valid_spectrum_pdf(path),
        Ok(_) => Err(format!(
            "Mnova cache path is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect Mnova PDF cache: {error}")),
    }
}

pub fn is_valid_spectrum_pdf(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !(15..=MAX_PDF_BYTES).contains(&metadata.len()) {
        return Ok(false);
    }
    let mut header = [0; 5];
    file.read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    if &header != b"%PDF-" {
        return Ok(false);
    }
    let tail_len = metadata.len().min(1024) as usize;
    file.seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(|error| error.to_string())?;
    let mut tail = vec![0; tail_len];
    file.read_exact(&mut tail)
        .map_err(|error| error.to_string())?;
    Ok(tail.trim_ascii_end().ends_with(b"%%EOF"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_are_json_escaped_and_spectrum_indices_are_explicit() {
        let source = PathBuf::from(r#"C:\paper\a "quoted".mnova"#);
        let output = PathBuf::from(r"C:\cache\spectrum.pdf");
        let script = script_contents(&source, &output, NonZeroUsize::new(2).unwrap()).unwrap();
        assert!(script.contains(r#"var sourcePath = "C:\\paper\\a \"quoted\".mnova";"#));
        assert!(script.contains("var requestedSpectrum = 2;"));
    }

    #[test]
    fn exporter_selects_one_spectrum_and_destroys_only_its_temporary_document() {
        assert!(SCRIPT.contains("serialization.open(sourcePath)"));
        assert!(SCRIPT.contains("new DocumentWindow(Application.mainWindow.newWindow())"));
        assert!(SCRIPT.contains("targetSpectrum.curSpecIndex = selected.spectrumIndex"));
        assert!(SCRIPT.contains("targetPage.deleteItems(remove)"));
        assert!(SCRIPT.contains("document.clonePages(selected.page)"));
        assert!(SCRIPT.contains("serialization.save(outputPath, \"pdf\")"));
        assert!(SCRIPT.contains("document.close()"));
        assert!(SCRIPT.contains("sourceWindow.close()"));
    }

    #[test]
    fn cache_key_changes_with_source_spectrum_or_executable() {
        let directory = std::env::temp_dir().join(format!("chemex-mnova-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.exe");
        let second = directory.join("second.exe");
        fs::write(&first, b"renderer").unwrap();
        fs::write(&second, b"renderer").unwrap();
        let index = NonZeroUsize::new(1).unwrap();
        let original = cache_key("source", index, &first).unwrap();
        assert_ne!(original, cache_key("changed", index, &first).unwrap());
        assert_ne!(
            original,
            cache_key("source", NonZeroUsize::new(2).unwrap(), &first).unwrap()
        );
        assert_ne!(original, cache_key("source", index, &second).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
}
