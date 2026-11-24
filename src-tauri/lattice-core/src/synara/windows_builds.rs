use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::latex::{self, ActiveBuild, AuxFiles, BuildResult, BuildSteps};

use super::windows_sandbox::tex::Preview;
use super::windows_workspace::{file_identity, tex_outputs};
use super::{bibliography_guard, SynaraRuntime};

const BUILD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct ProjectBuild<'a> {
    pub root: &'a Path,
    pub force: bool,
    pub unattended: bool,
    pub active: &'a ActiveBuild,
    pub open_document: Option<&'a str>,
    pub steps: &'a BuildSteps,
    pub layout: AuxFiles,
}

impl SynaraRuntime {
    pub fn with_project_export_tex<T>(
        &self, root: &Path,
        work: impl FnOnce(Option<&crate::latex::ExportTex<'_>>) -> Result<T, String>,
    ) -> Result<T, String> {
        let tex = self.project_tex(root)?;
        if !tex_isolated(tex.as_ref())? {
            return work(None);
        }
        let control = self.control_dir();
        let runtime = self
            .server_entry
            .ancestors()
            .nth(3)
            .ok_or("The bundled agent runtime has no directory.")?;
        let sandbox = Preview { runtime, control: &control, project: root };
        let run = |command, scratch: &Path, timeout, cancelled: &dyn Fn() -> bool| {
            sandbox
                .run_export(command, scratch, timeout, cancelled)
                .map_err(|error| error.to_string())
        };
        work(Some(&run))
    }

    pub fn build_project(&self, request: ProjectBuild<'_>) -> Result<BuildResult, String> {
        let tex = self.project_tex(request.root)?;
        if tex_isolated(tex.as_ref())? {
            return self.build_isolated(request);
        }
        latex::build(
            request.root,
            request.force,
            request.unattended,
            request.active,
            request.open_document,
            request.steps,
            request.layout,
        )
    }

    pub fn clean_project_build(
        &self, root: &Path, open_document: Option<&str>,
    ) -> Result<String, String> {
        let tex = self.project_tex(root)?;
        if tex_isolated(tex.as_ref())? {
            let document = latex::project_build_target(root, open_document)?;
            tex_outputs::clean(root, &document)?;
            return Ok(String::new());
        }
        latex::clean(root, open_document)
    }

    fn build_isolated(&self, request: ProjectBuild<'_>) -> Result<BuildResult, String> {
        let control = self.control_dir();
        let sandbox = Preview {
            runtime: self
                .server_entry
                .ancestors()
                .nth(3)
                .ok_or("The bundled agent runtime has no directory.")?,
            control: &control,
            project: request.root,
        };
        latex::isolated_project_build(
            &request,
            |command, output| {
                sandbox
                    .run_build(
                        command,
                        output,
                        BUILD_TIMEOUT,
                        || latex::stop_requested(request.active),
                        request.steps,
                    )
                    .map_err(|error| error.to_string())
            },
            |output, document, layout| tex_outputs::publish(request.root, output, document, layout),
        )
    }

    fn project_tex(&self, root: &Path) -> Result<Option<ProjectTex>, String> {
        self.lock_project_tex(root, bibliography_guard::AVAILABLE)
            .map_err(|error| error.to_string())
    }

    pub(super) fn lock_project_tex(
        &self, root: &Path, create: bool,
    ) -> io::Result<Option<ProjectTex>> {
        ProjectTex::lock(&self.control_dir().join("tex-projects"), root, create)
    }
}

fn tex_isolated(project: Option<&ProjectTex>) -> Result<bool, String> {
    project.map_or(Ok(false), ProjectTex::isolated).map_err(|error| error.to_string())
}

pub(super) struct ProjectTex {
    marker: File,
    _root: File,
}

impl ProjectTex {
    fn lock(directory: &Path, root: &Path, create: bool) -> io::Result<Option<Self>> {
        if create {
            fs::create_dir_all(directory)?;
        } else if !directory.try_exists()? {
            return Ok(None);
        }
        let root = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)?;
        let id = file_identity(&root)?;
        let path = directory.join(format!("{:08x}-{:016x}.lock", id.volume, id.index));
        let marker = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        marker.lock()?;
        Ok(Some(Self { marker, _root: root }))
    }

    fn isolated(&self) -> io::Result<bool> {
        Ok(self.marker.metadata()?.len() != 0)
    }

    pub(super) fn enroll(&self) -> io::Result<()> {
        if !self.isolated()? {
            self.marker.set_len(1)?;
            self.marker.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrolled_project_stays_isolated_after_renaming_and_reopening() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = fixture.path().join("control");
        let original = fixture.path().join("project");
        fs::create_dir(&original).unwrap();
        assert!(ProjectTex::lock(&directory, &original, false).unwrap().is_none());

        let project = ProjectTex::lock(&directory, &original, true).unwrap().unwrap();
        assert!(!project.isolated().unwrap());
        project.enroll().unwrap();
        drop(project);

        let renamed = fixture.path().join("renamed");
        fs::rename(original, &renamed).unwrap();
        let reopened = ProjectTex::lock(&directory, &renamed, false).unwrap().unwrap();
        assert!(reopened.isolated().unwrap());
    }
}
