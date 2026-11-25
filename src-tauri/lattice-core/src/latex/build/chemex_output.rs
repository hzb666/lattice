use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::chemistry;
use crate::latex::aux_files::{self, AuxFiles, SYNCTEX};
use crate::latex::chemex::{Preparation, PreparedBuild};
use crate::util::err;
use crate::{project, project_fs::ProjectDir};

pub(super) struct Staging {
    folder: PathBuf,
    aux: PathBuf,
    stem: String,
}

impl Staging {
    pub(super) fn new(
        root: &Path, document: &str, layout: AuxFiles, preparation: &Preparation<'_>,
    ) -> Result<Self, String> {
        let stem = Path::new(document)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("Missing Chemex document name")?;
        Ok(Self {
            folder: preparation.output_dir()?,
            aux: aux_files::folder(root, document, layout),
            stem: stem.to_owned(),
        })
    }

    pub(super) fn configure(&self, command: &mut Command) {
        #[cfg(windows)]
        let folder = crate::host_paths::tex_path(&self.folder);
        #[cfg(not(windows))]
        let folder = self.folder.to_string_lossy();
        command.arg(format!("-outdir={folder}"));
    }

    fn file(&self, extension: &str) -> PathBuf {
        self.folder.join(format!("{}.{extension}", self.stem))
    }

    pub(super) fn agrees(&self, prepared: &PreparedBuild, skipped: bool) -> bool {
        self.file("pdf").is_file()
            && if skipped {
                !prepared.reused && prepared.accepted()
            } else {
                prepared.agrees(&self.aux, &self.folder, &self.stem)
            }
    }

    pub(super) fn publish(
        &self, root: &Path, document: &str, layout: AuxFiles,
    ) -> Result<(), String> {
        let bytes = fs::read(self.file("pdf")).map_err(err)?;
        let relative = Path::new(document).with_extension("pdf");
        let directory = ProjectDir::open(root)?;
        let map = SYNCTEX.iter().find_map(|extension| {
            let path = self.file(extension);
            path.is_file().then_some((path, *extension))
        });
        if let Some((source, extension)) = &map {
            let destination = aux_files::in_layout(Path::new(""), document, extension, layout);
            directory
                .atomic_write(&destination.to_string_lossy(), &fs::read(source).map_err(err)?)?;
        }
        directory.atomic_write(&relative.to_string_lossy(), &bytes)?;
        for extension in SYNCTEX {
            if map.as_ref().is_none_or(|(_, current)| *current != extension) {
                let target = aux_files::in_layout(Path::new(""), document, extension, layout);
                if let Ok(path) = project::safe_path(root, &target.to_string_lossy()) {
                    fs::remove_file(path).map_err(err)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn copy_recorder(
        &self, root: &Path, document: &str, layout: AuxFiles,
    ) -> Result<(), String> {
        let recorder = self.file("fls");
        if recorder.is_file() {
            let relative = aux_files::in_layout(Path::new(""), document, "fls", layout);
            let destination = project::creation_path(root, &relative.to_string_lossy())?;
            chemistry::write_if_changed(&destination, &fs::read(recorder).map_err(err)?)?;
        }
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        for directory in [&self.aux, &self.folder] {
            for extension in ["chemex-figures.dat", "chemex-numbers.dat"] {
                let _ = fs::remove_file(directory.join(format!("{}.{extension}", self.stem)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn verification_reports_are_removed_after_the_build() {
        let root = TempDir::new("chemex-output-reports");
        let aux = root.join("aux");
        let output = root.join("output");
        fs::create_dir_all(&aux).unwrap();
        fs::create_dir_all(&output).unwrap();
        let staging = Staging { folder: output.clone(), aux: aux.clone(), stem: "main".into() };
        for directory in [&aux, &output] {
            fs::write(directory.join("main.chemex-figures.dat"), "report").unwrap();
            fs::write(directory.join("main.chemex-numbers.dat"), "report").unwrap();
        }

        drop(staging);

        for directory in [&aux, &output] {
            assert!(!directory.join("main.chemex-figures.dat").exists());
            assert!(!directory.join("main.chemex-numbers.dat").exists());
        }
    }
}
