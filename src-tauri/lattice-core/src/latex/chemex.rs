//! Prepare ChemDraw assets before the ordinary, incremental latexmk run.

use super::{build, search_paths, ActiveBuild, BuildSteps};
use crate::util::{err, sha256_hex};
use crate::{chemistry, commands, host_paths, project};
use chemex_cdxml::{DrawingDocument, FieldValues};
use chemex_cli::{self as tex, DrawingRegistration, DrawingRequest};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

const REQUESTS: &str = "chemex-drawings.dat";
const NUMBERS: &str = "chemex-numbers.dat";
const REGISTER: &str = "chemex-register.tex";

pub(super) struct Inputs {
    folder: PathBuf,
    assets: PathBuf,
}

impl Inputs {
    pub(super) fn configure(&self, command: &mut Command) -> Result<(), String> {
        prepend_inputs(command, &[&self.folder, &self.assets])
    }
}

pub(super) fn enabled(source: &Path) -> bool {
    fs::read(source).is_ok_and(|bytes| {
        bytes.split(|byte| *byte == b'\n').any(|line| {
            let text = line.split(|byte| *byte == b'%').next().unwrap_or_default();
            text.windows(b"chemex".len()).any(|word| word == b"chemex")
                || text.windows(b".cdxml".len()).any(|word| word.eq_ignore_ascii_case(b".cdxml"))
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

impl Preparation<'_> {
    pub(super) fn run(&self) -> Result<Inputs, String> {
        let manifest = project::read_manifest(self.root)?;
        let key = self.fingerprint(&manifest)?;
        let namespace = sha256_hex(self.document);
        if let Some(inputs) = cached_inputs(self.root, &namespace, &key)? {
            return Ok(inputs);
        }
        let discovery = self.pass_folder(&namespace, "discover", b"\\ChemexDiscoveryMode\n")?;
        let drawings = self.discover(&discovery)?;
        if drawings.requests.is_empty() {
            return Err("The LaTeX project contains no CDXML references".into());
        }
        let mut registrations = drawings.registrations();
        let (values, numbers) = self.number(&namespace, &drawings, &registrations)?;
        let mut assets = Vec::with_capacity(drawings.requests.len());
        for (index, (request, drawing)) in drawings.iter().enumerate() {
            let rendered = drawing.render_fields(&values).map_err(err)?;
            let rendered_path = chemistry::cache_file(
                self.root,
                &format!("{namespace}/drawings/drawing-{:03}.cdxml", index + 1),
            )?;
            let (svg, pdf) =
                chemistry::pdf(self.root, &rendered, |command| self.run_tool("ChemDraw", command))
                    .map_err(|error| format!("{}: {error}", request.path))?;
            chemistry::write_if_changed(&rendered_path, rendered.as_bytes())?;
            let asset = pdf
                .file_name()
                .and_then(OsStr::to_str)
                .ok_or("Missing Chemex asset filename")?
                .to_owned();
            let numbered =
                svg.file_stem().and_then(OsStr::to_str).ok_or("Missing Chemex SVG key")?;
            assets.push(format!("{asset} {} {numbered}", sha256_hex(drawing.source())));
            registrations[index].asset = asset;
        }
        let folder = self.pass_folder(&namespace, "final", b"\\ChemexPreparedMode\n")?;
        let registration = tex::prepared_registrations(&registrations, &numbers).map_err(err)?;
        chemistry::write_if_changed(&folder.join(REGISTER), registration.as_bytes())?;
        let inputs = Inputs { folder, assets: assets_folder(self.root)? };
        if self.fingerprint(&manifest)? != key {
            return Err(
                "LaTeX or CDXML sources changed during Chemex preparation; build again".into()
            );
        }
        chemistry::write_if_changed(
            &chemistry::ready_path(self.root, &namespace)?,
            format!("{key}\n{}\n{}", sha256_hex(registration.as_bytes()), assets.join("\n"))
                .as_bytes(),
        )?;
        Ok(inputs)
    }

    fn fingerprint(&self, manifest: &crate::models::ProjectManifest) -> Result<String, String> {
        let root = self.root.canonicalize().map_err(err)?;
        let mut hash = Sha256::new();
        hash.update(self.document.as_bytes());
        hash.update([0]);
        hash.update(self.engine.as_bytes());
        hash.update([0]);
        hash.update(manifest.trusted.to_string().as_bytes());
        hash.update([0]);
        hash.update(b"chemex-preparation-v2");
        hash.update([0]);
        hash.update(tex::PACKAGE.as_bytes());
        let mut files = dependency_files(&root)?;
        files.push(project::safe_path(&root, self.document)?);
        files.sort();
        files.dedup();
        for path in files {
            let relative = path.strip_prefix(&root).map_err(err)?.to_string_lossy();
            hash.update((relative.len() as u64).to_le_bytes());
            hash.update(relative.as_bytes());
            let mut file = File::open(path).map_err(err)?;
            let mut content = Sha256::new();
            let mut buffer = [0; 16 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(err)?;
                if read == 0 {
                    break;
                }
                content.update(&buffer[..read]);
            }
            hash.update(content.finalize());
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    fn pass_folder(&self, namespace: &str, pass: &str, mode: &[u8]) -> Result<PathBuf, String> {
        let mode_file =
            chemistry::cache_file(self.root, &format!("{namespace}/{pass}/chemex-mode.tex"))?;
        let folder = mode_file.parent().ok_or("Missing Chemex preparation directory")?;
        chemistry::write_if_changed(&folder.join("chemex.sty"), tex::PACKAGE.as_bytes())?;
        chemistry::write_if_changed(&mode_file, mode)?;
        Ok(folder.to_path_buf())
    }

    fn discover(&self, folder: &Path) -> Result<Drawings, String> {
        self.pass("Chemex discovery", folder)?;
        let requests = match fs::symlink_metadata(folder.join(REQUESTS)) {
            Ok(info) if info.is_file() => {
                tex::read_drawings(&folder.join(REQUESTS)).map_err(err)?
            }
            Ok(_) => return Err("Invalid Chemex drawing request file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(err(error)),
        };
        Drawings::load(self.root, self.document, requests)
    }

    fn number(
        &self, namespace: &str, drawings: &Drawings, registrations: &[DrawingRegistration],
    ) -> Result<(FieldValues, BTreeMap<String, String>), String> {
        let fields: Vec<_> =
            registrations.iter().flat_map(|drawing| drawing.fields.iter().cloned()).collect();
        if fields.is_empty() {
            return Ok((FieldValues::new(), BTreeMap::new()));
        }
        let folder = self.pass_folder(namespace, "number", b"\\ChemexNumberingMode\n")?;
        write_registrations(&folder, registrations)?;
        self.pass("Chemex numbering", &folder)?;
        let numbered = tex::read_drawings(&folder.join(REQUESTS)).map_err(err)?;
        tex::validate_drawings(&drawings.requests, &numbered, "numbering").map_err(err)?;
        let numbers = tex::read_numbers(&folder.join(NUMBERS)).map_err(err)?;
        let values = tex::field_values(&fields, &numbers).map_err(err)?;
        Ok((values, numbers))
    }

    fn pass(&self, name: &str, folder: &Path) -> Result<(), String> {
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
        prepend_inputs(&mut command, &[folder])?;
        self.run_tool(name, command)
    }

    fn run_tool(&self, name: &str, command: Command) -> Result<(), String> {
        self.steps.begin(name, true);
        let (output, cancelled) = build::run_tracked(
            command,
            self.active,
            self.steps,
            "Could not start Chemex tool: ",
            "Chemex tool stopped: ",
        )?;
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
}

fn dependency_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let root = root.canonicalize().map_err(err)?;
    let mut files = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_entry(|entry| {
        entry.depth() == 0
            || !matches!(
                entry.file_name().to_str(),
                Some(".research" | ".git" | "node_modules" | "target" | "build")
            )
    }) {
        let entry = entry.map_err(err)?;
        let extension = entry.path().extension().and_then(OsStr::to_str).unwrap_or_default();
        if entry.file_type().is_file()
            && (["tex", "sty", "cls", "def", "cfg", "cdxml", "bib"]
                .iter()
                .any(|kind| extension.eq_ignore_ascii_case(kind))
                || matches!(entry.file_name().to_str(), Some("latexmkrc" | ".latexmkrc")))
        {
            files.push(entry.into_path());
        }
    }
    Ok(files)
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
    for (index, line) in lines.enumerate() {
        let valid = if let Some(asset) = chemistry::Asset::parse(line) {
            asset_cached(&assets, asset.pdf, asset.numbered)
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
    Ok(Some(Inputs { folder, assets }))
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

struct Drawings {
    requests: Vec<DrawingRequest>,
    documents: BTreeMap<String, DrawingDocument>,
}

impl Drawings {
    fn load(root: &Path, document: &str, requests: Vec<DrawingRequest>) -> Result<Self, String> {
        let mut documents = BTreeMap::new();
        for request in &requests {
            if !documents.contains_key(&request.path) {
                let path = drawing_path(root, document, &request.path)?;
                let drawing = chemistry::read_document(&path)
                    .map_err(|error| format!("{}: {error}", request.path))?;
                documents.insert(request.path.clone(), drawing);
            }
        }
        Ok(Self { requests, documents })
    }

    fn iter(&self) -> impl Iterator<Item = (&DrawingRequest, &DrawingDocument)> {
        self.requests.iter().map(|request| (request, &self.documents[&request.path]))
    }

    fn registrations(&self) -> Vec<DrawingRegistration> {
        self.iter()
            .map(|(request, drawing)| DrawingRegistration {
                counter: request.counter.clone(),
                fields: drawing.fields().iter().map(|field| field.compound_id.clone()).collect(),
                asset: String::new(),
            })
            .collect()
    }
}

fn drawing_path(root: &Path, document: &str, name: &str) -> Result<PathBuf, String> {
    let relative = Path::new(name);
    if !relative.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cdxml")) {
        return Err(format!("Expected a CDXML drawing: {name}"));
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

fn write_registrations(folder: &Path, registrations: &[DrawingRegistration]) -> Result<(), String> {
    chemistry::write_if_changed(
        &folder.join(REGISTER),
        tex::registrations(registrations).as_bytes(),
    )
}

fn prepend_inputs(command: &mut Command, paths: &[&Path]) -> Result<(), String> {
    let inherited = command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("TEXINPUTS"))
        .and_then(|(_, value)| value.map(OsStr::to_owned))
        .or_else(|| std::env::var_os("TEXINPUTS"))
        .unwrap_or_default();
    let mut value = std::env::join_paths(paths.iter().map(|path| PathBuf::from(tex_path(path))))
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

    #[test]
    fn project_detection_and_fingerprint_include_nested_tex_and_cdxml() {
        let root = TempDir::new("chemex-fingerprint");
        let manifest = project::default_manifest("Chemex fingerprint");
        project::write_manifest(&root, &manifest).unwrap();
        root.write("main.tex", "\\documentclass{article}\n\\input{chapters/results}\n");
        root.write(
            "chapters/results.tex",
            "% \\includegraphics{ignored.cdxml}\n\\includegraphics{drawing.cdxml}\n",
        );
        root.write("drawing.cdxml", "first");
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
        let registration = format!("\\chemex@register{{1}}{{compound}}{{}}{{{name}}}\n");
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
        let marker = format!(
            "{key}\n{}\n{name} {source_key} {source_key}",
            sha256_hex(registration.as_bytes())
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
        assert!(cached_inputs(&root, &namespace, &key).unwrap().is_some());
        fs::remove_file(assets_folder(&root).unwrap().join(name)).unwrap();
        assert!(cached_inputs(&root, &namespace, &key).unwrap().is_none());
    }

    #[test]
    #[ignore = "requires ChemDraw and TeX Live"]
    fn lattice_build_renders_cdxml_then_reuses_the_persistent_assets() {
        let source = PathBuf::from(std::env::var_os("CHEMEX_TEST_CDXML").unwrap());
        let root = TempDir::new("chemex-lattice-build");
        let document = ".chem/paper-01/manuscript.tex";
        project::write_manifest(&root, &project::default_manifest("Chemex build test")).unwrap();
        root.write(
            document,
            "\\documentclass{article}\n\\usepackage{graphicx}\n\\usepackage{chemex}\n\\newcounter{compound}\n\\begin{document}\n\\chemex{compound-001}, \\chemex{compound-002}.\\par\\includegraphics[width=\\linewidth]{../../drawing.cdxml}\n\\par\\includegraphics[width=\\linewidth]{../../other.cdxml}\n\\end{document}\n",
        );
        project::set_compile_root(&root, document).unwrap();
        let original = fs::read_to_string(source).unwrap();
        let other = format!("{original}\n<!-- second drawing -->");
        let edited = format!("{original}\n<!-- edited drawing -->");
        root.write("drawing.cdxml", &original);
        root.write("other.cdxml", &other);

        for (index, (source, expected_exports)) in
            [(&original, 2), (&original, 0), (&edited, 1)].into_iter().enumerate()
        {
            if source == &edited {
                root.write("drawing.cdxml", source);
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
            for source in [source, &other] {
                assert!(chemistry::built_preview_svg(&root, document, source).is_some());
            }
            if expected_exports == 0 {
                assert!(result.steps.is_empty(), "{:?}\n{}", result.steps, result.log);
                assert!(crate::latex::build_log::skipped_recompile(&result.log), "{}", result.log);
            }
        }
        assert_eq!(edited, fs::read_to_string(root.join("drawing.cdxml")).unwrap());
        assert_eq!(other, fs::read_to_string(root.join("other.cdxml")).unwrap());
    }
}
