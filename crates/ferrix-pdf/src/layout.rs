//! Rewriting a document's page list: reorder, delete, rotate and import pages.

use std::collections::{BTreeSet, HashSet};

use ferrix_types::Rotation;
use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId};

use crate::{PdfError, Source};

/// Page attributes a page may inherit from its ancestors in the page tree (ISO 32000 §7.7.3.4).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
/// Guards against cyclic or absurdly deep page trees in malformed files.
const MAX_TREE_DEPTH: usize = 256;

/// One page of the output, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutPage {
    /// 0 is the base document; `n` is `imports[n - 1]`.
    pub source: usize,
    /// Zero-based page index within that source.
    pub page: u32,
    /// Rotation added to the page's current `/Rotate`.
    pub rotate_by: Rotation,
}

/// Produces `base` with its page list replaced by `layout`, as an incremental update.
///
/// Pages of `base` that are left out are unlinked from the page tree but, as with any
/// incremental update, their objects remain in the file until it is rewritten.
pub fn write_page_layout(
    base: Source<'_>,
    imports: &[Source<'_>],
    layout: &[LayoutPage],
) -> Result<Vec<u8>, PdfError> {
    if layout.is_empty() {
        return Err(PdfError::InvalidLayout(
            "a document needs at least one page".into(),
        ));
    }
    let mut seen = HashSet::new();
    if let Some(dup) = layout.iter().find(|p| !seen.insert((p.source, p.page))) {
        return Err(PdfError::InvalidLayout(format!(
            "page {} of source {} appears twice",
            dup.page + 1,
            dup.source
        )));
    }

    let base_doc = load(base)?;
    let base_pages: Vec<ObjectId> = base_doc.get_pages().into_values().collect();
    let root_id = pages_root(&base_doc)?;
    let mut inc = IncrementalDocument::create_from(base.bytes.to_vec(), base_doc);

    let mut imported_docs = Vec::with_capacity(imports.len());
    for source in imports {
        let mut doc = load(*source)?;
        // Give imported objects ids above everything in the base document and earlier imports.
        doc.renumber_objects_with(inc.new_document.max_id + 1);
        inc.new_document.max_id = doc.max_id;
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        imported_docs.push((doc, pages));
    }

    let mut kids = Vec::with_capacity(layout.len());
    for entry in layout {
        let (page_id, page, dependencies) = {
            let (doc, pages) = match entry.source {
                0 => (inc.get_prev_documents(), &base_pages),
                n => imported_docs
                    .get(n - 1)
                    .map(|(doc, pages)| (doc, pages))
                    .ok_or_else(|| PdfError::InvalidLayout(format!("no source {n}")))?,
            };
            let page_id = *pages.get(entry.page as usize).ok_or_else(|| {
                PdfError::InvalidLayout(format!(
                    "source {} has no page {}",
                    entry.source,
                    entry.page + 1
                ))
            })?;

            let mut page = flattened_page(doc, page_id)?;
            let rotate = page.get(b"Rotate").ok().and_then(as_degrees).unwrap_or(0);
            page.set(
                "Rotate",
                Object::Integer(i64::from(rotate + entry.rotate_by.degrees()) % 360),
            );
            page.set("Parent", Object::Reference(root_id));

            // Imported pages bring along everything they draw with (fonts, images, annotations…).
            let mut dependencies = Vec::new();
            if entry.source != 0 {
                let mut ids = BTreeSet::new();
                collect_references(doc, &Object::Dictionary(page.clone()), &mut ids)?;
                dependencies.extend(
                    ids.into_iter()
                        .filter_map(|id| doc.get_object(id).ok().map(|o| (id, o.clone()))),
                );
            }
            (page_id, page, dependencies)
        };
        for (id, object) in dependencies {
            inc.new_document.set_object(id, object);
        }
        inc.new_document
            .set_object(page_id, Object::Dictionary(page));
        kids.push(Object::Reference(page_id));
    }

    let mut root = inc.get_prev_documents().get_dictionary(root_id)?.clone();
    for key in INHERITABLE {
        // Every page now carries its own copy; stop the root leaking values onto imports.
        root.remove(key);
    }
    root.set("Count", Object::Integer(kids.len() as i64));
    root.set("Kids", Object::Array(kids));
    inc.new_document
        .set_object(root_id, Object::Dictionary(root));

    let mut out = Vec::with_capacity(base.bytes.len() + 4096);
    inc.save_to(&mut out)
        .map_err(|e| PdfError::Write(e.to_string()))?;

    // Never hand back a file we can't read ourselves.
    let check = Document::load_mem(&out)
        .map_err(|e| PdfError::Write(format!("output is unreadable: {e}")))?;
    let written = check.get_pages().len();
    if written != layout.len() {
        return Err(PdfError::Write(format!(
            "output has {written} pages, expected {}",
            layout.len()
        )));
    }
    Ok(out)
}

fn load(source: Source<'_>) -> Result<Document, PdfError> {
    let mut doc = Document::load_mem(source.bytes)?;
    if doc.is_encrypted() {
        doc.decrypt(source.password.unwrap_or(""))
            .map_err(|_| PdfError::Encrypted)?;
    }
    Ok(doc)
}

fn pages_root(doc: &Document) -> Result<ObjectId, PdfError> {
    match doc.catalog()?.get(b"Pages")? {
        Object::Reference(id) => Ok(*id),
        _ => Err(PdfError::Parse("/Pages is not an indirect object".into())),
    }
}

/// The page dictionary with inherited attributes copied onto it, so it can move to a new parent.
fn flattened_page(doc: &Document, page_id: ObjectId) -> Result<Dictionary, PdfError> {
    let mut page = doc.get_dictionary(page_id)?.clone();
    let mut parent = page.get(b"Parent").ok().cloned();
    for _ in 0..MAX_TREE_DEPTH {
        let Some(Object::Reference(id)) = parent else {
            break;
        };
        let Ok(node) = doc.get_dictionary(id) else {
            break;
        };
        for key in INHERITABLE {
            if !page.has(key)
                && let Ok(value) = node.get(key)
            {
                page.set(key, value.clone());
            }
        }
        parent = node.get(b"Parent").ok().cloned();
    }
    Ok(page)
}

/// `/Rotate` value in degrees, snapped to a multiple of 90 and normalized to 0..360.
fn as_degrees(object: &Object) -> Option<i32> {
    let value = match object {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => f64::from(*r),
        _ => return None,
    };
    let quarter_turns = (value / 90.0).round() as i64;
    Some((quarter_turns.rem_euclid(4) * 90) as i32)
}

/// Collects every object reachable from `object`, without following the page tree upwards.
fn collect_references(
    doc: &Document,
    object: &Object,
    found: &mut BTreeSet<ObjectId>,
) -> Result<(), PdfError> {
    let mut stack = vec![object.clone()];
    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(id) => {
                if found.insert(id)
                    && let Ok(target) = doc.get_object(id)
                {
                    stack.push(target.clone());
                }
            }
            Object::Array(items) => stack.extend(items),
            Object::Dictionary(dict) => push_dict(&dict, &mut stack),
            Object::Stream(stream) => push_dict(&stream.dict, &mut stack),
            _ => {}
        }
    }
    Ok(())
}

fn push_dict(dict: &Dictionary, stack: &mut Vec<Object>) {
    let is_tree_node = matches!(
        dict.get(b"Type"),
        Ok(Object::Name(name)) if name == b"Page" || name == b"Pages"
    );
    for (key, value) in dict.iter() {
        // A page's /Parent leads to the source document's whole page tree.
        if is_tree_node && key == b"Parent" {
            continue;
        }
        stack.push(value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_values_snap_and_normalize() {
        assert_eq!(as_degrees(&Object::Integer(-90)), Some(270));
        assert_eq!(as_degrees(&Object::Integer(450)), Some(90));
        assert_eq!(as_degrees(&Object::Real(89.0)), Some(90));
        assert_eq!(as_degrees(&Object::Name(b"x".to_vec())), None);
    }
}
