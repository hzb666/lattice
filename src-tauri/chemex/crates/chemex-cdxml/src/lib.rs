//! Read-only CDXML inspection and source-preserving compound fields.

mod chemistry;
mod error;
mod field_layout;
mod fields;
mod grouping;
mod model;
mod parser;
mod text;

pub use error::{Error, Result};
pub use fields::FieldValues;
pub use model::{
    Atom, Bond, Diagnostic, DrawingDocument, DynamicField, Fragment, Object, ObjectKind, ObjectTag,
    TextObject,
};

/// Maximum supported UTF-8 source size; images embedded in CDXML count too.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
