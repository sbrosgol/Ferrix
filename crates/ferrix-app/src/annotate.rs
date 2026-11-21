//! Annotations in the app (ADR-0009 phase 2): loading them with a document, and what the
//! document allows.

use std::sync::Arc;

use ferrix_annot::{Permission, annotation_permission, read_annotations};
use ferrix_core::{AnnotSet, Annotation, PageEntry, SourceId};
use ferrix_types::PageTransform;
use lopdf::Document as Pdf;

use crate::{document::Origin, worker::blocking};

/// What a document lets the user do with annotations, read when it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub(crate) permission: Permission,
    /// Tagged, but with a structure tree new annotations can't be added to.
    pub(crate) untaggable: bool,
}

/// One source's annotations, per page, and (for the document itself) its policy.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub(crate) pages: Vec<Vec<Annotation>>,
    pub(crate) policy: Policy,
    /// Each page's user space → page space mapping, to place annotations on screen.
    pub(crate) transforms: Vec<PageTransform>,
    /// The file's bytes, kept to build previews without reading it again.
    pub(crate) bytes: Arc<Vec<u8>>,
}

/// Reads the annotations of `origin` off the UI thread.
pub(crate) async fn load(origin: Origin, source: SourceId) -> Result<Loaded, String> {
    blocking(move || {
        let bytes = origin.read()?;
        let doc = ferrix_pdf::load(ferrix_pdf::Source::new(&bytes)).map_err(|e| e.to_string())?;
        Ok(Loaded {
            pages: read_annotations(&doc, source),
            policy: policy(&doc),
            transforms: ferrix_pdf::page_transforms(&doc),
            bytes: Arc::new(bytes),
        })
    })
    .await
}

fn policy(doc: &Pdf) -> Policy {
    Policy {
        permission: annotation_permission(doc),
        untaggable: ferrix_annot::structure_tree_blocked(doc),
    }
}

/// The document as it would be saved with `annots`, for PDFium to render (ADR-0009 §7):
/// the original bytes plus an annotation-only incremental update for its own pages. `None` when
/// nothing differs from the file.
pub(crate) async fn build_preview(
    bytes: Arc<Vec<u8>>,
    pages: Vec<PageEntry>,
    annots: AnnotSet,
) -> Result<Option<Vec<u8>>, String> {
    blocking(move || {
        let mut revision = ferrix_pdf::Revision::new(ferrix_pdf::Source::new(&bytes), &[])
            .map_err(|e| e.to_string())?;
        let now = ferrix_annot::pdf_date(std::time::SystemTime::now());
        let report = ferrix_annot::write_page_annotations(&mut revision, &pages, &annots, &now)
            .map_err(|e| e.to_string())?;
        if report.is_empty() {
            return Ok(None);
        }
        revision.finish().map(Some).map_err(|e| e.to_string())
    })
    .await
}
