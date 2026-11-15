//! Turning other file formats into PDF (ADR-0007).
//!
//! Images are converted in-process. Office documents are converted by an installed office
//! suite run in the background: Microsoft Word for word-processing formats when available,
//! otherwise LibreOffice, which also handles spreadsheets and presentations.

mod images;
mod libreoffice;
mod process;
mod word;

use std::path::{Path, PathBuf};

pub use images::images_to_pdf;
pub use libreoffice::{LibreOffice, SOFFICE_ENV};
pub use word::Word;

/// What kind of file a path holds, judged by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Pdf,
    Image,
    WordProcessing,
    Spreadsheet,
    Presentation,
}

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"];
pub const WORD_EXTENSIONS: &[&str] = &["doc", "docx", "docm", "dot", "dotx", "odt", "rtf", "txt"];
pub const SPREADSHEET_EXTENSIONS: &[&str] = &["xls", "xlsx", "xlsm", "ods", "csv"];
pub const PRESENTATION_EXTENSIONS: &[&str] = &["ppt", "pptx", "odp"];

pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let is = |list: &[&str]| list.contains(&ext.as_str());
    if ext == "pdf" {
        Some(Kind::Pdf)
    } else if is(IMAGE_EXTENSIONS) {
        Some(Kind::Image)
    } else if is(WORD_EXTENSIONS) {
        Some(Kind::WordProcessing)
    } else if is(SPREADSHEET_EXTENSIONS) {
        Some(Kind::Spreadsheet)
    } else if is(PRESENTATION_EXTENSIONS) {
        Some(Kind::Presentation)
    } else {
        None
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConvertError {
    #[error("“.{0}” files can’t be converted")]
    Unsupported(String),
    #[error("converting this file needs LibreOffice{}", if *.0 { " or Microsoft Word" } else { "" })]
    NoConverter(bool),
    #[error("{tool} could not convert the file: {message}")]
    Failed { tool: &'static str, message: String },
    #[error("{0} did not finish in time")]
    TimedOut(&'static str),
    #[error("{0}")]
    Io(String),
    #[error("could not read the image: {0}")]
    Image(String),
}

impl From<std::io::Error> for ConvertError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// The conversion tools available on this machine.
#[derive(Debug, Clone, Default)]
pub struct Converter {
    libreoffice: Option<LibreOffice>,
    word: Option<Word>,
}

impl Converter {
    /// Looks for installed tools. Cheap, but may run a short process on Windows.
    pub fn detect() -> Self {
        let converter = Self {
            libreoffice: LibreOffice::detect(),
            word: Word::detect(),
        };
        tracing::debug!(?converter, "detected converters");
        converter
    }

    pub fn supports(&self, kind: Kind) -> bool {
        match kind {
            Kind::Pdf | Kind::Image => true,
            Kind::WordProcessing => self.libreoffice.is_some() || self.word.is_some(),
            Kind::Spreadsheet | Kind::Presentation => self.libreoffice.is_some(),
        }
    }

    /// Names of the available office tools, for the UI.
    pub fn tools(&self) -> Vec<&'static str> {
        let mut tools = Vec::new();
        if self.word.is_some() {
            tools.push("Microsoft Word");
        }
        if self.libreoffice.is_some() {
            tools.push("LibreOffice");
        }
        tools
    }

    /// Every extension that can be opened or converted right now (PDF included).
    pub fn extensions(&self) -> Vec<&'static str> {
        let mut all = vec!["pdf"];
        all.extend(IMAGE_EXTENSIONS);
        for (kind, list) in [
            (Kind::WordProcessing, WORD_EXTENSIONS),
            (Kind::Spreadsheet, SPREADSHEET_EXTENSIONS),
            (Kind::Presentation, PRESENTATION_EXTENSIONS),
        ] {
            if self.supports(kind) {
                all.extend(list);
            }
        }
        all
    }

    /// Converts a non-PDF file to PDF bytes. Blocking; can take seconds.
    pub fn convert(&self, path: &Path) -> Result<Vec<u8>, ConvertError> {
        let kind = kind_of(path).ok_or_else(|| {
            ConvertError::Unsupported(
                path.extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        })?;
        match kind {
            Kind::Pdf => Ok(std::fs::read(path)?),
            Kind::Image => images_to_pdf(&[path.to_path_buf()]),
            Kind::WordProcessing => {
                if let Some(word) = &self.word {
                    match word.convert(path) {
                        Ok(bytes) => return Ok(bytes),
                        Err(e) if self.libreoffice.is_some() => {
                            tracing::warn!("Word failed, falling back to LibreOffice: {e}");
                        }
                        Err(e) => return Err(e),
                    }
                }
                self.libreoffice()?.convert(path)
            }
            Kind::Spreadsheet | Kind::Presentation => self.libreoffice()?.convert(path),
        }
    }

    fn libreoffice(&self) -> Result<&LibreOffice, ConvertError> {
        self.libreoffice
            .as_ref()
            .ok_or(ConvertError::NoConverter(cfg!(any(
                target_os = "macos",
                windows
            ))))
    }
}

/// A unique scratch directory for one conversion, removed on drop.
pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    pub(crate) fn new() -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ferrix-convert-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(
            kind_of(Path::new("a/Report.DOCX")),
            Some(Kind::WordProcessing)
        );
        assert_eq!(kind_of(Path::new("scan.jpeg")), Some(Kind::Image));
        assert_eq!(kind_of(Path::new("deck.pptx")), Some(Kind::Presentation));
        assert_eq!(kind_of(Path::new("doc.pdf")), Some(Kind::Pdf));
        assert_eq!(kind_of(Path::new("archive.zip")), None);
        assert_eq!(kind_of(Path::new("no-extension")), None);
    }

    #[test]
    fn without_office_tools_only_pdf_and_images_are_offered() {
        let none = Converter::default();
        assert!(none.supports(Kind::Image));
        assert!(!none.supports(Kind::WordProcessing));
        assert!(!none.extensions().contains(&"docx"));
        assert!(matches!(
            none.convert(Path::new("x.xlsx")),
            Err(ConvertError::NoConverter(_))
        ));
    }
}
