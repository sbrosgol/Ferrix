//! [`PdfEngine`] backend built on PDFium via `pdfium-render` (ADR-0001).
//!
//! PDFium is bound once per process. Calls are serialized by `pdfium-render`, so parallelism
//! comes from running several engine processes, not threads (ADR-0004).

mod library;

use std::{collections::HashMap, path::Path};

use ferrix_engine::{
    AnnotSummary, Destination, DocInfo, DocMetadata, DocSource, EngineCaps, EngineError,
    LinkTarget, OutlineItem, PageInfo, PageLink, PdfEngine, RenderedTile, Secret, TextChar,
    TextPage, TileRequest,
};
use ferrix_types::{DocId, PageIndex, PageTransform, PdfBox, Rect, Rotation};
pub use library::{LibraryError, PDFIUM_DIR_ENV};
use pdfium_render::prelude::*;

/// Largest tile edge we will allocate, in pixels (a 8192² RGBA tile is 256 MiB).
const MAX_TILE_EDGE: u32 = 8192;
/// Largest rendered page edge; PDFium uses `int` coordinates internally.
const MAX_PAGE_EDGE: u32 = 1 << 20;
/// Guards against malicious or cyclic outlines.
const MAX_OUTLINE_DEPTH: usize = 64;
const MAX_OUTLINE_ITEMS: usize = 100_000;
/// Guards against pages stuffed with link annotations.
const MAX_PAGE_LINKS: usize = 10_000;
/// URIs longer than this are not offered (they would only be shown, truncated, to be refused).
const MAX_URI_LEN: usize = 8192;

#[derive(Debug)]
pub struct PdfiumEngine {
    pdfium: &'static Pdfium,
    docs: HashMap<DocId, PdfDocument<'static>>,
    next_id: u64,
}

impl PdfiumEngine {
    /// Loads PDFium from the default search path (see [`PDFIUM_DIR_ENV`]).
    pub fn new() -> Result<Self, LibraryError> {
        library::bind(None).map(Self::with_pdfium)
    }

    /// Loads PDFium from a specific directory.
    pub fn with_library_dir(dir: &Path) -> Result<Self, LibraryError> {
        library::bind(Some(dir)).map(Self::with_pdfium)
    }

    fn with_pdfium(pdfium: &'static Pdfium) -> Self {
        Self {
            pdfium,
            docs: HashMap::new(),
            next_id: 1,
        }
    }

    fn doc(&self, id: DocId) -> Result<&PdfDocument<'static>, EngineError> {
        self.docs.get(&id).ok_or(EngineError::UnknownDocument(id))
    }

    fn page(&self, id: DocId, index: PageIndex) -> Result<PdfPage<'static>, EngineError> {
        let pages = self.doc(id)?.pages();
        let count = page_count(pages);
        let out_of_range = EngineError::PageOutOfRange { page: index, count };
        let Ok(raw) = i32::try_from(index.0) else {
            return Err(out_of_range);
        };
        if index.0 >= count {
            return Err(out_of_range);
        }
        pages.get(raw).map_err(backend)
    }
}

impl PdfEngine for PdfiumEngine {
    fn name(&self) -> &'static str {
        "pdfium"
    }

    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            render: true,
            text: true,
            outline: true,
            annotations: true,
            ..EngineCaps::default()
        }
    }

    fn open(
        &mut self,
        source: DocSource,
        password: Option<&Secret>,
    ) -> Result<DocInfo, EngineError> {
        let password = password.map(Secret::expose);
        // Read files into memory rather than keeping them open: an open handle would block
        // (Windows) or go stale (Unix) when a save atomically replaces the file.
        let bytes = match source {
            DocSource::Path(path) => {
                std::fs::read(&path).map_err(|e| EngineError::Io(e.to_string()))?
            }
            DocSource::Bytes(bytes) => bytes,
        };
        let document = self
            .pdfium
            .load_pdf_from_byte_vec(bytes, password)
            .map_err(open_error)?;

        let id = DocId(self.next_id);
        self.next_id += 1;
        let info = DocInfo {
            id,
            page_count: page_count(document.pages()),
            pdf_version: version_string(document.version()),
            metadata: metadata(document.metadata()),
        };
        self.docs.insert(id, document);
        tracing::debug!(%id, pages = info.page_count, "opened document");
        Ok(info)
    }

    fn close(&mut self, doc: DocId) {
        self.docs.remove(&doc);
    }

    fn page_info(&mut self, doc: DocId, index: PageIndex) -> Result<PageInfo, EngineError> {
        let page = self.page(doc, index)?;
        let (transform, rotation) = page_transform(&page)?;
        Ok(PageInfo {
            index,
            size: transform.page_size(),
            rotation,
            label: page.label().map(str::to_owned),
            transform,
        })
    }

    fn render_tile(&mut self, request: &TileRequest) -> Result<RenderedTile, EngineError> {
        if !(request.scale.is_finite() && request.scale > 0.0) {
            return Err(EngineError::InvalidRequest(format!(
                "scale {}",
                request.scale
            )));
        }
        let page = self.page(request.doc, request.page)?;
        let (transform, _) = page_transform(&page)?;

        let (mut page_w, mut page_h) = transform.page_size().to_pixels(request.scale);
        if request.rotation.is_sideways() {
            std::mem::swap(&mut page_w, &mut page_h);
        }
        if page_w > MAX_PAGE_EDGE || page_h > MAX_PAGE_EDGE {
            return Err(EngineError::InvalidRequest(format!(
                "page would be {page_w}×{page_h} px at scale {}",
                request.scale
            )));
        }
        let region = request.region.clamp_to(page_w, page_h).ok_or_else(|| {
            EngineError::InvalidRequest(format!("{:?} is outside the page", request.region))
        })?;
        if region.width > MAX_TILE_EDGE || region.height > MAX_TILE_EDGE {
            return Err(EngineError::InvalidRequest(format!(
                "tile {}×{} exceeds {MAX_TILE_EDGE} px",
                region.width, region.height
            )));
        }

        // Render the whole page geometry, offset so only `region` lands in the tile-sized bitmap.
        let config = PdfRenderConfig::new()
            .set_fixed_size(px(page_w), px(page_h))
            .set_origin(-px(region.x), -px(region.y))
            .rotate(render_rotation(request.rotation), false)
            .render_annotations(request.flags.annotations)
            .render_form_data(request.flags.form_fields)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let mut bitmap =
            PdfBitmap::empty(px(region.width), px(region.height), PdfBitmapFormat::BGRA)
                .map_err(backend)?;
        page.render_into_bitmap_with_config(&mut bitmap, &config)
            .map_err(backend)?;

        let pixels = bitmap.as_rgba_bytes();
        let expected = region.width as usize * region.height as usize * 4;
        if pixels.len() != expected {
            return Err(EngineError::Backend(format!(
                "bitmap has {} bytes, expected {expected}",
                pixels.len()
            )));
        }
        Ok(RenderedTile { region, pixels })
    }

    fn text_page(&mut self, doc: DocId, index: PageIndex) -> Result<TextPage, EngineError> {
        let page = self.page(doc, index)?;
        let (transform, _) = page_transform(&page)?;
        let text = page.text().map_err(backend)?;
        let chars = text.chars();

        let chars = chars
            .iter()
            .filter_map(|c| {
                let ch = c.unicode_char()?;
                let generated = c.is_generated().unwrap_or(false);
                let bounds = match c.loose_bounds() {
                    Ok(b) if !generated => transform.box_to_page(pdf_box(&b)),
                    _ => Rect::default(),
                };
                Some(TextChar {
                    ch,
                    bounds,
                    font_size: c.scaled_font_size().value,
                    generated,
                })
            })
            .collect();
        Ok(TextPage { page: index, chars })
    }

    fn annotations(
        &mut self,
        doc: DocId,
        index: PageIndex,
    ) -> Result<Vec<AnnotSummary>, EngineError> {
        let page = self.page(doc, index)?;
        Ok(page
            .annotations()
            .iter()
            .map(|annot| AnnotSummary {
                subtype: subtype_name(annot.annotation_type()).to_owned(),
                rect: annot.bounds().map(|r| pdf_box(&r)).unwrap_or_default(),
            })
            .collect())
    }

    fn outline(&mut self, doc: DocId) -> Result<Vec<OutlineItem>, EngineError> {
        let document = self.doc(doc)?;
        let Some(first) = document.bookmarks().root() else {
            return Ok(Vec::new());
        };
        let mut budget = MAX_OUTLINE_ITEMS;
        let mut tops = Tops::new(document);
        Ok(outline_level(Some(first), 0, &mut budget, &mut tops))
    }

    fn links(&mut self, doc: DocId, index: PageIndex) -> Result<Vec<PageLink>, EngineError> {
        let page = self.page(doc, index)?;
        let (transform, _) = page_transform(&page)?;
        let mut tops = Tops::new(self.doc(doc)?);
        let mut links = Vec::new();
        for link in page.links().iter().take(MAX_PAGE_LINKS) {
            let Ok(rect) = link.rect() else { continue };
            let rect = transform.box_to_page(pdf_box(&rect));
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue;
            }
            let target = match link.action() {
                Some(action) => match action.action_type() {
                    PdfActionType::GoToDestinationInSameDocument => action
                        .as_local_destination_action()
                        .and_then(|a| a.destination().ok())
                        .and_then(|d| tops.destination(&d))
                        .map(LinkTarget::Page),
                    PdfActionType::Uri => action
                        .as_uri_action()
                        .and_then(|a| a.uri().ok())
                        .map(|uri| uri.trim().to_owned())
                        .filter(|uri| !uri.is_empty() && uri.len() <= MAX_URI_LEN)
                        .map(LinkTarget::Uri),
                    // Launching files, other documents and scripts are not followed.
                    _ => None,
                },
                // A link with `/Dest` instead of an action.
                None => link
                    .destination()
                    .and_then(|d| tops.destination(&d))
                    .map(LinkTarget::Page),
            };
            if let Some(target) = target {
                links.push(PageLink { rect, target });
            }
        }
        Ok(links)
    }
}

/// Resolves destinations to page space, measuring each target page once.
struct Tops<'a> {
    doc: &'a PdfDocument<'static>,
    transforms: HashMap<u32, Option<(PageTransform, Rotation)>>,
}

impl<'a> Tops<'a> {
    fn new(doc: &'a PdfDocument<'static>) -> Self {
        Self {
            doc,
            transforms: HashMap::new(),
        }
    }

    fn destination(&mut self, dest: &PdfDestination<'_>) -> Option<Destination> {
        let page = u32::try_from(dest.page_index().ok()?).ok()?;
        if page >= page_count(self.doc.pages()) {
            return None;
        }
        Some(Destination {
            page: PageIndex(page),
            top: self.top(page, dest),
        })
    }

    /// The destination's top edge in page space, if it names one.
    fn top(&mut self, page: u32, dest: &PdfDestination<'_>) -> Option<f32> {
        use PdfDestinationViewSettings as V;
        let (x, y) = match dest.view_settings().ok()? {
            V::SpecificCoordinatesAndZoom(x, y, _) => (x, y?),
            V::FitPageHorizontallyToWindow(top) | V::FitBoundsHorizontallyToWindow(top) => {
                (None, top?)
            }
            V::FitPageToRectangle(rect) => (Some(rect.left()), rect.top()),
            _ => return None,
        };
        let doc = self.doc;
        let (transform, rotation) = (*self.transforms.entry(page).or_insert_with(|| {
            let raw = i32::try_from(page).ok()?;
            page_transform(&doc.pages().get(raw).ok()?).ok()
        }))?;
        let x = match x {
            Some(x) => x.value,
            // On a sideways page the PDF's x runs down the screen, so a top without an x
            // says nothing about where to scroll.
            None if rotation.is_sideways() => return None,
            None => 0.0,
        };
        let size = transform.page_size();
        Some(transform.to_page(x, y.value).y.clamp(0.0, size.height))
    }
}

/// Collects `first` and its following siblings, recursing into children.
fn outline_level(
    first: Option<PdfBookmark<'_>>,
    depth: usize,
    budget: &mut usize,
    tops: &mut Tops<'_>,
) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut next = first;
    while let Some(bookmark) = next {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let children = if depth + 1 < MAX_OUTLINE_DEPTH {
            outline_level(bookmark.first_child(), depth + 1, budget, tops)
        } else {
            Vec::new()
        };
        let dest = bookmark.destination().and_then(|d| tops.destination(&d));
        items.push(OutlineItem {
            title: bookmark.title().unwrap_or_default(),
            page: dest.map(|d| d.page),
            top: dest.and_then(|d| d.top),
            children,
        });
        next = bookmark.next_sibling();
    }
    items
}

fn page_transform(page: &PdfPage<'_>) -> Result<(PageTransform, Rotation), EngineError> {
    let boundaries = page.boundaries();
    // `bounding()` is the visible area (crop box clipped to the media box) with inheritance from
    // the page tree resolved. `crop()` and `media()` read only the page's own dictionary, so they
    // fail for the many files that put `/MediaBox` on the root; keep them as fallbacks only.
    let crop = boundaries
        .bounding()
        .or_else(|_| boundaries.crop())
        .or_else(|_| boundaries.media())
        .map_err(backend)?;
    let rotation = match page.rotation().map_err(backend)? {
        PdfPageRenderRotation::None => Rotation::R0,
        PdfPageRenderRotation::Degrees90 => Rotation::R90,
        PdfPageRenderRotation::Degrees180 => Rotation::R180,
        PdfPageRenderRotation::Degrees270 => Rotation::R270,
    };
    Ok((
        PageTransform::new(pdf_box(&crop.bounds), rotation),
        rotation,
    ))
}

/// The PDF `/Subtype` name of a PDFium annotation type.
fn subtype_name(kind: PdfPageAnnotationType) -> &'static str {
    use PdfPageAnnotationType as T;
    match kind {
        T::Unknown => "Unknown",
        T::Text => "Text",
        T::Link => "Link",
        T::FreeText => "FreeText",
        T::Line => "Line",
        T::Square => "Square",
        T::Circle => "Circle",
        T::Polygon => "Polygon",
        T::Polyline => "PolyLine",
        T::Highlight => "Highlight",
        T::Underline => "Underline",
        T::Squiggly => "Squiggly",
        T::Strikeout => "StrikeOut",
        T::Stamp => "Stamp",
        T::Caret => "Caret",
        T::Ink => "Ink",
        T::Popup => "Popup",
        T::FileAttachment => "FileAttachment",
        T::Sound => "Sound",
        T::Movie => "Movie",
        T::Widget | T::XfaWidget => "Widget",
        T::Screen => "Screen",
        T::PrinterMark => "PrinterMark",
        T::TrapNet => "TrapNet",
        T::Watermark => "Watermark",
        T::ThreeD => "3D",
        T::RichMedia => "RichMedia",
        T::Redacted => "Redact",
    }
}

fn render_rotation(rotation: Rotation) -> PdfPageRenderRotation {
    match rotation {
        Rotation::R0 => PdfPageRenderRotation::None,
        Rotation::R90 => PdfPageRenderRotation::Degrees90,
        Rotation::R180 => PdfPageRenderRotation::Degrees180,
        Rotation::R270 => PdfPageRenderRotation::Degrees270,
    }
}

fn pdf_box(r: &PdfRect) -> PdfBox {
    PdfBox {
        left: r.left().value,
        bottom: r.bottom().value,
        right: r.right().value,
        top: r.top().value,
    }
}

/// Pixel sizes are validated against `MAX_PAGE_EDGE` before conversion, so this cannot overflow.
fn px(value: u32) -> Pixels {
    Pixels::try_from(value).unwrap_or(Pixels::MAX)
}

fn version_string(version: PdfDocumentVersion) -> Option<String> {
    let s = match version {
        PdfDocumentVersion::Unset => return None,
        PdfDocumentVersion::Pdf1_0 => "1.0",
        PdfDocumentVersion::Pdf1_1 => "1.1",
        PdfDocumentVersion::Pdf1_2 => "1.2",
        PdfDocumentVersion::Pdf1_3 => "1.3",
        PdfDocumentVersion::Pdf1_4 => "1.4",
        PdfDocumentVersion::Pdf1_5 => "1.5",
        PdfDocumentVersion::Pdf1_6 => "1.6",
        PdfDocumentVersion::Pdf1_7 => "1.7",
        PdfDocumentVersion::Pdf2_0 => "2.0",
        PdfDocumentVersion::Other(v) => return Some(format!("{}.{}", v / 10, v % 10)),
    };
    Some(s.to_owned())
}

fn metadata(meta: &PdfMetadata<'_>) -> DocMetadata {
    let get = |tag| {
        meta.get(tag)
            .map(|t| t.value().trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    DocMetadata {
        title: get(PdfDocumentMetadataTagType::Title),
        author: get(PdfDocumentMetadataTagType::Author),
        subject: get(PdfDocumentMetadataTagType::Subject),
        keywords: get(PdfDocumentMetadataTagType::Keywords),
        creator: get(PdfDocumentMetadataTagType::Creator),
        producer: get(PdfDocumentMetadataTagType::Producer),
    }
}

fn open_error(error: PdfiumError) -> EngineError {
    match error {
        PdfiumError::IoError(e) => EngineError::Io(e.to_string()),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            EngineError::PasswordRequired
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            EngineError::Io("file could not be read".into())
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            EngineError::InvalidDocument("malformed file structure".into())
        }
        other => EngineError::Backend(other.to_string()),
    }
}

fn backend(error: PdfiumError) -> EngineError {
    EngineError::Backend(error.to_string())
}

fn page_count(pages: &PdfPages<'_>) -> u32 {
    u32::try_from(pages.len()).unwrap_or(0)
}
