//! Object-level PDF writing (ADR-0001 §3, ADR-0003).
//!
//! PDFium renders; this crate writes. Structural edits are saved as an **incremental update**
//! appended to the original bytes, so existing content (and digital signatures over it) is
//! left byte-for-byte intact.

#![forbid(unsafe_code)]

mod create;
mod layout;

pub use create::{ImageData, ImagePage, PaperSize, blank_document, image_document};
pub use layout::{LayoutPage, write_page_layout};

/// No single stream may inflate beyond this while parsing (defends against decompression bombs).
const MAX_STREAM_BYTES: usize = 256 * 1024 * 1024;

/// Parses untrusted PDF bytes with resource limits.
fn parse(bytes: &[u8], password: Option<&str>) -> Result<lopdf::Document, PdfError> {
    let options = lopdf::LoadOptions {
        password: password.map(str::to_owned),
        max_decompressed_size: Some(MAX_STREAM_BYTES),
        ..lopdf::LoadOptions::default()
    };
    Ok(lopdf::Document::load_mem_with_options(bytes, options)?)
}

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
