use std::collections::{BTreeMap, BTreeSet};

use crate::{DrawingDocument, DynamicField, ObjectKind};

#[derive(Default)]
struct Group {
    keys: BTreeSet<String>,
    fragments: Vec<usize>,
}

pub(crate) fn standalone(text: &str, fields: &[&DynamicField]) -> bool {
    fields.len() == 1 && fields[0].text_range == (0..text.len())
}

pub(crate) fn associate(document: &mut DrawingDocument) {
    let text_lengths: BTreeMap<_, _> = document
        .texts
        .iter()
        .map(|text| (text.object, text.text.len()))
        .collect();
    let mut owners = Vec::with_capacity(document.objects.len());
    for object in &document.objects {
        owners.push(object.parent.and_then(|parent| {
            if document.objects[parent].kind == ObjectKind::Group {
                Some(parent)
            } else {
                owners[parent]
            }
        }));
    }
    let mut groups: BTreeMap<usize, Group> = BTreeMap::new();
    for field in &mut document.fields {
        if field.text_range != (0..text_lengths[&field.text_object]) {
            continue;
        }
        field.group_object = owners[field.text_object];
        if let Some(group) = field.group_object {
            groups
                .entry(group)
                .or_default()
                .keys
                .insert(field.compound_id.clone());
        }
    }
    collect_fragments(document, &owners, &mut groups);
    for field in &mut document.fields {
        if let Some(group) = field.group_object.and_then(|owner| groups.get(&owner)) {
            if group.keys.len() == 1 {
                field.fragment_objects.clone_from(&group.fragments);
            }
        }
    }
    for (object, group) in groups {
        if group.keys.len() > 1 {
            document.warn(
                "ambiguous_field_group",
                object,
                "different compound keys in one Group; group each structure with its own label",
            );
        }
    }
}

fn collect_fragments(
    document: &DrawingDocument,
    owners: &[Option<usize>],
    groups: &mut BTreeMap<usize, Group>,
) {
    for fragment in &document.fragments {
        let object = &document.objects[fragment.object];
        if crate::chemistry::ancestor(document, object, ObjectKind::Fragment)
            || crate::chemistry::ancestor(document, object, ObjectKind::Other)
        {
            continue;
        }
        let mut owner = owners[fragment.object];
        while let Some(index) = owner {
            if let Some(group) = groups.get_mut(&index) {
                group.fragments.push(fragment.object);
                break;
            }
            owner = owners[index];
        }
    }
}
