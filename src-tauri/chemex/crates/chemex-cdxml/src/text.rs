use std::ops::Range;

use roxmltree::{Document, Node};

use crate::{DrawingDocument, ObjectKind, TextObject};

pub(crate) struct Unit {
    pub decoded: Range<usize>,
    pub raw: Range<usize>,
    pub cdata: bool,
}

impl Unit {
    pub fn intersection(&self, field: &Range<usize>) -> Option<Range<usize>> {
        let start = self.decoded.start.max(field.start);
        let end = self.decoded.end.min(field.end);
        if start >= end {
            return None;
        }
        if self.raw.len() == self.decoded.len() {
            Some(
                self.raw.start + start - self.decoded.start
                    ..self.raw.start + end - self.decoded.start,
            )
        } else if start == self.decoded.start && end == self.decoded.end {
            Some(self.raw.clone())
        } else {
            None
        }
    }
}

#[derive(Default)]
pub(crate) struct MappedText {
    pub text: String,
    pub units: Vec<Unit>,
}

pub(crate) fn extract(xml: &Document<'_>, document: &mut DrawingDocument) {
    for index in 0..document.objects.len() {
        if document.objects[index].kind != ObjectKind::Text {
            continue;
        }
        if crate::chemistry::ancestor(document, &document.objects[index], ObjectKind::Text) {
            continue;
        }
        let Some(node) = xml.get_node(document.xml_nodes[index]) else {
            continue;
        };
        let object = &document.objects[index];
        let atom_label = crate::chemistry::ancestor(document, object, ObjectKind::Node);
        let read_only = atom_label
            || crate::chemistry::ancestor(document, object, ObjectKind::Other)
            || crate::chemistry::ancestor(document, object, ObjectKind::ObjectTag);
        document.texts.push(TextObject {
            object: index,
            text: decoded(node),
            atom_label,
            read_only,
        });
    }
}

pub(crate) fn decoded(node: Node<'_, '_>) -> String {
    node.children()
        .filter(|child| child.has_tag_name("s"))
        .flat_map(|run| run.descendants())
        .filter(|child| child.is_text())
        .filter_map(|child| child.text())
        .collect()
}

pub(crate) fn map(node: Node<'_, '_>, source: &str) -> std::result::Result<MappedText, String> {
    let mut mapped = MappedText::default();
    for run in node.children().filter(|child| child.has_tag_name("s")) {
        if run.children().any(|child| child.is_element()) {
            return Err("nested elements in a style run are not a supported field location".into());
        }
        let range = run.range();
        let raw = &source[range.clone()];
        let start = tag_end(raw)?;
        if raw[..start].trim_end_matches('>').trim_end().ends_with('/') {
            continue;
        }
        let end = raw.rfind("</").ok_or("missing run closing tag")?;
        scan(&raw[start..end], range.start + start, &mut mapped)?;
    }
    if mapped.text != decoded(node) {
        return Err("text and source offsets disagree; no output was changed".into());
    }
    Ok(mapped)
}

fn scan(raw: &str, base: usize, mapped: &mut MappedText) -> std::result::Result<(), String> {
    let mut offset = 0;
    while offset < raw.len() {
        let rest = &raw[offset..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").ok_or("unclosed CDATA")?;
            scan_chars(&body[..end], base + offset + 9, true, mapped)?;
            offset += 9 + end + 3;
        } else if let Some(body) = rest.strip_prefix("<!--") {
            offset += 4 + body.find("-->").ok_or("unclosed comment")? + 3;
        } else if let Some(body) = rest.strip_prefix("<?") {
            offset += 2 + body.find("?>").ok_or("unclosed processing instruction")? + 2;
        } else if rest.starts_with('<') {
            offset += tag_end(rest)?;
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            scan_chars(&rest[..end], base + offset, false, mapped)?;
            offset += end;
        }
    }
    Ok(())
}

fn scan_chars(
    raw: &str,
    base: usize,
    cdata: bool,
    mapped: &mut MappedText,
) -> std::result::Result<(), String> {
    let mut offset = 0;
    while let Some(character) = raw[offset..].chars().next() {
        let rest = &raw[offset..];
        let (character, length) = if character == '&' && !cdata {
            let end = rest.find(';').ok_or("unclosed entity")?;
            (entity(&rest[1..end])?, end + 1)
        } else if character == '\r' {
            ('\n', if rest.starts_with("\r\n") { 2 } else { 1 })
        } else {
            (character, character.len_utf8())
        };
        let start = mapped.text.len();
        mapped.text.push(character);
        let unit = Unit {
            decoded: start..mapped.text.len(),
            raw: base + offset..base + offset + length,
            cdata,
        };
        if let Some(previous) = mapped.units.last_mut().filter(|previous| {
            previous.cdata == cdata
                && previous.raw.end == unit.raw.start
                && previous.raw.len() == previous.decoded.len()
                && unit.raw.len() == unit.decoded.len()
        }) {
            previous.raw.end = unit.raw.end;
            previous.decoded.end = unit.decoded.end;
        } else {
            mapped.units.push(unit);
        }
        offset += length;
    }
    Ok(())
}

fn entity(name: &str) -> std::result::Result<char, String> {
    let predefined = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    };
    if let Some(character) = predefined {
        return Ok(character);
    }
    let number = if let Some(hex) = name.strip_prefix("#x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        name.strip_prefix('#')
            .and_then(|decimal| decimal.parse().ok())
    };
    number.and_then(char::from_u32).ok_or_else(|| {
        format!("entity &{name}; has no unambiguous character mapping; use a plain-text field")
    })
}

fn tag_end(raw: &str) -> std::result::Result<usize, String> {
    let mut quote = None;
    for (index, character) in raw.char_indices() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(current), character) if current == character => quote = None,
            (None, '>') => return Ok(index + 1),
            _ => {}
        }
    }
    Err("missing tag delimiter".into())
}
