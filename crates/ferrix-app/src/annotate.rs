//! Annotations in the app (ADR-0009 phase 2): loading them with a document, and what the
//! document allows.

use ferrix_annot::{Permission, annotation_permission, read_annotations};
use ferrix_core::{Annotation, SourceId};
use lopdf::{Document as Pdf, Object};

use crate::{document::Origin, worker::blocking};

/// What a document lets the user do with annotations, read when it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub(crate) permission: Permission,
    /// Tagged for accessibility: new annotations aren't added to its structure yet (phase 4).
    pub(crate) tagged: bool,
}

/// One source's annotations, per page, and (for the document itself) its policy.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub(crate) pages: Vec<Vec<Annotation>>,
    pub(crate) policy: Policy,
}

/// Reads the annotations of `origin` off the UI thread.
pub(crate) async fn load(origin: Origin, source: SourceId) -> Result<Loaded, String> {
    blocking(move || {
        let bytes = origin.read()?;
        let doc = ferrix_pdf::load(ferrix_pdf::Source::new(&bytes)).map_err(|e| e.to_string())?;
        Ok(Loaded {
            pages: read_annotations(&doc, source),
            policy: policy(&doc),
        })
    })
    .await
}

fn policy(doc: &Pdf) -> Policy {
    let tagged = doc.catalog().is_ok_and(|catalog| {
        catalog.has(b"StructTreeRoot")
            || catalog
                .get_deref(b"MarkInfo", doc)
                .and_then(Object::as_dict)
                .and_then(|m| m.get(b"Marked"))
                .and_then(Object::as_bool)
                .unwrap_or(false)
    });
    Policy {
        permission: annotation_permission(doc),
        tagged,
    }
}
