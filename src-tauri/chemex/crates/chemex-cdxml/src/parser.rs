use std::collections::{BTreeMap, HashMap};

use roxmltree::{Document, Node, ParsingOptions};

use crate::{DrawingDocument, Error, MAX_SOURCE_BYTES, Object, ObjectKind, ObjectTag, Result};

pub(crate) fn xml(source: &str) -> Result<Document<'_>> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Error::SourceTooLarge {
            limit: MAX_SOURCE_BYTES,
        });
    }
    let options = ParsingOptions {
        // ChemDraw writes DOCTYPE. No resolver means no network or file reads.
        allow_dtd: true,
        nodes_limit: 250_000,
        entity_resolver: None,
    };
    let xml = Document::parse_with_options(source, options)?;
    let root = xml.root_element();
    if root.tag_name().name() != "CDXML" || root.tag_name().namespace().is_some() {
        return Err(Error::WrongRoot);
    }
    Ok(xml)
}

impl DrawingDocument {
    pub fn parse(source: String) -> Result<Self> {
        check_encoding(&source)?;
        let xml = xml(&source)?;
        let mut document = Self {
            source: String::new(),
            objects: Vec::new(),
            fragments: Vec::new(),
            texts: Vec::new(),
            object_tags: Vec::new(),
            fields: Vec::new(),
            diagnostics: Vec::new(),
            xml_nodes: Vec::new(),
            object_ids: BTreeMap::new(),
        };
        index_objects(&xml, &mut document);
        crate::chemistry::extract(&xml, &mut document);
        crate::text::extract(&xml, &mut document);
        crate::fields::discover(&mut document);
        drop(xml);
        document.source = source;
        Ok(document)
    }
}

fn index_objects(xml: &Document<'_>, document: &mut DrawingDocument) {
    let mut indices = HashMap::new();
    for node in xml.descendants().filter(Node::is_element) {
        let index = document.objects.len();
        let parent = node
            .parent()
            .and_then(|parent| indices.get(&parent.id()).copied());
        let kind = if node.tag_name().namespace().is_none() {
            object_kind(node.tag_name().name())
        } else {
            ObjectKind::Other
        };
        let page = if kind == ObjectKind::Page {
            Some(index)
        } else {
            parent.and_then(|parent: usize| document.objects[parent].page)
        };
        let object = object(node, parent, page);
        index_id(document, &object, index);
        indices.insert(node.id(), index);
        document.xml_nodes.push(node.id());
        document.objects.push(object);
        if node.has_tag_name("objecttag") {
            document.object_tags.push(object_tag(node, index, parent));
        }
    }
}

fn index_id(document: &mut DrawingDocument, object: &Object, index: usize) {
    if object.kind == ObjectKind::Other {
        return;
    }
    let Some(id) = &object.id else { return };
    if document
        .object_ids
        .insert(id.clone(), Some(index))
        .is_some()
    {
        document.object_ids.insert(id.clone(), None);
        document.warn(
            "duplicate_object_id",
            index,
            format!("duplicate CDXML object ID {id}"),
        );
    }
}

fn object(node: Node<'_, '_>, parent: Option<usize>, page: Option<usize>) -> Object {
    let attributes = node
        .attributes()
        .map(|attribute| {
            let name = attribute.namespace().map_or_else(
                || attribute.name().to_owned(),
                |namespace| format!("{{{namespace}}}{}", attribute.name()),
            );
            (name, attribute.value().to_owned())
        })
        .collect();
    Object {
        kind: if node.tag_name().namespace().is_none() {
            object_kind(node.tag_name().name())
        } else {
            ObjectKind::Other
        },
        element: node.tag_name().name().to_owned(),
        id: node.attribute("id").map(str::to_owned),
        parent,
        page,
        attributes,
        source_range: node.range(),
    }
}

fn object_tag(node: Node<'_, '_>, object: usize, owner: Option<usize>) -> ObjectTag {
    ObjectTag {
        object,
        owner,
        name: node.attribute("Name").map(str::to_owned),
        value: node.attribute("Value").map(str::to_owned),
        tag_type: node.attribute("TagType").map(str::to_owned),
    }
}

fn object_kind(name: &str) -> ObjectKind {
    match name {
        "CDXML" => ObjectKind::Document,
        "page" => ObjectKind::Page,
        "fragment" => ObjectKind::Fragment,
        "n" => ObjectKind::Node,
        "b" => ObjectKind::Bond,
        "t" => ObjectKind::Text,
        "group" => ObjectKind::Group,
        "graphic" => ObjectKind::Graphic,
        "curve" => ObjectKind::Curve,
        "objecttag" => ObjectKind::ObjectTag,
        _ => ObjectKind::Other,
    }
}

fn check_encoding(source: &str) -> Result<()> {
    let source = source.trim_start_matches('\u{feff}');
    if !source.starts_with("<?xml") {
        return Ok(());
    }
    let Some(end) = source.find("?>") else {
        return Ok(());
    };
    let declaration = &source[..end];
    let Some(start) = declaration.find("encoding") else {
        return Ok(());
    };
    let value = declaration[start + "encoding".len()..].trim_start();
    let Some(value) = value.strip_prefix('=') else {
        return Ok(());
    };
    let value = value.trim_start();
    let Some(quote @ ('\'' | '"')) = value.chars().next() else {
        return Ok(());
    };
    if let Some(end) = value[1..].find(quote) {
        let encoding = &value[1..end + 1];
        if !encoding.eq_ignore_ascii_case("UTF-8") {
            return Err(Error::Encoding(encoding.into()));
        }
    }
    Ok(())
}
