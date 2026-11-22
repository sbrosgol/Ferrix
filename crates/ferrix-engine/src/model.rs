use std::{fmt, path::PathBuf};

use ferrix_types::{DocId, PageIndex, PageTransform, PdfBox, PixelRect, Rect, Rotation, Size};
use serde::{Deserialize, Serialize};

/// Where a document's bytes come from.
#[derive(Clone, Serialize, Deserialize)]
pub enum DocSource {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

impl fmt::Debug for DocSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Self::Bytes(bytes) => write!(f, "Bytes({} bytes)", bytes.len()),
        }
    }
}

/// A password. `Debug` never prints the value.
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// What a backend can do, so the UI can hide unsupported features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EngineCaps {
    pub render: bool,
    pub text: bool,
    pub outline: bool,
    pub annotations: bool,
    pub forms: bool,
    pub save: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DocMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocInfo {
    pub id: DocId,
    pub page_count: u32,
    /// e.g. `"1.7"`; `None` if the header is unreadable.
    pub pdf_version: Option<String>,
    pub metadata: DocMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub index: PageIndex,
    /// Displayed size in points (crop box, intrinsic rotation applied).
    pub size: Size,
    /// The page's intrinsic `/Rotate`.
    pub rotation: Rotation,
    /// Page label from the document (e.g. `"iv"`), if defined.
    pub label: Option<String>,
    /// User space → page space.
    pub transform: PageTransform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFlags {
    pub annotations: bool,
    pub form_fields: bool,
}

impl Default for RenderFlags {
    fn default() -> Self {
        Self {
            annotations: true,
            form_fields: true,
        }
    }
}

/// A request for one region of a page rendered at `scale` device pixels per point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileRequest {
    pub doc: DocId,
    pub page: PageIndex,
    pub scale: f32,
    /// View rotation applied on top of the page's intrinsic rotation.
    pub rotation: Rotation,
    /// Region in device pixels of the whole page rendered at `scale` and `rotation`.
    pub region: PixelRect,
    pub flags: RenderFlags,
}

/// Rendered pixels: tightly packed, row-major, 8-bit RGBA, opaque.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderedTile {
    /// The region actually rendered, after clamping to the page.
    pub region: PixelRect,
    pub pixels: Vec<u8>,
}

impl fmt::Debug for RenderedTile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderedTile")
            .field("region", &self.region)
            .field("pixels", &format_args!("{} bytes", self.pixels.len()))
            .finish()
    }
}

/// One character on a page, in content-stream order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextChar {
    pub ch: char,
    /// Bounding box in page space. Empty for characters the engine synthesized.
    pub bounds: Rect,
    pub font_size: f32,
    /// True for spaces/line breaks inferred by the engine rather than present in the file.
    pub generated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextPage {
    pub page: PageIndex,
    pub chars: Vec<TextChar>,
}

impl TextPage {
    /// The page text in engine order, generated line breaks included.
    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.ch).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    /// Target page, if the entry points into this document.
    pub page: Option<PageIndex>,
    /// Where on the target page, if the entry says.
    pub top: Option<f32>,
    pub children: Vec<OutlineItem>,
}

/// A place in this document: a page, and how far down it, in page-space points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Destination {
    pub page: PageIndex,
    /// Distance from the top of the displayed page; `None` for the whole page.
    pub top: Option<f32>,
}

/// What following a link does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LinkTarget {
    /// Somewhere in this document.
    Page(Destination),
    /// A URI action, e.g. a web page or `mailto:`. Untrusted: the UI asks before opening it.
    Uri(String),
}

/// A link area on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageLink {
    /// The clickable area in page space.
    pub rect: Rect,
    pub target: LinkTarget,
}

/// An annotation as the engine sees it; used to verify what Ferrix wrote (ADR-0009 §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotSummary {
    /// The PDF `/Subtype` name, e.g. `"Square"`.
    pub subtype: String,
    /// `/Rect` in PDF user space.
    pub rect: PdfBox,
}
