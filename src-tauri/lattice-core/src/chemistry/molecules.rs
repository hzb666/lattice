use std::{collections::HashMap, path::Path, time::Duration};

use chemex_cli::{CompoundStructure, StructureReport};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoleculeLocation {
    pub file: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMolecule {
    pub location: MoleculeLocation,
    pub structure: CompoundStructure,
    pub duplicate_of: Option<MoleculeLocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoleculeAnalysisError {
    pub file: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectMoleculeReport {
    pub molecules: Vec<ProjectMolecule>,
    pub errors: Vec<MoleculeAnalysisError>,
}

pub fn analyze_project_molecules(root: &Path) -> Result<ProjectMoleculeReport, String> {
    let tree = crate::project::scan_tree(root, crate::project::TreeView::Project)?;
    let drawings = super::cdxml_files(&tree).map(|file| {
        let result = read_stable_drawing(root, file)
            .and_then(|source| chemex_cli::analyze_compounds_with_rdkit(&source));
        (file.to_owned(), result)
    });
    Ok(merge_drawings(drawings))
}

fn read_stable_drawing(root: &Path, relative: &str) -> Result<String, String> {
    let path = crate::project::safe_path(root, relative)?;
    let before = super::read_asset(&path)?;
    std::thread::sleep(Duration::from_millis(100));
    let after = super::read_asset(&path)?;
    if before != after {
        return Err("CDXML changed while being read; wait for ChemDraw to finish saving".into());
    }
    String::from_utf8(after).map_err(|error| error.to_string())
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
}
