use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use roxmltree::{Document, Node};

use crate::{Atom, Bond, DrawingDocument, Fragment, Object, ObjectKind};

pub(crate) fn extract(xml: &Document<'_>, document: &mut DrawingDocument) {
    let indices: HashMap<_, _> = document
        .xml_nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (*node, index))
        .collect();
    for node in xml
        .descendants()
        .filter(|node| node.has_tag_name("fragment"))
    {
        let object = indices[&node.id()];
        let mut fragment = Fragment {
            object,
            atoms: Vec::new(),
            bonds: Vec::new(),
        };
        for child in node.children().filter(Node::is_element) {
            let index = indices[&child.id()];
            if child.has_tag_name("n") {
                fragment.atoms.push(atom(child, index, document));
            } else if child.has_tag_name("b") {
                fragment.bonds.push(bond(index, document));
            }
        }
        validate_bonds(&fragment, document);
        document.fragments.push(fragment);
    }
}

fn atom(node: Node<'_, '_>, object: usize, document: &mut DrawingDocument) -> Atom {
    let node_type = document.objects[object]
        .attribute("NodeType")
        .unwrap_or("Element")
        .to_owned();
    let element = if node_type == "Element" || node_type == "Unspecified" {
        if node.has_attribute("Element") {
            number(document, object, "Element")
        } else {
            Some(6)
        }
    } else {
        document.warn(
            "unsupported_node_type",
            object,
            format!("retained NodeType={node_type}"),
        );
        None
    };
    let element = element.filter(|number| (1..=118).contains(number));
    if element.is_none() && (node_type == "Element" || node_type == "Unspecified") {
        document.warn(
            "unknown_element",
            object,
            "element is outside 1..=118 or invalid",
        );
    }
    Atom {
        object,
        atomic_number: element,
        charge: if node.has_attribute("Charge") {
            number(document, object, "Charge")
        } else {
            Some(0)
        },
        isotope: number(document, object, "Isotope"),
        position: position(document, object),
        label: node
            .children()
            .filter(|child| child.has_tag_name("t"))
            .flat_map(|text| text.descendants())
            .filter(|child| child.is_text())
            .filter_map(|child| child.text())
            .collect(),
    }
}

fn bond(object: usize, document: &DrawingDocument) -> Bond {
    let attributes = &document.objects[object];
    Bond {
        object,
        begin: attributes.attribute("B").map(str::to_owned),
        end: attributes.attribute("E").map(str::to_owned),
        order: attributes.attribute("Order").unwrap_or("1").to_owned(),
        display: attributes.attribute("Display").map(str::to_owned),
    }
}

fn number<T: FromStr>(document: &mut DrawingDocument, object: usize, name: &str) -> Option<T> {
    let value = document.objects[object].attribute(name)?;
    match value.parse() {
        Ok(value) => Some(value),
        Err(_) => {
            let message = format!("invalid {name}={value:?}; raw attribute retained");
            document.warn("invalid_atom_attribute", object, message);
            None
        }
    }
}

fn position(document: &mut DrawingDocument, object: usize) -> Option<[f64; 2]> {
    let value = document.objects[object].attribute("p")?;
    let values: Vec<_> = value.split_whitespace().collect();
    let parsed = match values.as_slice() {
        [x, y] => x.parse::<f64>().ok().zip(y.parse::<f64>().ok()),
        _ => None,
    };
    match parsed {
        Some((x, y)) if x.is_finite() && y.is_finite() => Some([x, y]),
        _ => {
            document.warn(
                "invalid_coordinates",
                object,
                "coordinates are not two finite numbers",
            );
            None
        }
    }
}

fn validate_bonds(fragment: &Fragment, document: &mut DrawingDocument) {
    let ids: HashSet<_> = fragment
        .atoms
        .iter()
        .filter_map(|atom| document.objects[atom.object].id.clone())
        .collect();
    for bond in &fragment.bonds {
        let valid = bond
            .begin
            .as_ref()
            .zip(bond.end.as_ref())
            .is_some_and(|(begin, end)| {
                begin != end
                    && ids.contains(begin)
                    && ids.contains(end)
                    && document.object_by_id(begin).is_some()
                    && document.object_by_id(end).is_some()
            });
        if !valid {
            document.warn(
                "invalid_bond_reference",
                bond.object,
                "bond endpoints are missing, ambiguous or outside this fragment",
            );
        }
    }
}

pub(crate) fn ancestor(document: &DrawingDocument, object: &Object, kind: ObjectKind) -> bool {
    let mut parent = object.parent;
    while let Some(index) = parent {
        if document.objects[index].kind == kind {
            return true;
        }
        parent = document.objects[index].parent;
    }
    false
}
