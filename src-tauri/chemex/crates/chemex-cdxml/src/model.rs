use std::collections::BTreeMap;
use std::ops::Range;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Document,
    Page,
    Fragment,
    Node,
    Bond,
    Text,
    Group,
    Graphic,
    Curve,
    ObjectTag,
    Other,
}

#[derive(Clone, Debug, Serialize)]
pub struct Object {
    pub kind: ObjectKind,
    pub element: String,
    pub id: Option<String>,
    pub parent: Option<usize>,
    pub page: Option<usize>,
    pub attributes: BTreeMap<String, String>,
    pub source_range: Range<usize>,
}

impl Object {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Atom {
    pub object: usize,
    pub atomic_number: Option<u16>,
    pub charge: Option<i32>,
    pub isotope: Option<u16>,
    pub position: Option<[f64; 2]>,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Bond {
    pub object: usize,
    pub begin: Option<String>,
    pub end: Option<String>,
    /// Authored CDXML token; unusual and query orders are not coerced.
    pub order: String,
    pub display: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Fragment {
    pub object: usize,
    /// A CDXML fragment is an authored container, not a compound identity.
    pub atoms: Vec<Atom>,
    pub bonds: Vec<Bond>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TextObject {
    pub object: usize,
    pub text: String,
    pub atom_label: bool,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectTag {
    pub object: usize,
    pub owner: Option<usize>,
    pub name: Option<String>,
    pub value: Option<String>,
    pub tag_type: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub object: usize,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DynamicField {
    pub text_object: usize,
    pub compound_id: String,
    pub prefix: String,
    pub suffix: String,
    /// Explicit ChemDraw group containing a standalone compound label.
    pub group_object: Option<usize>,
    /// Authored fragments in that group; this does not assert chemical identity.
    pub fragment_objects: Vec<usize>,
    /// UTF-8 byte range in the decoded text, not an XML byte offset.
    pub text_range: Range<usize>,
    /// Decoded range replaced by the display label.
    pub value_range: Range<usize>,
    #[serde(skip)]
    pub(crate) syntax_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Serialize)]
pub struct DrawingDocument {
    #[serde(skip)]
    pub(crate) source: String,
    pub(crate) objects: Vec<Object>,
    pub(crate) fragments: Vec<Fragment>,
    pub(crate) texts: Vec<TextObject>,
    pub(crate) object_tags: Vec<ObjectTag>,
    pub(crate) fields: Vec<DynamicField>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    pub(crate) xml_nodes: Vec<roxmltree::NodeId>,
    #[serde(skip)]
    pub(crate) object_ids: BTreeMap<String, Option<usize>>,
}

impl DrawingDocument {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }

    pub fn texts(&self) -> &[TextObject] {
        &self.texts
    }

    pub fn object_tags(&self) -> &[ObjectTag] {
        &self.object_tags
    }

    pub fn fields(&self) -> &[DynamicField] {
        &self.fields
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Numeric ChemDraw IDs are local object addresses, never permanent IDs.
    /// Ambiguous duplicate IDs deliberately do not resolve.
    pub fn object_by_id(&self, id: &str) -> Option<&Object> {
        let index = self.object_ids.get(id).copied().flatten()?;
        self.objects.get(index)
    }

    /// Resolve a compound label to its authored, unambiguous group.
    pub fn compound_group(&self, compound_id: &str) -> crate::Result<&Object> {
        self.compound_group_index(compound_id)
            .map(|index| &self.objects[index])
    }

    fn compound_group_index(&self, compound_id: &str) -> crate::Result<usize> {
        let mut matching_groups: Vec<_> = self
            .fields
            .iter()
            .filter(|field| field.compound_id == compound_id)
            .filter_map(|field| field.group_object)
            .collect();
        if matching_groups.len()
            != self
                .fields
                .iter()
                .filter(|field| field.compound_id == compound_id)
                .count()
        {
            return Err(crate::Error::MissingGroup(compound_id.into()));
        }
        matching_groups.sort_unstable();
        matching_groups.dedup();
        let [group] = matching_groups.as_slice() else {
            return Err(if matching_groups.is_empty() {
                crate::Error::MissingGroup(compound_id.into())
            } else {
                crate::Error::AmbiguousGroup(compound_id.into())
            });
        };

        self.objects[*group]
            .page
            .map(|_| *group)
            .ok_or_else(|| crate::Error::MissingGroup(compound_id.into()))
    }

    /// Return a source-preserving CDXML document containing one numbered group.
    pub fn compound_group_source(&self, compound_id: &str) -> crate::Result<String> {
        let group = self.compound_group_index(compound_id)?;
        let page = self.objects[group]
            .page
            .ok_or_else(|| crate::Error::MissingGroup(compound_id.into()))?;
        let mut removed = Vec::new();
        removed.extend(
            self.objects
                .iter()
                .filter(|object| object.kind == ObjectKind::Page && object.page != Some(page))
                .map(|object| object.source_range.clone()),
        );
        let mut current = group;
        while current != page {
            let parent = self.objects[current]
                .parent
                .ok_or_else(|| crate::Error::MissingGroup(compound_id.into()))?;
            removed.extend(
                self.objects
                    .iter()
                    .filter(|object| {
                        object.parent == Some(parent)
                            && object.source_range != self.objects[current].source_range
                    })
                    .map(|object| object.source_range.clone()),
            );
            current = parent;
        }
        removed.sort_by_key(|range| std::cmp::Reverse(range.start));
        removed.dedup();

        let mut source = self.source.clone();
        for range in removed {
            source.replace_range(range, "");
        }
        Ok(source)
    }

    /// Return a source-preserving document for one group without its dynamic labels.
    pub fn compound_group_source_without_fields(&self, compound_id: &str) -> crate::Result<String> {
        let source = self.compound_group_source(compound_id)?;
        let document = Self::parse(source)?;
        document.strip_fields()
    }

    pub(crate) fn warn(&mut self, code: &str, object: usize, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: code.into(),
            object,
            message: message.into(),
        });
    }
}
