//! Documents written from scratch in Ferrix (ADR-0008).
//!
//! A [`Document`] is page setup plus paragraphs of formatted text. It changes only through
//! [`Edit`]s, each of which returns its inverse for undo. [`Layout`] breaks it into lines and
//! pages using [`Fonts`]; [`export_pdf`] writes that layout as a PDF with the document embedded,
//! and [`open_pdf`] finds it again.

#![forbid(unsafe_code)]

mod edit;
mod export;
pub mod fonts;
mod layout;
mod model;

pub use edit::{Applied, Edit, EditError, Fragment, Settings};
pub use export::{ExportError, Opened, SOURCE_NAME, export_pdf, open_pdf};
pub use fonts::Fonts;
pub use layout::{Decoration, Layout, Line, PageLayout, PlacedGlyph};
pub use model::{
    Align, CharFormat, Color, Document, FORMAT_VERSION, FontChoice, Margins, ModelError, PageSetup,
    Paragraph, ParagraphFormat, ParagraphStyle, Position, Run, TextRange,
};
