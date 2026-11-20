use std::collections::HashMap;

use chemex_cdxml::{DrawingDocument, ObjectKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const RDKIT_VERSION: &str = "2026.09.2";
const IDENTITY_POLICY: &str = "exact-isomeric-smiles-v1";
const RDKIT_SCRIPT: &str = include_str!("structures.py");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CompoundStructure {
    pub compound_id: String,
    pub formula: Option<String>,
    pub canonical_smiles: Option<String>,
    pub structure_hash: Option<String>,
    pub duplicate_of: Option<String>,
    pub deduplication_eligible: bool,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct StructureReport {
    pub rdkit_version: String,
    pub compounds: Vec<CompoundStructure>,
}

#[derive(Serialize)]
struct RdkitGroup {
    id: String,
    fragments: Vec<String>,
}

#[derive(Deserialize)]
struct RdkitReport {
    rdkit_version: String,
    compounds: Vec<RdkitCompound>,
}

#[derive(Deserialize)]
struct RdkitCompound {
    id: String,
    formula: Option<String>,
    canonical_smiles: Option<String>,
    deduplication_eligible: bool,
    status: Option<String>,
}

/// Analyze each valid numbered ChemDraw group using a caller-owned RDKit runner.
///
/// The runner receives the embedded Python program and its JSON request, and
/// returns the program's JSON output. It owns process, cache, and timeout policy.
pub fn analyze_compounds(
    source: &str,
    run_rdkit: impl FnOnce(&str, &str) -> Result<String, String>,
) -> Result<StructureReport, String> {
    let document = DrawingDocument::parse(source.to_owned()).map_err(|error| error.to_string())?;
    let mut groups = Vec::new();
    let mut unavailable = Vec::new();
    let mut seen_ids = Vec::new();
    for compound_id in document
        .fields()
        .iter()
        .map(|field| field.compound_id.as_str())
    {
        if seen_ids.iter().any(|seen| seen == compound_id) {
            continue;
        }
        seen_ids.push(compound_id.to_owned());
        let Ok(group_object) = document.compound_group(compound_id) else {
            unavailable.push(unavailable_compound(
                compound_id,
                "No unique ChemDraw group is associated with this label.",
            ));
            continue;
        };
        let group_index = document
            .objects()
            .iter()
            .position(|object| std::ptr::eq(object, group_object))
            .ok_or_else(|| "ChemDraw group is missing from the parsed document".to_string())?;
        let mut fragment_ids = Vec::new();
        let mut invalid_fragment = false;
        for field in document.fields().iter().filter(|field| {
            field.compound_id == compound_id && field.group_object == Some(group_index)
        }) {
            for fragment in &field.fragment_objects {
                let object = &document.objects()[*fragment];
                if object.kind != ObjectKind::Fragment {
                    continue;
                }
                let Some(id) = object
                    .id
                    .as_deref()
                    .filter(|id| document.object_by_id(id).is_some())
                else {
                    invalid_fragment = true;
                    break;
                };
                if !fragment_ids.iter().any(|found: &String| found == id) {
                    fragment_ids.push(id.to_owned());
                }
            }
        }
        if invalid_fragment {
            unavailable.push(unavailable_compound(
                compound_id,
                "A ChemDraw fragment has no unique object ID.",
            ));
        } else if fragment_ids.is_empty() {
            unavailable.push(unavailable_compound(
                compound_id,
                "The associated ChemDraw group contains no molecular fragments.",
            ));
        } else {
            groups.push(RdkitGroup {
                id: compound_id.to_owned(),
                fragments: fragment_ids,
            });
        }
    }

    if groups.is_empty() {
        return Ok(StructureReport {
            rdkit_version: RDKIT_VERSION.into(),
            compounds: unavailable,
        });
    }
    let request = serde_json::json!({ "source": source, "groups": groups });
    let output = run_rdkit(RDKIT_SCRIPT, &request.to_string())?;
    let parsed: RdkitReport = serde_json::from_str(&output)
        .map_err(|error| format!("RDKit returned an invalid structure report: {error}"))?;
    if parsed.rdkit_version != RDKIT_VERSION {
        return Err(format!(
            "RDKit version mismatch: expected {RDKIT_VERSION}, got {}",
            parsed.rdkit_version
        ));
    }

    let expected_ids: Vec<_> = groups.iter().map(|group| group.id.as_str()).collect();
    let returned_ids: Vec<_> = parsed
        .compounds
        .iter()
        .map(|compound| compound.id.as_str())
        .collect();
    if returned_ids.len() != expected_ids.len()
        || expected_ids.iter().any(|id| {
            returned_ids
                .iter()
                .filter(|returned| *returned == id)
                .count()
                != 1
        })
    {
        return Err(
            "RDKit did not return exactly one result for each requested ChemDraw group".into(),
        );
    }

    let mut analyzed: HashMap<_, _> = parsed
        .compounds
        .into_iter()
        .map(|compound| (compound.id.clone(), compound))
        .collect();
    let mut unresolved: HashMap<_, _> = unavailable
        .into_iter()
        .map(|compound| (compound.compound_id.clone(), compound))
        .collect();
    let mut seen = HashMap::new();
    let mut compounds = Vec::with_capacity(seen_ids.len());
    for compound_id in seen_ids {
        let Some(compound) = analyzed.remove(&compound_id) else {
            if let Some(compound) = unresolved.remove(&compound_id) {
                compounds.push(compound);
            }
            continue;
        };
        let deduplication_eligible = compound.deduplication_eligible
            && compound
                .canonical_smiles
                .as_deref()
                .is_some_and(|smiles| !smiles.is_empty());
        let structure_hash = if deduplication_eligible {
            compound.canonical_smiles.as_ref().map(|smiles| {
                let identity = format!("{IDENTITY_POLICY}\n{}\n{smiles}", parsed.rdkit_version);
                format!("{:x}", Sha256::digest(identity.as_bytes()))
            })
        } else {
            None
        };
        let duplicate_of = structure_hash
            .as_ref()
            .and_then(|hash| seen.get(hash).cloned());
        if let Some(hash) = &structure_hash {
            seen.entry(hash.clone())
                .or_insert_with(|| compound.id.clone());
        }
        compounds.push(CompoundStructure {
            compound_id: compound.id,
            formula: compound.formula,
            canonical_smiles: compound.canonical_smiles,
            structure_hash,
            duplicate_of,
            deduplication_eligible,
            status: compound.status.or_else(|| {
                (!deduplication_eligible).then(|| {
                    "A complete canonical structure is unavailable for safe deduplication."
                        .to_owned()
                })
            }),
        });
    }
    Ok(StructureReport {
        rdkit_version: parsed.rdkit_version,
        compounds,
    })
}

fn unavailable_compound(compound_id: &str, status: &str) -> CompoundStructure {
    CompoundStructure {
        compound_id: compound_id.to_owned(),
        formula: None,
        canonical_smiles: None,
        structure_hash: None,
        duplicate_of: None,
        deduplication_eligible: false,
        status: Some(status.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWING: &str = concat!(
        "<CDXML><page id='1'>",
        "<group id='10'><fragment id='11'><n id='12'/></fragment>",
        "<t id='13'><s>[[@x]]</s></t></group>",
        "<group id='20'><fragment id='21'><n id='22'/></fragment>",
        "<t id='23'><s>[[@y]]</s></t></group>",
        "<group id='30'><fragment id='31'><n id='32'/></fragment>",
        "<t id='33'><s>[[@z]]</s></t></group>",
        "<group id='40'><fragment id='41'><n id='42'/></fragment>",
        "<t id='43'><s>[[@salt]]</s></t><group id='44'>",
        "<fragment id='45'><n id='46'/></fragment></group></group>",
        "</page></CDXML>"
    );

    const UNGROUPED_DRAWING: &str =
        "<CDXML><page><fragment id='1'/><t><s>[[@loose]]</s></t></page></CDXML>";

    fn rdkit_report(run: impl FnOnce(&str) -> String) -> StructureReport {
        analyze_compounds(DRAWING, |script, input| {
            assert!(script.contains("MolsFromCDXML"));
            let request: serde_json::Value = serde_json::from_str(input).unwrap();
            let groups = request["groups"].as_array().unwrap();
            assert_eq!(groups.len(), 4);
            assert_eq!(groups[0]["fragments"], serde_json::json!(["11"]));
            assert_eq!(groups[3]["fragments"], serde_json::json!(["41", "45"]));
            Ok(run(input))
        })
        .unwrap()
    }

    #[test]
    fn groups_components_and_reports_exact_duplicates_without_guessing_stereo() {
        let report = rdkit_report(|_| {
            serde_json::json!({
                "rdkit_version": RDKIT_VERSION,
                "compounds": [
                    { "id": "x", "formula": "CH4", "canonical_smiles": "C", "deduplication_eligible": true, "status": null },
                    { "id": "y", "formula": "CH4", "canonical_smiles": "C", "deduplication_eligible": true, "status": null },
                    { "id": "z", "formula": "C2H6", "canonical_smiles": "CC", "deduplication_eligible": false, "status": "Unspecified stereochemistry." },
                    { "id": "salt", "formula": "H2NaO", "canonical_smiles": "O.[Na+]", "deduplication_eligible": true, "status": null }
                ]
            })
            .to_string()
        });
        assert_eq!(Some("x"), report.compounds[1].duplicate_of.as_deref());
        assert_eq!(None, report.compounds[2].structure_hash);
        assert_eq!(None, report.compounds[2].duplicate_of);
        assert_eq!(
            Some("O.[Na+]"),
            report.compounds[3].canonical_smiles.as_deref()
        );
    }

    #[test]
    fn rejects_a_different_rdkit_version_and_invalid_runner_output() {
        let version = analyze_compounds(DRAWING, |_, _| {
            Ok(serde_json::json!({ "rdkit_version": "0", "compounds": [] }).to_string())
        });
        assert!(version.unwrap_err().contains("version mismatch"));

        let invalid = analyze_compounds(DRAWING, |_, _| Ok("not JSON".into()));
        assert!(invalid.unwrap_err().contains("invalid structure report"));

        let incomplete = analyze_compounds(DRAWING, |_, _| {
            Ok(serde_json::json!({
                "rdkit_version": RDKIT_VERSION,
                "compounds": []
            })
            .to_string())
        });
        assert!(incomplete.unwrap_err().contains("exactly one result"));
    }

    #[test]
    fn drawings_without_numbered_groups_do_not_launch_rdkit() {
        let launched = std::cell::Cell::new(false);
        let report = analyze_compounds("<CDXML><page><fragment/></page></CDXML>", |_, _| {
            launched.set(true);
            Ok(String::new())
        })
        .unwrap();
        assert!(!launched.get());
        assert!(report.compounds.is_empty());
    }

    #[test]
    fn ungrouped_or_ambiguous_labels_are_reported_without_rdkit_guessing() {
        let report =
            analyze_compounds(UNGROUPED_DRAWING, |_, _| panic!("RDKit must not run")).unwrap();
        assert_eq!("loose", report.compounds[0].compound_id);
        assert!(!report.compounds[0].deduplication_eligible);
        assert!(report.compounds[0].status.is_some());
    }

    #[test]
    fn grouped_source_reports_multiple_fragments_and_can_be_sent_to_rdkit() {
        let source = concat!(
            "<CDXML><page><group id='10'>",
            "<fragment id='11'><n id='12' Element='6'/><n id='13' Element='8'/>",
            "<b id='14' B='12' E='13' Order='1'/></fragment>",
            "<group><fragment id='21'><n id='22' Element='7'/></fragment></group>",
            "<t id='30'><s>[[@x]]</s></t></group></page></CDXML>"
        );
        let mut invoked = false;
        let report = analyze_compounds(source, |_, input| {
            invoked = true;
            let request: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(
                request["groups"][0]["fragments"],
                serde_json::json!(["11", "21"])
            );
            Ok(serde_json::json!({
                "rdkit_version": RDKIT_VERSION,
                "compounds": [{
                    "id": "x", "formula": "CHNO", "canonical_smiles": "N.O=C",
                    "deduplication_eligible": true, "status": null
                }]
            })
            .to_string())
        })
        .unwrap();
        assert!(invoked);
        assert_eq!("CHNO", report.compounds[0].formula.as_deref().unwrap());
    }
}
