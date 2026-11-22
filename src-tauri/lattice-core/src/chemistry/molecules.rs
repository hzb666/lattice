use std::{collections::HashMap, path::Path, time::Duration};

use chemex_cli::{CompoundStructure, StructureReport};
use serde::{Deserialize, Serialize};

mod cache;

const STABILITY_DELAY: Duration = Duration::from_millis(100);
const MAX_BATCH_SOURCE_BYTES: usize = 16 * 1024 * 1024;

type DrawingResult<T> = (String, Result<T, String>);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MoleculeLocation {
    pub file: String,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProjectMolecule {
    pub location: MoleculeLocation,
    pub structure: CompoundStructure,
    pub duplicate_of: Option<MoleculeLocation>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MoleculeAnalysisError {
    pub file: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProjectMoleculeReport {
    pub molecules: Vec<ProjectMolecule>,
    pub errors: Vec<MoleculeAnalysisError>,
}

pub fn analyze_project_molecules(root: &Path) -> Result<ProjectMoleculeReport, String> {
    analyze_project(root, chemex_cli::analyze_compounds_batch_with_rdkit)
}

fn analyze_project(
    root: &Path, analyze: impl FnMut(&[&str]) -> Vec<Result<StructureReport, String>>,
) -> Result<ProjectMoleculeReport, String> {
    let tree = crate::project::scan_tree(root, crate::project::TreeView::Project)?;
    let files: Vec<_> = super::cdxml_files(&tree).map(str::to_owned).collect();
    let mut connection = cache::open(root).ok();
    let mut cached = connection.as_ref().and_then(|db| cache::load(db).ok()).unwrap_or_default();
    let mut drawings = HashMap::new();
    let mut pending = Vec::new();
    for file in &files {
        let stamp = cache::file_stamp(root, file);
        match cached.remove(file).and_then(|entry| entry.report_at(stamp)) {
            Some(report) => {
                drawings.insert(file.clone(), Ok(report));
            }
            None => pending.push((file, stamp)),
        }
    }

    let stable = read_drawings(root, pending.iter().map(|(file, _)| (*file).clone()));
    let analyzed = analyze_drawings(stable, analyze);
    if let Some(db) = &mut connection {
        let updates: Vec<_> = pending
            .iter()
            .zip(&analyzed)
            .filter_map(|((file, stamp), (_, report))| {
                let report = report.as_ref().ok()?;
                let stamp = (*stamp)?;
                if cache::file_stamp(root, file) != Some(stamp) {
                    return None;
                }
                Some((file.as_str(), stamp, report))
            })
            .collect();
        let _ = cache::store(db, cached.keys().map(String::as_str), &updates);
    }
    drawings.extend(analyzed);
    Ok(merge_drawings(files.into_iter().filter_map(|file| {
        let report = drawings.remove(&file)?;
        Some((file, report))
    })))
}

fn read_drawings(
    root: &Path, files: impl Iterator<Item = String>,
) -> impl Iterator<Item = DrawingResult<String>> + '_ {
    let snapshots = files
        .map(|file| {
            let result = crate::project::safe_path(root, &file)
                .and_then(|path| super::read_asset(&path))
                .map(crate::util::sha256_hex);
            (file, result)
        })
        .collect::<Vec<_>>();
    if !snapshots.is_empty() {
        std::thread::sleep(STABILITY_DELAY);
    }
    snapshots.into_iter().map(move |(file, hash)| {
        let source = hash.and_then(|hash| read_stable_drawing(root, &file, &hash));
        (file, source)
    })
}

fn analyze_drawings(
    stable: impl IntoIterator<Item = DrawingResult<String>>,
    mut analyze: impl FnMut(&[&str]) -> Vec<Result<StructureReport, String>>,
) -> Vec<DrawingResult<StructureReport>> {
    let mut batch = Vec::new();
    let mut source_bytes = 0;
    let mut drawings = Vec::new();
    for (file, source) in stable {
        let size = source.as_ref().map_or(0, String::len);
        if !batch.is_empty() && source_bytes + size > MAX_BATCH_SOURCE_BYTES {
            drawings.extend(analyze_batch(std::mem::take(&mut batch), &mut analyze));
            source_bytes = 0;
        }
        batch.push((file, source));
        source_bytes += size;
    }
    if !batch.is_empty() {
        drawings.extend(analyze_batch(batch, &mut analyze));
    }
    drawings
}

fn analyze_batch(
    batch: Vec<DrawingResult<String>>,
    analyze: impl FnOnce(&[&str]) -> Vec<Result<StructureReport, String>>,
) -> impl Iterator<Item = DrawingResult<StructureReport>> {
    let sources = batch.iter().filter_map(|(_, source)| source.as_deref().ok()).collect::<Vec<_>>();
    let mut reports = analyze(&sources).into_iter();
    batch.into_iter().map(move |(file, source)| {
        let report =
            source.and_then(|_| reports.next().ok_or("RDKit report is missing for a drawing")?);
        (file, report)
    })
}

pub fn display_formula(formula: &str) -> String {
    chemex_cli::formula_display(formula).unwrap_or_else(|_| formula.to_owned())
}

pub fn compound_formula(root: &Path, key: &str) -> Result<Option<String>, String> {
    if key.is_empty() {
        return Ok(None);
    }
    let report = analyze_project_molecules(root)?;
    formula_for_key(&report, key)
}

fn formula_for_key(report: &ProjectMoleculeReport, key: &str) -> Result<Option<String>, String> {
    let matches: Vec<_> =
        report.molecules.iter().filter(|molecule| molecule.location.key == key).collect();
    let Some(first) = matches.first() else {
        return Ok(None);
    };
    let formula = first
        .structure
        .formula
        .as_ref()
        .ok_or_else(|| format!("ChemDraw key {key} has no molecular formula"))?;
    if matches.iter().any(|molecule| molecule.structure.formula.as_ref() != Some(formula)) {
        return Err(format!("ChemDraw key {key} maps to different molecular formulas"));
    }
    Ok(Some(formula.clone()))
}

pub fn compound_formula_display(root: &Path, key: &str) -> Result<Option<String>, String> {
    compound_formula(root, key)?
        .map(|formula| chemex_cli::formula_display(&formula).map_err(|error| error.to_string()))
        .transpose()
}

fn read_stable_drawing(root: &Path, relative: &str, before_hash: &str) -> Result<String, String> {
    let path = crate::project::safe_path(root, relative)?;
    let bytes = super::read_asset(&path)?;
    if crate::util::sha256_hex(&bytes) != before_hash {
        return Err("CDXML changed while being read; wait for ChemDraw to finish saving".into());
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn merge_drawings(
    drawings: impl IntoIterator<Item = (String, Result<StructureReport, String>)>,
) -> ProjectMoleculeReport {
    let mut report = ProjectMoleculeReport::default();
    let mut by_structure: HashMap<String, MoleculeLocation> = HashMap::new();
    for (file, result) in drawings {
        let drawing = match result {
            Ok(report) => report,
            Err(message) => {
                report.errors.push(MoleculeAnalysisError { file, message });
                continue;
            }
        };
        append_drawing(&mut report, &mut by_structure, file, drawing.compounds);
    }
    report
}

fn append_drawing(
    report: &mut ProjectMoleculeReport, by_structure: &mut HashMap<String, MoleculeLocation>,
    file: String, compounds: Vec<CompoundStructure>,
) {
    for structure in compounds {
        let location = MoleculeLocation { file: file.clone(), key: structure.compound_id.clone() };
        let duplicate_of = structure
            .structure_hash
            .as_ref()
            .filter(|_| structure.deduplication_eligible)
            .and_then(|hash| match by_structure.entry(hash.clone()) {
                std::collections::hash_map::Entry::Occupied(first) => Some(first.get().clone()),
                std::collections::hash_map::Entry::Vacant(first) => {
                    first.insert(location.clone());
                    None
                }
            });
        report.molecules.push(ProjectMolecule { location, structure, duplicate_of });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn structure(key: &str, hash: Option<&str>, duplicate_of: Option<&str>) -> CompoundStructure {
        CompoundStructure {
            compound_id: key.into(),
            formula: Some("CH4".into()),
            canonical_smiles: Some("C".into()),
            structure_hash: hash.map(str::to_owned),
            duplicate_of: duplicate_of.map(str::to_owned),
            deduplication_eligible: hash.is_some(),
            status: None,
        }
    }

    #[test]
    fn merges_duplicate_keys_across_files_but_keeps_unknowns_and_file_errors() {
        let report = merge_drawings([
            (
                "first.cdxml".into(),
                Ok(StructureReport {
                    rdkit_version: "2026.09.2".into(),
                    compounds: vec![structure("a", Some("hash"), None)],
                }),
            ),
            (
                "second.cdxml".into(),
                Ok(StructureReport {
                    rdkit_version: "2026.09.2".into(),
                    compounds: vec![
                        structure("b", Some("hash"), None),
                        structure("c", Some("hash"), Some("b")),
                        structure("unknown", None, None),
                    ],
                }),
            ),
            ("broken.cdxml".into(), Err("invalid XML".into())),
        ]);

        assert_eq!(4, report.molecules.len());
        assert_eq!(
            Some(MoleculeLocation { file: "first.cdxml".into(), key: "a".into() }),
            report.molecules[1].duplicate_of.clone()
        );
        assert_eq!(report.molecules[1].duplicate_of, report.molecules[2].duplicate_of);
        assert_eq!(None, report.molecules[3].duplicate_of);
        assert_eq!("broken.cdxml", report.errors[0].file);
    }

    #[test]
    fn stable_read_rejects_a_drawing_changed_during_the_settle_delay() {
        let fixture = crate::project::test_support::Fixture::empty("molecule-stable-read");
        fixture.write("drawing.cdxml", b"before");
        let before = crate::util::sha256_hex(b"before");
        fixture.write("drawing.cdxml", b"after");

        assert!(read_stable_drawing(&fixture.root, "drawing.cdxml", &before).is_err());
    }

    #[test]
    fn finds_a_formula_by_dynamic_key_and_rejects_conflicts() {
        let molecule = |key: &str, formula: &str| {
            let mut structure = structure(key, None, None);
            structure.formula = Some(formula.into());
            ProjectMolecule {
                location: MoleculeLocation { file: format!("{key}.cdxml"), key: key.into() },
                structure,
                duplicate_of: None,
            }
        };
        let report = ProjectMoleculeReport {
            molecules: vec![molecule("product-17", "C6H6"), molecule("other", "CH4")],
            errors: Vec::new(),
        };

        assert_eq!(Some("C6H6".into()), formula_for_key(&report, "product-17").unwrap());
        assert_eq!(None, formula_for_key(&report, "missing").unwrap());
        let conflict = ProjectMoleculeReport {
            molecules: vec![molecule("product-17", "C6H6"), molecule("product-17", "C5H6")],
            errors: Vec::new(),
        };
        assert!(formula_for_key(&conflict, "product-17").is_err());
    }

    #[test]
    fn reuses_unchanged_drawings_and_rebuilds_project_duplicates() {
        let fixture = crate::project::test_support::Fixture::project("molecule-cache");
        let root = &fixture.root;
        fixture.write("a.cdxml", "<CDXML><page id='a'/></CDXML>");
        fixture.write("b.cdxml", "<CDXML><page id='b'/></CDXML>");
        let analyze = |sources: &[&str]| {
            sources
                .iter()
                .map(|source| {
                    let hash = if source.contains("changed") { "other" } else { "same" };
                    Ok(StructureReport {
                        rdkit_version: "2026.09.2".into(),
                        compounds: vec![structure("molecule", Some(hash), None)],
                    })
                })
                .collect()
        };

        let first = analyze_project(root, |sources| {
            assert_eq!(2, sources.len());
            analyze(sources)
        })
        .unwrap();
        assert!(first.errors.is_empty());
        assert_eq!(Some(&first.molecules[0].location), first.molecules[1].duplicate_of.as_ref());

        fixture.write("a.cdxml", "<CDXML><page id='changed'/></CDXML>");
        let changed = analyze_project(root, |sources| {
            assert_eq!(&["<CDXML><page id='changed'/></CDXML>"][..], sources);
            analyze(sources)
        })
        .unwrap();
        assert_eq!(2, changed.molecules.len());
        assert_eq!(None, changed.molecules[1].duplicate_of);

        fs::remove_file(root.join("a.cdxml")).unwrap();
        let remaining =
            analyze_project(root, |_| panic!("unchanged files should reuse analysis")).unwrap();
        assert_eq!(1, remaining.molecules.len());
        assert_eq!("b.cdxml", remaining.molecules[0].location.file);
        assert_eq!(1, cache::load(&cache::open(root).unwrap()).unwrap().len());
    }

    #[test]
    fn caches_successful_drawings_when_another_fails() {
        let fixture = crate::project::test_support::Fixture::project("molecule-cache-partial");
        fixture.write("a.cdxml", "<CDXML><page id='a'/></CDXML>");
        fixture.write("b.cdxml", "<CDXML><page id='b'/></CDXML>");
        let analyze = |sources: &[&str]| {
            sources
                .iter()
                .map(|source| {
                    if source.contains("id='a'") {
                        Ok(StructureReport {
                            rdkit_version: "2026.09.2".into(),
                            compounds: Vec::new(),
                        })
                    } else {
                        Err("Analysis failed".into())
                    }
                })
                .collect()
        };

        let first = analyze_project(&fixture.root, analyze).unwrap();
        let second = analyze_project(&fixture.root, |sources| {
            assert_eq!(&["<CDXML><page id='b'/></CDXML>"][..], sources);
            analyze(sources)
        })
        .unwrap();
        assert_eq!(1, first.errors.len());
        assert_eq!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_cache_paths() {
        let parent_link = crate::project::test_support::Fixture::empty("molecule-cache-link");
        let outside = parent_link.parent.join("outside");
        fs::create_dir_all(&outside).unwrap();
        crate::test_support::symlink(&outside, parent_link.path(".chem")).unwrap();

        assert!(cache::open(&parent_link.root).is_err());
        assert!(!outside.join("molecules.sqlite").exists());

        let file_link = crate::project::test_support::Fixture::empty("molecule-cache-file-link");
        fs::create_dir_all(file_link.path(".chem")).unwrap();
        let external_database = file_link.parent.join("outside.sqlite");
        fs::write(&external_database, b"leave this file alone").unwrap();
        crate::test_support::symlink(&external_database, file_link.path(cache::DB_RELATIVE))
            .unwrap();

        assert!(cache::open(&file_link.root).is_err());
        assert_eq!(b"leave this file alone", fs::read(external_database).unwrap().as_slice());
    }
}
