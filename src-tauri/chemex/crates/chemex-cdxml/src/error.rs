use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("CDXML exceeds the {limit}-byte source limit")]
    SourceTooLarge { limit: usize },
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("expected an unnamespaced CDXML root element")]
    WrongRoot,
    #[error("unsupported XML encoding {0}; convert a copy to UTF-8 first")]
    Encoding(String),
    #[error("dynamic field in text object {object}: {message}")]
    Field { object: usize, message: String },
    #[error("invalid compound key {0:?}; use ASCII letters, digits, '.', '_', ':' or '-'")]
    InvalidKey(String),
    #[error("compound {0:?} has an empty or multiline/control-character label")]
    InvalidLabel(String),
    #[error("compound key {0:?} occurs more than once in the mapping")]
    DuplicateKey(String),
    #[error("no display label for compound {0:?}")]
    MissingValue(String),
    #[error("compound {0:?} has no ChemDraw group")]
    MissingGroup(String),
    #[error("compound {0:?} is associated with multiple ChemDraw groups")]
    AmbiguousGroup(String),
    #[error("cannot safely patch text object {object}: {message}")]
    TextMapping { object: usize, message: String },
}
