use std::collections::BTreeMap;
use std::ops::Range;

use crate::{DrawingDocument, DynamicField, Error, Result};

const PREFIX: &str = "[[@";

#[derive(Debug, Default)]
pub struct FieldValues {
    pub(crate) labels: BTreeMap<String, String>,
}

impl FieldValues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, compound_id: String, label: String) -> Result<()> {
        validate_key(&compound_id)?;
        if label.trim().is_empty()
            || label.chars().any(|character| {
                character.is_control()
                    || matches!(character, '\u{fffe}' | '\u{ffff}' | '\u{2028}' | '\u{2029}')
            })
        {
            return Err(Error::InvalidLabel(compound_id));
        }
        if self.labels.contains_key(&compound_id) {
            return Err(Error::DuplicateKey(compound_id));
        }
        self.labels.insert(compound_id, label);
        Ok(())
    }
}

pub(crate) fn discover(document: &mut DrawingDocument) {
    for index in 0..document.texts.len() {
        let text = &document.texts[index];
        if text.read_only {
            if text.text.contains(PREFIX) {
                let object = text.object;
                let code = if text.atom_label {
                    "field_in_atom_label"
                } else {
                    "field_in_read_only_text"
                };
                document.warn(
                    code,
                    object,
                    "compound field in chemical, metadata or unknown text is read-only",
                );
            }
            continue;
        }
        let object = text.object;
        let (fields, issues) = scan_fields(&text.text, object);
        document.fields.extend(fields);
        for issue in issues {
            document.warn("invalid_field", object, issue);
        }
    }
    crate::grouping::associate(document);
    crate::field_layout::diagnose(document);
}

fn scan_fields(text: &str, object: usize) -> (Vec<DynamicField>, Vec<String>) {
    let mut fields = Vec::new();
    let mut issues = Vec::new();
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find(PREFIX).map(|offset| cursor + offset) {
        let key_start = start + PREFIX.len();
        let Some(end) = text[key_start..]
            .find("]]")
            .map(|offset| key_start + offset)
        else {
            issues.push("unclosed [[@...]] field".into());
            break;
        };
        let token = &text[key_start..end];
        let parsed = token.split_once('_').and_then(|(key, tail)| {
            tail.split_once('^')
                .map(|(prefix, suffix)| (key, prefix, suffix))
        });
        let (key, prefix, suffix) = parsed.unwrap_or((token, "", ""));
        let key_end = key_start + key.len();
        let (value_range, syntax_ranges) = match parsed {
            Some((_, prefix, _)) => {
                let prefix_start = key_end + 1;
                let caret = prefix_start + prefix.len();
                (caret..caret + 1, vec![start..prefix_start, end..end + 2])
            }
            None => (start..end + 2, vec![]),
        };
        match validate_key(key) {
            Ok(()) => fields.push(DynamicField {
                text_object: object,
                compound_id: key.into(),
                prefix: prefix.into(),
                suffix: suffix.into(),
                group_object: None,
                fragment_objects: Vec::new(),
                text_range: start..end + 2,
                value_range,
                syntax_ranges,
            }),
            Err(error) => issues.push(error.to_string()),
        }
        cursor = end + 2;
    }
    (fields, issues)
}

pub(crate) fn validate_key(key: &str) -> Result<()> {
    let mut parts = key.split('|');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    };
    if !valid_part(parts.next().unwrap_or_default())
        || parts.next().is_some_and(|part| !valid_part(part))
        || parts.next().is_some()
    {
        return Err(Error::InvalidKey(key.into()));
    }
    Ok(())
}

pub(crate) struct Patch {
    pub range: Range<usize>,
    pub text: String,
}

impl DrawingDocument {
    /// Remove all dynamic field text from a new string. The authoritative source is never mutated.
    pub fn strip_fields(&self) -> Result<String> {
        let xml = crate::parser::xml(&self.source)?;
        let mut patches = Vec::new();
        let mut grouped: BTreeMap<usize, Vec<&DynamicField>> = BTreeMap::new();
        for field in &self.fields {
            grouped.entry(field.text_object).or_default().push(field);
        }
        for (object, fields) in grouped {
            let failure = |message: String| Error::TextMapping { object, message };
            let node = xml
                .get_node(self.xml_nodes[object])
                .ok_or_else(|| failure("missing XML node".into()))?;
            let mapped = crate::text::map(node, &self.source).map_err(failure)?;
            for field in fields {
                for unit in &mapped.units {
                    if let Some(range) = unit.intersection(&field.value_range) {
                        patches.push(Patch {
                            range,
                            text: String::new(),
                        });
                    }
                }
                for remove_range in &field.syntax_ranges {
                    for unit in &mapped.units {
                        if let Some(range) = unit.intersection(remove_range) {
                            patches.push(Patch {
                                range,
                                text: String::new(),
                            });
                        }
                    }
                }
            }
        }
        patches.sort_by_key(|patch| std::cmp::Reverse(patch.range.start));
        let mut output = self.source.clone();
        for patch in patches {
            output.replace_range(patch.range, &patch.text);
        }
        crate::parser::xml(&output)?;
        Ok(output)
    }

    /// Replace fields in a new string. The authoritative source is never mutated.
    pub fn render_fields(&self, values: &FieldValues) -> Result<String> {
        if let Some(issue) = self
            .diagnostics
            .iter()
            .find(|issue| issue.code == "invalid_field")
        {
            return Err(Error::Field {
                object: issue.object,
                message: issue.message.clone(),
            });
        }
        let xml = crate::parser::xml(&self.source)?;
        let mut grouped: BTreeMap<usize, Vec<&DynamicField>> = BTreeMap::new();
        for field in &self.fields {
            grouped.entry(field.text_object).or_default().push(field);
        }
        let mut patches = Vec::new();
        for (object, fields) in grouped {
            let failure = |message: String| Error::TextMapping { object, message };
            let node = xml
                .get_node(self.xml_nodes[object])
                .ok_or_else(|| failure("missing XML node".into()))?;
            let mapped = crate::text::map(node, &self.source).map_err(failure)?;
            patches.extend(field_patches(&mapped, &fields, values)?);
            if crate::grouping::standalone(&mapped.text, &fields) {
                if let Some(position) = crate::field_layout::center_position(self, object) {
                    patches.extend(crate::field_layout::patches(node, &position));
                }
            }
        }
        patches.sort_by_key(|patch| std::cmp::Reverse(patch.range.start));
        let mut output = self.source.clone();
        for patch in patches {
            output.replace_range(patch.range, &patch.text);
        }
        crate::parser::xml(&output)?;
        Ok(output)
    }
}

fn field_patches(
    mapped: &crate::text::MappedText,
    fields: &[&DynamicField],
    values: &FieldValues,
) -> Result<Vec<Patch>> {
    let mut patches = Vec::new();
    for field in fields {
        let label = values
            .labels
            .get(&field.compound_id)
            .ok_or_else(|| Error::MissingValue(field.compound_id.clone()))?;
        let spans: Vec<_> = mapped
            .units
            .iter()
            .filter_map(|unit| {
                unit.intersection(&field.value_range)
                    .map(|range| (range, unit.cdata))
            })
            .collect();
        if spans.is_empty() {
            return Err(Error::TextMapping {
                object: field.text_object,
                message: "field has no source bytes".into(),
            });
        }
        for (index, (range, cdata)) in spans.into_iter().enumerate() {
            patches.push(Patch {
                range,
                text: if index == 0 {
                    xml_label(label, cdata)
                } else {
                    String::new()
                },
            });
        }
        for remove_range in &field.syntax_ranges {
            for unit in &mapped.units {
                if let Some(range) = unit.intersection(remove_range) {
                    patches.push(Patch {
                        range,
                        text: String::new(),
                    });
                }
            }
        }
    }
    Ok(patches)
}

fn xml_label(label: &str, cdata: bool) -> String {
    if cdata {
        return label.replace("]]>", "]]]]><![CDATA[>");
    }
    label
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
