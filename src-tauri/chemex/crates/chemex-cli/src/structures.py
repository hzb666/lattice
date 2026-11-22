import json
import sys

from rdkit import Chem, rdBase
from rdkit.Chem import rdMolDescriptors


def analyze(request):
    if not request["groups"]:
        return {"rdkit_version": rdBase.rdkitVersion, "compounds": []}
    parameters = Chem.CDXMLParserParams()
    parameters.parseQueries = True
    parameters.strictQueryParsing = True
    requested_ids = {
        fragment_id
        for group in request["groups"]
        for fragment_id in group["fragments"]
    }
    parsed = {}
    ambiguous_ids = set()

    try:
        molecules = Chem.MolsFromCDXML(request["source"], parameters)
        for molecule in molecules:
            fragment_id = next(
                (
                    molecule.GetProp(name)
                    for name in ("CDXML_FRAG_ID", "CDX_FRAG_ID")
                    if molecule.HasProp(name)
                ),
                None,
            )
            if fragment_id not in requested_ids:
                continue
            if fragment_id in ambiguous_ids:
                continue
            if fragment_id in parsed:
                ambiguous_ids.add(fragment_id)
                del parsed[fragment_id]
                continue
            stereo = Chem.FindPotentialStereo(molecule, cleanIt=True, flagPossible=True)
            has_unspecified_stereo = any(
                info.specified != Chem.StereoSpecified.Specified for info in stereo
            )
            has_query = any(atom.HasQuery() for atom in molecule.GetAtoms()) or any(
                bond.HasQuery() for bond in molecule.GetBonds()
            )
            parsed[fragment_id] = (
                molecule,
                has_unspecified_stereo or has_query or bool(molecule.GetStereoGroups()),
            )
    except Exception as error:
        parse_error = str(error) or type(error).__name__
    else:
        parse_error = None

    compounds = []
    for group in request["groups"]:
        parts = [
            None if fragment_id in ambiguous_ids else parsed.get(fragment_id)
            for fragment_id in group["fragments"]
        ]
        missing = parse_error is not None or any(part is None for part in parts)
        if missing:
            compounds.append(
                {
                    "id": group["id"],
                    "formula": None,
                    "canonical_smiles": None,
                    "deduplication_eligible": False,
                    "status": parse_error or "Some structure fragments could not be parsed unambiguously.",
                }
            )
            continue

        try:
            molecules_in_group = [part[0] for part in parts]
            combined = Chem.Mol(molecules_in_group[0])
            for molecule in molecules_in_group[1:]:
                combined = Chem.CombineMols(combined, molecule)
            smiles = Chem.MolToSmiles(combined, canonical=True, isomericSmiles=True)
            formula = rdMolDescriptors.CalcMolFormula(combined)
        except Exception as error:
            compounds.append(
                {
                    "id": group["id"],
                    "formula": None,
                    "canonical_smiles": None,
                    "deduplication_eligible": False,
                    "status": str(error) or type(error).__name__,
                }
            )
            continue
        eligible = not any(part[1] for part in parts)
        compounds.append({
            "id": group["id"],
            "formula": formula,
            "canonical_smiles": smiles,
            "deduplication_eligible": eligible,
            "status": None if eligible else "Unspecified stereochemistry or query atoms prevent safe deduplication.",
        })
    return {"rdkit_version": rdBase.rdkitVersion, "compounds": compounds}


with open(sys.argv[1], encoding="utf-8") as request_file:
    request = json.load(request_file)

if "requests" in request:
    result = {"reports": [analyze(item) for item in request["requests"]]}
else:
    result = analyze(request)
print(json.dumps(result))
