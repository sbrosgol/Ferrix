//! The backend-neutral PDF engine interface (see ADR-0001).
//!
//! Everything here is plain, serializable data so the same API can later cross the process
//! boundary to a sandboxed engine host (ADR-0004). Engine-specific types never appear here.
//!
//! All geometry is in page space (see [`ferrix_types`]).

#![forbid(unsafe_code)]

mod error;
mod model;

pub use error::EngineError;
pub use model::{
    AnnotSummary, Destination, DocInfo, DocMetadata, DocSource, EngineCaps, LinkTarget,
    OutlineItem, PageInfo, PageLink, RenderFlags, RenderedTile, Secret, TextChar, TextPage,
    TileRequest,
};

use ferrix_types::{DocId, PageIndex};

/// A PDF engine backend. One instance owns its documents and is driven from one thread at a time.
///
/// Editing (`apply`) and saving arrive with roadmap M2; until then backends are read-only.
pub trait PdfEngine: Send {
    /// Short backend name for diagnostics, e.g. `"pdfium"`.
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> EngineCaps;

    fn open(
        &mut self,
        source: DocSource,
        password: Option<&Secret>,
    ) -> Result<DocInfo, EngineError>;

    /// Closes a document. Unknown ids are ignored.
    fn close(&mut self, doc: DocId);

    fn page_info(&mut self, doc: DocId, page: PageIndex) -> Result<PageInfo, EngineError>;

    /// Renders one rectangular region of a page. The region is clamped to the page bounds.
    fn render_tile(&mut self, request: &TileRequest) -> Result<RenderedTile, EngineError>;

    fn text_page(&mut self, doc: DocId, page: PageIndex) -> Result<TextPage, EngineError>;

    fn outline(&mut self, doc: DocId) -> Result<Vec<OutlineItem>, EngineError>;

    /// The links of a page that Ferrix can follow: to places in this document, and URIs.
    /// Other actions (launching files, scripts, remote documents) are left out.
    fn links(&mut self, _doc: DocId, _page: PageIndex) -> Result<Vec<PageLink>, EngineError> {
        Ok(Vec::new())
    }

    /// The annotations of a page in the engine's order, popups and form widgets included.
    fn annotations(
        &mut self,
        _doc: DocId,
        _page: PageIndex,
    ) -> Result<Vec<AnnotSummary>, EngineError> {
        Err(EngineError::Unsupported("annotations".into()))
    }
}
