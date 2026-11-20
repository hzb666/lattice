use std::collections::BTreeMap;

use roxmltree::Node;

use crate::{DrawingDocument, fields::Patch};

pub(crate) fn diagnose(document: &mut DrawingDocument) {
    let lengths: BTreeMap<_, _> = document
        .texts
        .iter()
        .map(|text| (text.object, text.text.len()))
        .collect();
    let unsupported: Vec<_> = document
        .fields
        .iter()
        .filter(|field| {
            field.text_range == (0..lengths[&field.text_object])
                && center_position(document, field.text_object).is_none()
        })
        .map(|field| field.text_object)
        .collect();
    for object in unsupported {
        document.warn("field_alignment_unavailable", object,
            "label needs finite p/BoundingBox and zero rotation for centering; text replaced with authored geometry retained");
    }
}

pub(crate) fn patches(node: Node<'_, '_>, position: &str) -> Vec<Patch> {
    let mut patches = Vec::new();
    let mut added = String::new();
    for (name, value) in [
        ("p", position),
        ("CaptionJustification", "Center"),
        ("Justification", "Center"),
    ] {
        if let Some(attribute) = node.attribute_node(name) {
            patches.push(Patch {
                range: attribute.range(),
                text: format!("{name}=\"{value}\""),
            });
        } else {
            added.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    if !added.is_empty() {
        let start = node.range().start + "<t".len();
        patches.push(Patch {
            range: start..start,
            text: added,
        });
    }
    // These describe the old long placeholder; ChemDraw must measure the new label.
    for name in ["BoundingBox", "LineStarts"] {
        if let Some(attribute) = node.attribute_node(name) {
            patches.push(Patch {
                range: attribute.range(),
                text: String::new(),
            });
        }
    }
    patches
}

pub(crate) fn center_position(document: &DrawingDocument, index: usize) -> Option<String> {
    let object = &document.objects[index];
    let rotation = object
        .attribute("RotationAngle")
        .unwrap_or("0")
        .parse::<f64>()
        .ok()?;
    // shortcut: rotated captions retain authored geometry until native rotation tests exist.
    if rotation != 0.0 {
        return None;
    }
    let [_, _] = numbers::<2>(object.attribute("p")?)?;
    let [left, top, right, bottom] = numbers::<4>(object.attribute("BoundingBox")?)?;
    if left > right || top > bottom {
        return None;
    }
    let position: Vec<_> = object.attribute("p")?.split_whitespace().collect();
    let centered = inherited(document, index, "CaptionJustification")
        .or_else(|| inherited(document, index, "Justification"))
        == Some("Center");
    let x = if centered {
        position[0].to_owned()
    } else {
        (left / 2.0 + right / 2.0).to_string()
    };
    Some(format!("{x} {}", position[1]))
}

fn inherited<'a>(document: &'a DrawingDocument, mut index: usize, name: &str) -> Option<&'a str> {
    loop {
        let object = &document.objects[index];
        if let Some(value) = object.attribute(name) {
            return Some(value);
        }
        index = object.parent?;
    }
}

fn numbers<const N: usize>(value: &str) -> Option<[f64; N]> {
    let values: Vec<f64> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    let array: [f64; N] = values.try_into().ok()?;
    array.iter().all(|value| value.is_finite()).then_some(array)
}
