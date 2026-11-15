use ferrix_types::{DocId, PageIndex};
use serde::{Deserialize, Serialize};

/// Errors reported by an engine backend. Serializable, so messages are owned strings.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum EngineError {
    #[error("could not read file: {0}")]
    Io(String),
    #[error("the document is password protected or the password is wrong")]
    PasswordRequired,
    #[error("not a valid PDF: {0}")]
    InvalidDocument(String),
    #[error("{0} is not open")]
    UnknownDocument(DocId),
    #[error("page {page} does not exist (document has {count} pages)")]
    PageOutOfRange { page: PageIndex, count: u32 },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("{0} is not supported by this engine")]
    Unsupported(String),
    #[error("engine failure: {0}")]
    Backend(String),
}
