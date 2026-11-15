//! Documents written from scratch in Ferrix (ADR-0008).
//!
//! A [`Document`] is page setup plus paragraphs of formatted text. It changes only through
//! [`Edit`]s, each of which returns its inverse for undo. [`Layout`] breaks it into lines and
//! pages using [`Fonts`]; [`export_pdf`] writes that layout as a PDF with the document embedded,
//! and [`open_pdf`] finds it again.

#![forbid(unsafe_code)]

mod edit;
mod editor;
mod export;
pub mod fonts;
mod layout;
mod model;
mod raster;

pub use edit::{Applied, Edit, EditError, Fragment, Settings};
pub use editor::{CaretFormat, Clicks, Editor, Motion, Rect, Toggle};
pub use export::{ExportError, ExportJob, Opened, SOURCE_NAME, export_pdf, open_pdf};
pub use fonts::{EmbedFonts, Fonts};
pub use layout::{Decoration, Layout, LayoutCache, Line, PageLayout, PlacedGlyph, PlacedImage};
pub use model::{
    Align, CharFormat, Color, Document, FORMAT_VERSION, FontChoice, Image, ImageData, ImageKind,
    List, ListKind, MAX_IMAGE_BYTES, Margins, ModelError, PageSetup, PageText, Paragraph,
    ParagraphFormat, ParagraphStyle, Position, Run, TextRange,
};
pub use raster::{Raster, Rasterizer};
