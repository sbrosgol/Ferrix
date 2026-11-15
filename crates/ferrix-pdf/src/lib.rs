//! Object-level PDF writing (ADR-0001 §3, ADR-0003).
//!
//! PDFium renders; this crate writes. Structural edits are saved as an **incremental update**
//! appended to the original bytes, so existing content (and digital signatures over it) is
//! left byte-for-byte intact.

#![forbid(unsafe_code)]

mod layout;

pub use layout::{LayoutPage, write_page_layout};

/// A PDF to read pages from.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub bytes: &'a [u8],
    pub password: Option<&'a str>,
}

impl<'a> Source<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            password: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error("could not read the PDF structure: {0}")]
    Parse(String),
    #[error("the document is encrypted and could not be unlocked")]
    Encrypted,
    #[error("invalid page layout: {0}")]
    InvalidLayout(String),
    #[error("could not write the PDF: {0}")]
    Write(String),
}

impl From<lopdf::Error> for PdfError {
    fn from(error: lopdf::Error) -> Self {
        Self::Parse(error.to_string())
    }
}
