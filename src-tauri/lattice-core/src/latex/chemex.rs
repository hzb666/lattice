//! Prepare ChemDraw assets before the ordinary, incremental latexmk run.

use super::{build, search_paths, ActiveBuild, BuildSteps};
use crate::util::{err, sha256_hex};
use crate::{chemistry, commands, host_paths, project};
use chemex_cdxml::{DrawingDocument, FieldValues};
use chemex_cli::{self as tex, DrawingRegistration, FigureRegistration, FigureRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use walkdir::WalkDir;

const REQUESTS: &str = "chemex-figures.dat";
const NUMBERS: &str = "chemex-numbers.dat";
const REGISTER: &str = "chemex-register.tex";
const VERIFIED: &[u8] = b"\\ChemexPreparedMode\n\\ChemexVerifyMode\n";

#[derive(Deserialize, Serialize)]
struct PreparedState {
    key: String,
    non_tex: String,
    non_figures: String,
    requests: Vec<FigureRequest>,
    fields: Vec<Vec<String>>,
    numbers: BTreeMap<String, String>,
}

struct Fingerprints {
    key: String,
    non_tex: String,
    non_figures: String,
}

pub(super) struct Inputs {
    folder: PathBuf,
    assets: PathBuf,
    spectra: Option<PathBuf>,
}

pub(super) struct PreparedBuild {
    pub inputs: Inputs,
    state: PreparedState,
    key: String,
    pub reused: bool,
    assets: Option<Vec<String>>,
}

impl PreparedBuild {
    pub fn agrees(&self, aux: &Path, output: &Path, stem: &str) -> bool {
        let Some((_, directory)) = [aux, output]
            .into_iter()
            .filter_map(|directory| {
                let figures = directory.join(format!("{stem}.{REQUESTS}"));
                let numbers = directory.join(format!("{stem}.{NUMBERS}"));
                let modified = figures
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .max(numbers.metadata().ok()?.modified().ok()?);
                Some((modified, directory))
            })
            .max_by_key(|(modified, _)| *modified)
        else {
            return false;
        };
        let requests = tex::read_figures(&directory.join(format!("{stem}.{REQUESTS}")));
        let numbers = tex::read_numbers(&directory.join(format!("{stem}.{NUMBERS}")));
        requests.is_ok_and(|requests| requests == self.state.requests)
            && numbers.is_ok_and(|numbers| numbers == self.state.numbers)
    }

    pub(super) fn accepted(&self) -> bool {
        self.assets.is_none()
    }
}

impl Inputs {
    pub(super) fn configure(&self, command: &mut Command) -> Result<(), String> {
        prepend_inputs(
            command,
            [self.folder.as_path(), self.assets.as_path()]
                .into_iter()
                .chain(self.spectra.as_deref()),
        )
    }
}

pub(super) fn enabled(source: &Path) -> bool {
    fs::read(source).is_ok_and(|bytes| {
        bytes.split(|byte| *byte == b'\n').any(|line| {
            let text = line.split(|byte| *byte == b'%').next().unwrap_or_default();
            text.windows(b"chemex".len()).any(|word| word == b"chemex")
                || [b".cdxml".as_slice(), b".mnova".as_slice()].iter().any(|extension| {
                    text.windows(extension.len()).any(|word| word.eq_ignore_ascii_case(extension))
                })
        })
    })
}

pub(super) fn enabled_project(root: &Path, document: &str) -> Result<bool, String> {
    let root = root.canonicalize().map_err(err)?;
    let root_document = project::safe_path(&root, document)?;
    let search_base = root_document.parent().unwrap_or(&root).to_path_buf();
    let mut pending = vec![root_document];
    let mut seen = std::collections::BTreeSet::new();
    let inputs =
        regex::Regex::new(r"\\(?:input|include)(?:\s*\{([^{}]+)\}|\s+([^\s%{}]+))").map_err(err)?;
    while let Some(source) = pending.pop() {
        if !seen.insert(source.clone()) {
            continue;
        }
        if enabled(&source) {
            return Ok(true);
        }
        let contents = fs::read_to_string(&source).map_err(err)?;
        for line in contents.lines() {
            let line = super::strip_comment(line);
            for capture in inputs.captures_iter(&line) {
                let Some(name) = capture.get(1).or_else(|| capture.get(2)) else { continue };
                for base in [search_base.as_path(), root.as_path()] {
                    let Ok(mut relative) = relative_input(&root, base, name.as_str()) else {
                        continue;
                    };
                    if relative.extension().is_none() {
                        relative.set_extension("tex");
                    }
                    let Some(relative) = relative.to_str() else { continue };
                    let Ok(candidate) = project::safe_path(&root, relative) else { continue };
                    if candidate.is_file() {
                        pending.push(candidate);
                        break;
                    }
                }
            }
        }
    }
    Ok(false)
}

fn relative_input(root: &Path, base: &Path, name: &str) -> Result<PathBuf, String> {
    let input = Path::new(name);
    if input.is_absolute() {
        return Err("TeX input must stay inside the project".into());
    }
    let mut relative = PathBuf::new();
    for part in base.strip_prefix(root).map_err(err)?.components().chain(input.components()) {
        match part {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir if relative.pop() => {}
            _ => return Err("TeX input must stay inside the project".into()),
        }
    }
    Ok(relative)
}

pub(super) struct Preparation<'a> {
    pub root: &'a Path,
    pub document: &'a str,
    pub engine: &'a str,
    pub active: &'a ActiveBuild,
    pub steps: &'a BuildSteps,
}

struct TexPasses<'a> {
    scratch: Option<&'a Path>,
    run: &'a dyn Fn(Command, &Path) -> Result<(Output, bool), String>,
}

impl Preparation<'_> {
    pub(super) fn run(&self, reuse: bool) -> Result<PreparedBuild, String> {
        self.prepare(
            &TexPasses { scratch: None, run: &|command, _| self.track_tool(command) },
            reuse,
        )
    }

    #[cfg(windows)]
    pub(super) fn run_isolated(
        &self, run: &impl Fn(Command, &Path) -> Result<(Output, bool), String>,
    ) -> Result<Inputs, String> {
        let scratch = tempfile::tempdir().map_err(err)?;
        self.prepare(&TexPasses { scratch: Some(scratch.path()), run }, false)
            .map(|prepared| prepared.inputs)
    }

    fn prepare(&self, passes: &TexPasses<'_>, reuse: bool) -> Result<PreparedBuild, String> {
        let manifest = project::read_manifest(self.root)?;
        let fingerprints = self.fingerprints(&manifest)?;
        let key = &fingerprints.key;
        let namespace = sha256_hex(self.document);
        let mut previous = self.prepared_state().ok();
        if let Some(mut state) = previous.take() {
            if can_reuse_inputs(&state, &fingerprints, reuse) {
                let cache_key = state.key.clone();
                if let Some(inputs) = cached_inputs(self.root, &namespace, &cache_key)? {
                    let reused = state.key != *key;
                    update_fingerprints(&mut state, &fingerprints);
                    return Ok(PreparedBuild {
                        inputs,
                        state,
                        key: key.clone(),
                        reused,
                        assets: None,
                    });
                }
            }
            previous = Some(state);
        }
        let saved_numbering =
            previous.filter(|state| can_reuse_numbering(state, &fingerprints, reuse));
        let (figures, saved_numbers) = self.figure_inputs(&namespace, passes, saved_numbering)?;
        if figures.items.is_empty() {
            return Err("The LaTeX project contains no Chemex figure references".into());
        }
        let mut registrations = figures.registrations()?;
        let fields = field_keys(&registrations);
        let reused = saved_numbers.is_some();
        let (values, numbers) = match saved_numbers {
            Some(numbers) => (tex::field_values(&fields.concat(), &numbers).map_err(err)?, numbers),
            None => self.number(&namespace, &figures, &registrations, passes)?,
        };
        let requests: Vec<_> = figures.items.iter().map(|figure| figure.request.clone()).collect();
        let sources: Vec<_> =
            figures.documents.values().map(|document| document.source().to_owned()).collect();
        chemex_cli::attach_mnova_formulas(&requests, &mut registrations, &sources, &numbers)?;
        let mut assets = Vec::with_capacity(figures.items.len());
        let mut spectra = None;
        for (index, figure) in figures.items.iter().enumerate() {
            let (pdf, source_hash, detail) = match &figure.request {
                FigureRequest::ChemDraw { path, .. } => {
                    let drawing = figures
                        .documents
                        .get(path)
                        .ok_or("Chemex CDXML request has no parsed drawing")?;
                    let rendered = drawing.render_fields(&values).map_err(err)?;
                    let rendered_path = chemistry::cache_file(
                        self.root,
                        &format!("{namespace}/drawings/drawing-{:03}.cdxml", index + 1),
                    )?;
                    let (svg, pdf) = chemistry::pdf(self.root, &rendered, |command| {
                        self.run_tool("ChemDraw", command)
                    })
                    .map_err(|error| format!("{}: {error}", figure.request.path()))?;
                    chemistry::write_if_changed(&rendered_path, rendered.as_bytes())?;
                    let detail = svg
                        .file_stem()
                        .and_then(OsStr::to_str)
                        .ok_or("Missing Chemex SVG key")?
                        .to_owned();
                    (pdf, sha256_hex(drawing.source()), detail)
                }
                FigureRequest::Mnova { spectrum, .. } => {
                    let pdf = chemistry::export_spectrum_pdf(
                        self.root,
                        &figure.path,
                        *spectrum,
                        |mut command, directory| {
                            command.current_dir(directory);
                            self.run_tool("MestReNova", command)
                        },
                    )
                    .map_err(|error| format!("{}: {error}", figure.request.path()))?;
                    spectra = pdf.parent().map(Path::to_path_buf);
                    (pdf, hash_file(&figure.path)?, spectrum.to_string())
                }
            };
            let asset = pdf
                .file_name()
                .and_then(OsStr::to_str)
                .ok_or("Missing Chemex asset filename")?
                .to_owned();
            assets.push(format!("{asset} {source_hash} {detail}"));
            match &mut registrations[index] {
                FigureRegistration::ChemDraw(drawing) => drawing.asset = asset,
                FigureRegistration::Mnova { asset: target, .. } => *target = asset,
            }
        }
        let folder = self.pass_folder(&namespace, "final", VERIFIED, None)?;
        let registration = tex::prepared_registrations(&registrations, &numbers).map_err(err)?;
        chemistry::write_if_changed(&folder.join(REGISTER), registration.as_bytes())?;
        let inputs = Inputs { folder, assets: assets_folder(self.root)?, spectra };
        if self.fingerprints(&manifest)?.key != *key {
            return Err(
                "LaTeX or figure sources changed during Chemex preparation; build again".into()
            );
        }
        let state = PreparedState {
            key: key.clone(),
            non_tex: fingerprints.non_tex,
            non_figures: fingerprints.non_figures,
            requests,
            fields,
            numbers,
        };
        if passes.scratch.is_some() {
            self.write_state(&state, &assets, &registration)?;
        }
        Ok(PreparedBuild { inputs, state, key: key.clone(), reused, assets: Some(assets) })
    }

    fn figure_inputs(
        &self, namespace: &str, passes: &TexPasses<'_>, saved: Option<PreparedState>,
    ) -> Result<(Figures, Option<BTreeMap<String, String>>), String> {
        if let Some(state) = saved {
            if let Ok(figures) = Figures::load(self.root, self.document, state.requests) {
                if field_keys(&figures.registrations()?) == state.fields {
                    return Ok((figures, Some(state.numbers)));
                }
            }
        }
        let discovery =
            self.pass_folder(namespace, "discover", b"\\ChemexDiscoveryMode\n", passes.scratch)?;
        Ok((self.discover(&discovery, passes)?, None))
    }

    fn state_path(&self) -> Result<PathBuf, String> {
        chemistry::cache_file(self.root, &format!("{}/prepared.json", sha256_hex(self.document)))
    }

    fn prepared_state(&self) -> Result<PreparedState, String> {
        let state: PreparedState =
            serde_json::from_slice(&chemistry::read_asset(&self.state_path()?)?).map_err(err)?;
        let ready = chemistry::ready_path(self.root, &sha256_hex(self.document))?;
        let marker = fs::read_to_string(ready).map_err(err)?;
        if marker.lines().next() != Some(state.key.as_str()) {
            return Err("Chemex preparation cache is incomplete".into());
        }
        Ok(state)
    }

    fn write_state(
        &self, state: &PreparedState, assets: &[String], registration: &str,
    ) -> Result<(), String> {
        chemistry::write_if_changed(&self.state_path()?, &serde_json::to_vec(state).map_err(err)?)?;
        chemistry::write_if_changed(
            &chemistry::ready_path(self.root, &sha256_hex(self.document))?,
            format!("{}\n{}\n{}", state.key, sha256_hex(registration), assets.join("\n"))
                .as_bytes(),
        )
    }

    pub(super) fn still_current(&self, prepared: &PreparedBuild) -> Result<bool, String> {
        Ok(self.fingerprints(&project::read_manifest(self.root)?)?.key == prepared.key)
    }

    pub(super) fn accept(&self, mut prepared: PreparedBuild) -> Result<(), String> {
        if !prepared.reused && prepared.assets.is_none() {
            return Ok(());
        }
        let registration =
            fs::read_to_string(prepared.inputs.folder.join(REGISTER)).map_err(err)?;
        let assets = match prepared.assets {
            Some(assets) => assets,
            None => {
                let ready = chemistry::ready_path(self.root, &sha256_hex(self.document))?;
                let marker = fs::read_to_string(ready).map_err(err)?;
                marker.lines().skip(2).map(str::to_owned).collect()
            }
        };
        prepared.state.key = prepared.key;
        self.write_state(&prepared.state, &assets, &registration)
    }

    pub(super) fn output_dir(&self) -> Result<PathBuf, String> {
        let namespace = sha256_hex(self.document);
        let path = chemistry::cache_file(self.root, &format!("{namespace}/output/.keep"))?;
        Ok(path.parent().ok_or("Missing Chemex output folder")?.to_path_buf())
    }

    #[cfg(test)]
    fn fingerprint(&self, manifest: &crate::models::ProjectManifest) -> Result<String, String> {
        Ok(self.fingerprints(manifest)?.key)
    }

    fn fingerprints(
        &self, manifest: &crate::models::ProjectManifest,
    ) -> Result<Fingerprints, String> {
        let root = self.root.canonicalize().map_err(err)?;
        let mut hash = Sha256::new();
        hash.update(self.document.as_bytes());
        hash.update([0]);
        hash.update(self.engine.as_bytes());
        hash.update([0]);
        hash.update(manifest.trusted.to_string().as_bytes());
        hash.update([0]);
        hash.update(b"chemex-preparation-v4");
        hash.update([0]);
        hash.update(tex::PACKAGE.as_bytes());
        let mut files = dependency_files(&root)?;
        if files.iter().any(|path| {
            path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("mnova"))
        }) {
            hash.update(chemex_cli::spectrum_renderer_fingerprint().as_bytes());
            hash.update([0]);
        }
        files.push(project::safe_path(&root, self.document)?);
        files.sort();
        files.dedup();
        let (mut non_tex, mut non_figures) = (hash.clone(), hash.clone());
        for path in files {
            let relative = path.strip_prefix(&root).map_err(err)?.to_string_lossy();
            let digest = hash_file(&path)?;
            let add = |hash: &mut Sha256| {
                hash.update((relative.len() as u64).to_le_bytes());
                hash.update(relative.as_bytes());
                hash.update(digest.as_bytes());
            };
            add(&mut hash);
            if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("tex")) {
                add(&mut non_tex);
            }
            if !(chemistry::is_cdxml(&path) || chemistry::is_mnova(&path)) {
                add(&mut non_figures);
            }
        }
        Ok(Fingerprints {
            key: format!("{:x}", hash.finalize()),
            non_tex: format!("{:x}", non_tex.finalize()),
            non_figures: format!("{:x}", non_figures.finalize()),
        })
    }

    fn pass_folder(
        &self, namespace: &str, pass: &str, mode: &[u8], scratch: Option<&Path>,
    ) -> Result<PathBuf, String> {
        let mode_file = if let Some(scratch) = scratch {
            let folder = scratch.join(pass);
            fs::create_dir(&folder).map_err(err)?;
            folder.join("chemex-mode.tex")
        } else {
            chemistry::cache_file(self.root, &format!("{namespace}/{pass}/chemex-mode.tex"))?
        };
        let folder = mode_file.parent().ok_or("Missing Chemex preparation directory")?;
        chemistry::write_if_changed(&folder.join("chemex.sty"), tex::PACKAGE.as_bytes())?;
        chemistry::write_if_changed(&mode_file, mode)?;
        Ok(folder.to_path_buf())
    }

    fn discover(&self, folder: &Path, passes: &TexPasses<'_>) -> Result<Figures, String> {
        self.pass("Chemex discovery", folder, passes)?;
        let requests = match fs::symlink_metadata(folder.join(REQUESTS)) {
            Ok(info) if info.is_file() => tex::read_figures(&folder.join(REQUESTS)).map_err(err)?,
            Ok(_) => return Err("Invalid Chemex figure request file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(err(error)),
        };
        Figures::load(self.root, self.document, requests)
    }

    fn number(
        &self, namespace: &str, figures: &Figures, registrations: &[FigureRegistration],
        passes: &TexPasses<'_>,
    ) -> Result<(FieldValues, BTreeMap<String, String>), String> {
        let fields: Vec<_> = registrations
            .iter()
            .filter_map(|registration| match registration {
                FigureRegistration::ChemDraw(drawing) => Some(drawing.fields.iter().cloned()),
                FigureRegistration::Mnova { .. } => None,
            })
            .flatten()
            .collect();
        if fields.is_empty() {
            return Ok((FieldValues::new(), BTreeMap::new()));
        }
        let folder =
            self.pass_folder(namespace, "number", b"\\ChemexNumberingMode\n", passes.scratch)?;
        write_registrations(&folder, registrations)?;
        self.pass("Chemex numbering", &folder, passes)?;
        let numbered = tex::read_figures(&folder.join(REQUESTS)).map_err(err)?;
        let expected: Vec<_> = figures.items.iter().map(|item| item.request.clone()).collect();
        tex::validate_figures(&expected, &numbered, "numbering").map_err(err)?;
        let numbers = tex::read_numbers(&folder.join(NUMBERS)).map_err(err)?;
        let values = tex::field_values(&fields, &numbers).map_err(err)?;
        Ok((values, numbers))
    }

    fn pass(&self, name: &str, folder: &Path, passes: &TexPasses<'_>) -> Result<(), String> {
        let manifest = project::read_manifest(self.root)?;
        let mut command = commands::command("latexmk");
        let path = tex_path(folder);
        command
            .current_dir(self.root)
            .args([
                "-cd",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-no-shell-escape",
                "-file-line-error",
                "-pv-",
                "-pvc-",
                "-g",
            ])
            .arg(format!("-outdir={path}"))
            .arg(format!("-auxdir={path}"))
            .args(super::build::engine_args(self.root, &manifest))
            .arg(self.document);
        search_paths::search_project_folder(&mut command, self.document);
        prepend_inputs(&mut command, [folder])?;
        self.steps.begin(name, true);
        check_tool(name, (passes.run)(command, folder)?)
    }

    fn run_tool(&self, name: &str, command: Command) -> Result<(), String> {
        self.steps.begin(name, true);
        check_tool(name, self.track_tool(command)?)
    }

    fn track_tool(&self, command: Command) -> Result<(Output, bool), String> {
        build::run_tracked(
            command,
            self.active,
            self.steps,
            "Could not start Chemex tool: ",
            "Chemex tool stopped: ",
        )
    }
}

fn check_tool(name: &str, (output, cancelled): (Output, bool)) -> Result<(), String> {
    if cancelled {
        return Err("Chemex preparation was stopped".into());
    }
    if !output.status.success() {
        let log = commands::combined_output(&output);
        let tail = log.lines().rev().take(25).collect::<Vec<_>>();
        return Err(format!(
            "{name} failed ({})\n{}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(())
}

fn dependency_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let root = root.canonicalize().map_err(err)?;
    let mut files = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_entry(|entry| {
        entry.depth() == 0
            || !(crate::project::is_internal_project_component(entry.file_name())
                || matches!(entry.file_name().to_str(), Some("node_modules" | "target" | "build")))
    }) {
        let entry = entry.map_err(err)?;
        let extension = entry.path().extension().and_then(OsStr::to_str).unwrap_or_default();
        if entry.file_type().is_file()
            && (["tex", "sty", "cls", "def", "cfg", "cdxml", "mnova", "bib"]
                .iter()
                .any(|kind| extension.eq_ignore_ascii_case(kind))
                || matches!(entry.file_name().to_str(), Some("latexmkrc" | ".latexmkrc")))
        {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(err)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(err)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn cached_inputs(root: &Path, namespace: &str, key: &str) -> Result<Option<Inputs>, String> {
    let ready = chemistry::ready_path(root, namespace)?;
    if !ready.is_file() {
        return Ok(None);
    }
    let folder = chemistry::cache_file(root, &format!("{namespace}/final/{REGISTER}"))?
        .parent()
        .ok_or("Missing Chemex registration folder")?
        .to_path_buf();
    if !folder.join("chemex.sty").is_file() || !folder.join("chemex-mode.tex").is_file() {
        return Ok(None);
    }
    let Ok(registrations) = fs::read(folder.join(REGISTER)) else {
        return Ok(None);
    };
    let assets = assets_folder(root)?;
    let ready = fs::read_to_string(ready).map_err(err)?;
    let mut lines = ready.lines();
    if lines.next() != Some(key) {
        return Ok(None);
    }
    if lines.next() != Some(sha256_hex(&registrations).as_str()) {
        return Ok(None);
    }
    if lines.clone().next().is_none() {
        return Ok(None);
    }
    let mut spectra = None;
    for (index, line) in lines.enumerate() {
        let valid = if let Some(asset) = chemistry::Asset::parse(line) {
            if chemistry::pdf_key(asset.pdf).is_some() {
                asset_cached(&assets, asset.pdf, asset.detail)
            } else {
                if spectra.is_none() {
                    spectra = Some(spectrum_assets_folder(root)?);
                }
                spectra
                    .as_deref()
                    .is_some_and(|folder| chemistry::cached_spectrum_pdf(folder, asset.pdf))
            }
        } else if chemistry::pdf_key(line).is_some() {
            let path = chemistry::cache_file(
                root,
                &format!("{namespace}/drawings/drawing-{:03}.cdxml", index + 1),
            )?;
            chemistry::read_asset(&path)
                .is_ok_and(|source| asset_cached(&assets, line, &sha256_hex(source)))
        } else {
            false
        };
        if !valid {
            return Ok(None);
        }
    }
    Ok(Some(Inputs { folder, assets, spectra }))
}

fn asset_cached(folder: &Path, name: &str, numbered: &str) -> bool {
    chemistry::read_asset(&folder.join(name)).is_ok_and(|bytes| chemistry::valid_pdf(&bytes))
        && chemistry::read_svg(&folder.join(format!("{numbered}.svg")))
            .is_some_and(|bytes| chemistry::pdf_key(name) == Some(sha256_hex(&bytes).as_str()))
}

fn assets_folder(root: &Path) -> Result<PathBuf, String> {
    let file = chemistry::cache_file(root, "assets/.keep")?;
    Ok(file.parent().ok_or("Missing Chemex asset directory")?.to_path_buf())
}

fn spectrum_assets_folder(root: &Path) -> Result<PathBuf, String> {
    let file = chemistry::spectrum_cache_file(root, ".keep")?;
    Ok(file.parent().ok_or("Missing MestReNova cache directory")?.to_path_buf())
}

struct LoadedFigure {
    request: FigureRequest,
    path: PathBuf,
}

struct Figures {
    items: Vec<LoadedFigure>,
    documents: BTreeMap<String, DrawingDocument>,
}

impl Figures {
    fn load(root: &Path, document: &str, requests: Vec<FigureRequest>) -> Result<Self, String> {
        let mut documents = BTreeMap::new();
        let mut items = Vec::with_capacity(requests.len());
        for request in &requests {
            let extension = match request {
                FigureRequest::ChemDraw { .. } => "cdxml",
                FigureRequest::Mnova { .. } => "mnova",
            };
            let path = figure_path(root, document, request.path(), extension)?;
            if let FigureRequest::ChemDraw { path: name, .. } = request {
                if !documents.contains_key(name) {
                    let drawing = chemistry::read_document(&path)
                        .map_err(|error| format!("{}: {error}", request.path()))?;
                    documents.insert(name.clone(), drawing);
                }
            }
            items.push(LoadedFigure { request: request.clone(), path });
        }
        Ok(Self { items, documents })
    }

    fn registrations(&self) -> Result<Vec<FigureRegistration>, String> {
        self.items
            .iter()
            .map(|figure| match &figure.request {
                FigureRequest::ChemDraw { counter, path } => {
                    let drawing = self
                        .documents
                        .get(path)
                        .ok_or("Chemex CDXML request has no parsed drawing")?;
                    Ok(FigureRegistration::ChemDraw(DrawingRegistration {
                        counter: counter.clone(),
                        fields: drawing
                            .fields()
                            .iter()
                            .map(|field| field.compound_id.clone())
                            .collect(),
                        asset: String::new(),
                    }))
                }
                FigureRequest::Mnova { spectrum, compound, x, y, size, .. } => {
                    Ok(FigureRegistration::Mnova {
                        spectrum: *spectrum,
                        compound: compound.clone(),
                        x: *x,
                        y: *y,
                        size: size.clone(),
                        formula: None,
                        asset: String::new(),
                    })
                }
            })
            .collect()
    }
}

fn figure_path(
    root: &Path, document: &str, name: &str, extension: &str,
) -> Result<PathBuf, String> {
    let relative = Path::new(name);
    if relative.is_absolute() {
        return Err(format!("Chemex figure path must stay inside the project: {name}"));
    }
    if !relative.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(extension)) {
        return Err(format!("Expected a .{extension} figure: {name}"));
    }
    let folder = Path::new(document).parent().unwrap_or(Path::new(""));
    let mut path = folder.to_path_buf();
    for part in relative.components() {
        match part {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            Component::ParentDir if path.pop() => {}
            _ => return Err(format!("CDXML must stay inside the project: {name}")),
        }
    }
    let beside = project::safe_path(root, path.to_str().ok_or("Invalid CDXML path")?);
    if beside.as_ref().is_ok_and(|path| path.is_file())
        || !project::stays_inside(relative)
        || name.starts_with("./")
    {
        return beside;
    }
    project::safe_path(root, name)
}

fn write_registrations(folder: &Path, registrations: &[FigureRegistration]) -> Result<(), String> {
    chemistry::write_if_changed(
        &folder.join(REGISTER),
        tex::registrations(registrations).as_bytes(),
    )
}

fn field_keys(registrations: &[FigureRegistration]) -> Vec<Vec<String>> {
    registrations
        .iter()
        .map(|registration| match registration {
            FigureRegistration::ChemDraw(drawing) => drawing.fields.clone(),
            FigureRegistration::Mnova { .. } => Vec::new(),
        })
        .collect()
}

fn can_reuse_inputs(state: &PreparedState, fingerprints: &Fingerprints, reuse: bool) -> bool {
    reuse && (state.key == fingerprints.key || state.non_tex == fingerprints.non_tex)
}

fn can_reuse_numbering(state: &PreparedState, fingerprints: &Fingerprints, reuse: bool) -> bool {
    reuse && state.non_figures == fingerprints.non_figures
}

fn update_fingerprints(state: &mut PreparedState, fingerprints: &Fingerprints) {
    state.key.clone_from(&fingerprints.key);
    state.non_tex.clone_from(&fingerprints.non_tex);
    state.non_figures.clone_from(&fingerprints.non_figures);
}

fn prepend_inputs<'a>(
    command: &mut Command, paths: impl IntoIterator<Item = &'a Path>,
) -> Result<(), String> {
    let inherited = command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("TEXINPUTS"))
        .and_then(|(_, value)| value.map(OsStr::to_owned))
        .or_else(|| std::env::var_os("TEXINPUTS"))
        .unwrap_or_default();
    let mut value =
        std::env::join_paths(paths.into_iter().map(|path| PathBuf::from(tex_path(path))))
            .map_err(err)?;
    value.push(if cfg!(windows) { ";" } else { ":" });
    value.push(OsString::from(inherited));
    command.env("TEXINPUTS", value);
    Ok(())
}

fn tex_path(path: &Path) -> String {
    #[cfg(windows)]
    return host_paths::tex_path(path);
    #[cfg(not(windows))]
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn prepared_state(key: &str, non_tex: &str, non_figures: &str) -> PreparedState {
        PreparedState {
            key: key.into(),
            non_tex: non_tex.into(),
            non_figures: non_figures.into(),
            requests: Vec::new(),
            fields: Vec::new(),
            numbers: BTreeMap::new(),
        }
    }

    #[test]
    fn prepared_cache_reuse_is_opt_in_and_tracks_tex_and_figure_changes() {
        let state = prepared_state("old", "same-assets", "old-tex");
        let exact = Fingerprints {
            key: "old".into(),
            non_tex: "same-assets".into(),
            non_figures: "old-tex".into(),
        };
        let text_edit = Fingerprints {
            key: "new".into(),
            non_tex: "same-assets".into(),
            non_figures: "new-tex".into(),
        };
        let figure_edit = Fingerprints {
            key: "new".into(),
            non_tex: "new-assets".into(),
            non_figures: "old-tex".into(),
        };
        let post_text_figure_edit = Fingerprints {
            key: "newer".into(),
            non_tex: "new-assets".into(),
            non_figures: "new-tex".into(),
        };
        let combined_edit = Fingerprints {
            key: "new".into(),
            non_tex: "new-assets".into(),
            non_figures: "new-tex".into(),
        };

        assert!(!can_reuse_inputs(&state, &exact, false));
        assert!(can_reuse_inputs(&state, &exact, true));
        assert!(!can_reuse_inputs(&state, &text_edit, false));
        assert!(can_reuse_inputs(&state, &text_edit, true));
        assert!(can_reuse_numbering(&state, &figure_edit, true));
        assert!(!can_reuse_numbering(&state, &combined_edit, true));
        let mut accepted_text = prepared_state("old", "same-assets", "old-tex");
        update_fingerprints(&mut accepted_text, &text_edit);
        assert!(can_reuse_numbering(&accepted_text, &post_text_figure_edit, true));
    }

    #[test]
    fn cached_mnova_requests_keep_the_selected_spectrum() {
        let request = FigureRequest::Mnova {
            path: "spectra/sample.mnova".into(),
            spectrum: std::num::NonZeroUsize::new(2).unwrap(),
            compound: Some("compound-a".into()),
            x: 70,
            y: 20,
            size: "8pt".into(),
        };

        let cached = serde_json::to_vec(&request).unwrap();
        let restored: FigureRequest = serde_json::from_slice(&cached).unwrap();

        assert_eq!(request, restored);
    }

    #[test]
    fn prepared_build_requires_exact_number_report_even_when_expected_numbers_are_empty() {
        let root = TempDir::new("chemex-number-report");
        let aux = root.join("aux");
        let output = root.join("output");
        fs::create_dir_all(&aux).unwrap();
        fs::create_dir_all(&output).unwrap();
        let requests = vec![FigureRequest::ChemDraw {
            counter: "compound".into(),
            path: "drawing.cdxml".into(),
        }];
        let prepared = PreparedBuild {
            inputs: Inputs { folder: PathBuf::new(), assets: PathBuf::new(), spectra: None },
            state: PreparedState {
                key: String::new(),
                non_tex: String::new(),
                non_figures: String::new(),
                requests,
                fields: Vec::new(),
                numbers: BTreeMap::new(),
            },
            key: String::new(),
            reused: false,
            assets: Some(Vec::new()),
        };
        for directory in [&aux, &output] {
            fs::write(
                directory.join(format!("main.{REQUESTS}")),
                "\\chemex@drawrequest{compound}{drawing.cdxml}\n",
            )
            .unwrap();
        }
        fs::write(aux.join(format!("main.{NUMBERS}")), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(output.join(format!("main.{NUMBERS}")), "\\chemex@number{compound-a}{1}\n")
            .unwrap();
        assert!(!prepared.agrees(&aux, &output, "main"));

        fs::write(output.join(format!("main.{NUMBERS}")), "").unwrap();
        assert!(prepared.agrees(&aux, &output, "main"));
    }

    #[cfg(windows)]
    #[test]
    fn isolated_discovery_and_numbering_use_separate_scratch_folders() {
        use std::cell::Cell;
        use std::os::windows::process::ExitStatusExt;
        use std::process::ExitStatus;

        let root = TempDir::new("chemex-isolated-preparation");
        let manifest = project::default_manifest("Chemex isolated preparation");
        project::write_manifest(&root, &manifest).unwrap();
        root.write("main.tex", "\\includegraphics{drawing.cdxml}");
        root.write(
            "drawing.cdxml",
            "<CDXML><page id='1'><t id='2' p='10 20'><s>[[@compound-001]]</s></t></page></CDXML>",
        );
        let preparation = Preparation {
            root: &root,
            document: "main.tex",
            engine: &manifest.engine,
            active: &ActiveBuild::default(),
            steps: &BuildSteps::silent(),
        };
        let calls = Cell::new(0);
        let result = preparation.run_isolated(&|command, scratch| {
            assert!(!scratch.starts_with(&*root));
            assert_eq!(Some(&*root), command.get_current_dir());
            let mode = fs::read_to_string(scratch.join("chemex-mode.tex")).unwrap();
            let status = match calls.replace(calls.get() + 1) {
                0 => {
                    assert_eq!("\\ChemexDiscoveryMode\n", mode);
                    assert!(!scratch.join(REGISTER).exists());
                    fs::write(
                        scratch.join(REQUESTS),
                        "\\chemex@drawrequest{compound}{drawing.cdxml}\n",
                    )
                    .unwrap();
                    0
                }
                1 => {
                    assert_eq!("\\ChemexNumberingMode\n", mode);
                    assert!(scratch.join(REGISTER).is_file());
                    assert!(!scratch.join(REQUESTS).exists());
                    1
                }
                _ => panic!("unexpected TeX pass"),
            };
            Ok((
                Output { status: ExitStatus::from_raw(status), stdout: vec![], stderr: vec![] },
                false,
            ))
        });
        assert_eq!(2, calls.get());
        assert!(result.err().is_some_and(|error| error.contains("Chemex numbering failed")));
    }

    #[test]
    fn project_detection_and_fingerprint_include_nested_tex_and_cdxml() {
        let root = TempDir::new("chemex-fingerprint");
        let manifest = project::default_manifest("Chemex fingerprint");
        project::write_manifest(&root, &manifest).unwrap();
        root.write("main.tex", "\\documentclass{article}\n\\input{chapters/results}\n");
        root.write(
            "chapters/results.tex",
            "% \\includegraphics{ignored.cdxml}\n\\includegraphics{drawing.cdxml}\n\\chemd{spectra/sample.mnova}\n",
        );
        root.write("drawing.cdxml", "first");
        root.write("spectra/sample.mnova", "first spectrum");
        let preparation = Preparation {
            root: &root,
            document: "main.tex",
            engine: "pdf",
            active: &ActiveBuild::default(),
            steps: &BuildSteps::silent(),
        };
        let before = preparation.fingerprint(&manifest).unwrap();
        assert!(enabled_project(&root, "main.tex").unwrap());
        root.write(".research/cache/ignored.tex", "changed");
        assert_eq!(before, preparation.fingerprint(&manifest).unwrap());
        root.write("drawing.cdxml", "changed");
        assert_ne!(before, preparation.fingerprint(&manifest).unwrap());
        let before = preparation.fingerprint(&manifest).unwrap();
        root.write("spectra/sample.mnova", "changed spectrum");
        assert_ne!(before, preparation.fingerprint(&manifest).unwrap());
    }

    #[test]
    fn project_detection_ignores_unreferenced_tex_documents() {
        let root = TempDir::new("chemex-unreferenced-tex");
        root.write("main.tex", "\\documentclass{article}\n\\begin{document}Text\\end{document}");
        root.write("figures/standalone.tex", "\\includegraphics{drawing.cdxml}");
        root.write("drawing.cdxml", "drawing");
        assert!(!enabled_project(&root, "main.tex").unwrap());
    }

    #[test]
    fn dependency_fingerprint_ignores_generated_and_build_outputs() {
        let root = TempDir::new("chemex-dependencies");
        let manifest = project::default_manifest("Chemex dependencies");
        project::write_manifest(&root, &manifest).unwrap();
        root.write("main.tex", "source");
        root.write("figure.cdxml", "used");
        root.write("spectrum.mnova", "used spectrum");
        let preparation = Preparation {
            root: &root,
            document: "main.tex",
            engine: "pdf",
            active: &ActiveBuild::default(),
            steps: &BuildSteps::silent(),
        };
        let before = preparation.fingerprint(&manifest).unwrap();
        root.write("build/previous/manuscript.tex", "old generated source");
        root.write(".research/build/main.aux", "build output");
        assert_eq!(before, preparation.fingerprint(&manifest).unwrap());
        root.write(".latexmkrc", "$pdf_mode = 1;");
        assert_ne!(before, preparation.fingerprint(&manifest).unwrap());
        let before = preparation.fingerprint(&manifest).unwrap();
        root.write("figure.cdxml", "changed drawing");
        assert_ne!(before, preparation.fingerprint(&manifest).unwrap());
        let before = preparation.fingerprint(&manifest).unwrap();
        root.write("spectrum.mnova", "changed spectrum");
        assert_ne!(before, preparation.fingerprint(&manifest).unwrap());
    }

    #[test]
    fn prepared_assets_are_reused_only_while_registered_pdfs_exist() {
        let root = TempDir::new("chemex-prepared-cache");
        let namespace = "a".repeat(64);
        let key = "b".repeat(64);
        let folder =
            chemistry::cache_file(&root, &format!("{namespace}/final/{REGISTER}")).unwrap();
        let final_dir = folder.parent().unwrap();
        fs::write(final_dir.join("chemex.sty"), tex::PACKAGE).unwrap();
        fs::write(final_dir.join("chemex-mode.tex"), "\\ChemexPreparedMode\n").unwrap();
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let name = format!("chemex-{}.pdf", sha256_hex(svg));
        let spectrum = format!("mnova-{}.pdf", "c".repeat(64));
        let registration = format!(
            "\\chemex@register{{1}}{{compound}}{{}}{{{name}}}\n\\chemex@registerspectrum{{2}}{{1}}{{{spectrum}}}\n"
        );
        fs::write(&folder, &registration).unwrap();
        let rendered = b"drawing";
        fs::write(
            chemistry::cache_file(&root, &format!("{namespace}/drawings/drawing-001.cdxml"))
                .unwrap(),
            rendered,
        )
        .unwrap();
        let assets = assets_folder(&root).unwrap();
        let source_key = sha256_hex(rendered);
        fs::write(assets.join(format!("{source_key}.svg")), svg).unwrap();
        fs::write(assets.join(&name), b"%PDF-1.4\n%%EOF").unwrap();
        fs::write(spectrum_assets_folder(&root).unwrap().join(&spectrum), b"%PDF-1.4\nx\n%%EOF")
            .unwrap();
        let marker = format!(
            "{key}\n{}\n{name} {source_key} {source_key}\n{spectrum} {} 1",
            sha256_hex(registration.as_bytes()),
            sha256_hex(b"source spectrum"),
        );
        chemistry::write_if_changed(
            &chemistry::ready_path(&root, &namespace).unwrap(),
            marker.as_bytes(),
        )
        .unwrap();
        assert!(chemistry::valid_pdf(&fs::read(assets.join(&name)).unwrap()));
        assert!(crate::project::check_embedded_svg(svg).is_ok());
        assert_eq!(name, format!("chemex-{}.pdf", sha256_hex(svg)));
        let ready = chemistry::ready_path(&root, &namespace).unwrap();
        assert!(ready.is_file());
        let ready = fs::read_to_string(ready).unwrap();
        assert_eq!(Some(key.as_str()), ready.lines().next());
        assert_eq!(Some(sha256_hex(registration.as_bytes()).as_str()), ready.lines().nth(1));
        let inputs = cached_inputs(&root, &namespace, &key).unwrap().unwrap();
        let mut command = Command::new("latexmk");
        inputs.configure(&mut command).unwrap();
        let (_, search) =
            command.get_envs().find(|(name, _)| *name == OsStr::new("TEXINPUTS")).unwrap();
        assert_eq!(
            Some(PathBuf::from(tex_path(&spectrum_assets_folder(&root).unwrap()))),
            std::env::split_paths(search.unwrap()).nth(2)
        );
        fs::remove_file(spectrum_assets_folder(&root).unwrap().join(&spectrum)).unwrap();
        assert!(cached_inputs(&root, &namespace, &key).unwrap().is_none());
        fs::write(spectrum_assets_folder(&root).unwrap().join(&spectrum), b"%PDF-1.4\nx\n%%EOF")
            .unwrap();
        fs::remove_file(assets_folder(&root).unwrap().join(name)).unwrap();
        assert!(cached_inputs(&root, &namespace, &key).unwrap().is_none());
    }

    #[test]
    fn figures_keep_mnova_spectrum_selection_and_resolve_paths_beside_the_paper() {
        let root = TempDir::new("chemex-mnova-figure");
        root.write("main.tex", "paper");
        root.write("spectra/Document 1.mnova", "opaque Mnova source");
        let figures = Figures::load(
            &root,
            "main.tex",
            vec![
                FigureRequest::Mnova {
                    path: "spectra/Document 1.mnova".into(),
                    spectrum: std::num::NonZeroUsize::new(1).unwrap(),
                    compound: None,
                    x: 50,
                    y: 15,
                    size: "8pt".into(),
                },
                FigureRequest::Mnova {
                    path: "spectra/Document 1.mnova".into(),
                    spectrum: std::num::NonZeroUsize::new(2).unwrap(),
                    compound: None,
                    x: 50,
                    y: 15,
                    size: "8pt".into(),
                },
            ],
        )
        .unwrap();

        assert_eq!(2, figures.items.len());
        assert_eq!(
            root.join("spectra/Document 1.mnova").canonicalize().unwrap(),
            figures.items[0].path
        );
        let registrations = figures.registrations().unwrap();
        assert!(
            matches!(registrations[0], FigureRegistration::Mnova { spectrum, .. } if spectrum.get() == 1)
        );
        assert!(
            matches!(registrations[1], FigureRegistration::Mnova { spectrum, .. } if spectrum.get() == 2)
        );
    }

    #[test]
    #[ignore = "requires ChemDraw and TeX Live"]
    fn lattice_build_reuses_preparation_and_content_keyed_exports() {
        let source = PathBuf::from(std::env::var_os("CHEMEX_TEST_CDXML").unwrap());
        let root = TempDir::new("chemex-lattice-build");
        let document = ".chem/paper-01/manuscript.tex";
        project::write_manifest(&root, &project::default_manifest("Chemex build test")).unwrap();
        let manuscript = "\\documentclass{article}\n\\usepackage{graphicx}\n\\usepackage{chemex}\n\\newcounter{compound}\n\\begin{document}\nThis word is unchanged.\\par\n\\chemex{compound-001}, \\chemex{compound-002}.\\par\\includegraphics[width=\\linewidth]{../../drawing.cdxml}\n\\par\\includegraphics[width=\\linewidth]{../../other.cdxml}\n\\end{document}\n";
        let text_edit = manuscript.replace("word is unchanged", "word was edited");
        root.write(document, manuscript);
        project::set_compile_root(&root, document).unwrap();
        let original = fs::read_to_string(source).unwrap();
        let other = format!("{original}\n<!-- second drawing -->");
        let edited = format!("{original}\n<!-- edited drawing -->");
        root.write("drawing.cdxml", &original);
        root.write("other.cdxml", &other);

        let builds = [
            (manuscript, &original, 2, false, false),
            (manuscript, &original, 0, true, false),
            (text_edit.as_str(), &original, 0, false, true),
            (text_edit.as_str(), &edited, 1, false, true),
        ];
        for (index, (document_source, drawing_source, expected_exports, skipped, no_preparation)) in
            builds.into_iter().enumerate()
        {
            root.write(document, document_source);
            if drawing_source == &edited {
                root.write("drawing.cdxml", drawing_source);
            }
            let steps = BuildSteps::silent();
            let result = super::build::build(
                &root,
                false,
                false,
                &ActiveBuild::default(),
                None,
                &steps,
                crate::latex::AuxFiles::Hidden,
            )
            .unwrap();
            assert!(result.success, "{}", result.log);
            let pdf = lopdf::Document::load(root.join(document).with_extension("pdf")).unwrap();
            assert!(
                pdf.objects.values().any(|object| {
                    object.as_stream().is_ok_and(|stream| {
                        stream.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok()
                            == Some(b"Form")
                    })
                }),
                "The manuscript PDF did not embed the vector drawing"
            );
            let exports = result.steps.iter().filter(|step| step.tool == "ChemDraw").count();
            assert_eq!(expected_exports, exports, "build pass {index}");
            if no_preparation {
                assert!(result.steps.iter().all(|step| {
                    step.tool != "Chemex discovery" && step.tool != "Chemex numbering"
                }));
            }
            for drawing in [drawing_source, &other] {
                assert!(chemistry::built_preview_svg(&root, document, drawing).is_some());
            }
            if skipped {
                assert!(result.steps.is_empty(), "{:?}\n{}", result.steps, result.log);
                assert!(crate::latex::build_log::skipped_recompile(&result.log), "{}", result.log);
            }
        }
        assert_eq!(edited, fs::read_to_string(root.join("drawing.cdxml")).unwrap());
        assert_eq!(other, fs::read_to_string(root.join("other.cdxml")).unwrap());
    }
}
