//! Incremental revisions: rewriting a document's page list (reorder, delete, rotate and import
//! pages) and replacing page annotations.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ferrix_types::Rotation;
use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId};

use crate::{PdfError, Source, parse};

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
    let mut revision = Revision::new(base, imports)?;
    revision.set_layout(layout.to_vec())?;
    revision.finish()
}

/// A document imported into a revision, renumbered above the base document's objects.
struct Imported {
    doc: Document,
    pages: Vec<ObjectId>,
    /// Original id → renumbered id, for the annotation objects of its pages.
    annots: HashMap<ObjectId, ObjectId>,
}

/// One incremental update of `base`: a new page list and/or changed annotations (ADR-0003,
/// ADR-0009). Callers read source objects, add or replace objects, then [`Self::finish`].
///
/// Pages and objects are addressed by **source**: 0 is `base`, `n` is `imports[n - 1]`.
/// Object ids of imported documents are renumbered; [`Self::object_id`] maps original ids of
/// their annotations.
pub struct Revision {
    inc: IncrementalDocument,
    base_pages: Vec<ObjectId>,
    imports: Vec<Imported>,
    root_id: ObjectId,
    layout: Option<Vec<LayoutPage>>,
    /// Entries to set on pages (`/Annots`, `/Tabs`…), by source page.
    page_entries: HashMap<(usize, u32), Vec<(&'static str, Object)>>,
    /// Replacements for existing objects, applied after imported pages are copied.
    replaced: BTreeMap<ObjectId, Object>,
    /// The base document's password, to read the output back for verification.
    password: Option<String>,
}

impl std::fmt::Debug for Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Revision")
            .field("base_pages", &self.base_pages.len())
            .field("imports", &self.imports.len())
            .field("layout", &self.layout.as_ref().map(Vec::len))
            .field("pages_changed", &self.page_entries.len())
            .field("replaced", &self.replaced.len())
            .finish_non_exhaustive()
    }
}

impl Revision {
    pub fn new(base: Source<'_>, imports: &[Source<'_>]) -> Result<Self, PdfError> {
        let base_doc = load(base)?;
        let base_pages: Vec<ObjectId> = base_doc.get_pages().into_values().collect();
        let root_id = pages_root(&base_doc)?;
        let mut inc = IncrementalDocument::create_from(base.bytes.to_vec(), base_doc);

        let mut imported = Vec::with_capacity(imports.len());
        for source in imports {
            let mut doc = load(*source)?;
            // Renumbering only moves objects that exist. A reference to a missing object would
            // keep its old number, which after renumbering belongs to an unrelated object (in the
            // import or the base file). A missing object reads as null (ISO 32000-2 §7.3.10), so
            // say so explicitly first.
            let present: HashSet<ObjectId> = doc.objects.keys().copied().collect();
            doc.traverse_objects(|object| {
                if let Object::Reference(id) = object
                    && !present.contains(id)
                {
                    *object = Object::Null;
                }
            });
            let before = annotation_refs(&doc);
            // Give imported objects ids above everything in the base document and earlier imports.
            doc.renumber_objects_with(inc.new_document.max_id + 1);
            inc.new_document.max_id = doc.max_id;
            // Renumbering keeps page order and each page's /Annots order, so the two lists pair up.
            let annots = before
                .into_iter()
                .zip(annotation_refs(&doc))
                .flat_map(|(old, new)| old.into_iter().zip(new))
                .collect();
            let pages = doc.get_pages().into_values().collect();
            imported.push(Imported { doc, pages, annots });
        }
        Ok(Self {
            inc,
            base_pages,
            imports: imported,
            root_id,
            layout: None,
            page_entries: HashMap::new(),
            replaced: BTreeMap::new(),
            password: base.password.map(str::to_owned),
        })
    }

    /// The parsed (and decrypted) source document, with renumbered ids for imports.
    pub fn source(&self, source: usize) -> Result<&Document, PdfError> {
        match source {
            0 => Ok(self.inc.get_prev_documents()),
            n => self
                .imports
                .get(n - 1)
                .map(|i| &i.doc)
                .ok_or_else(|| PdfError::InvalidLayout(format!("no source {n}"))),
        }
    }

    /// Number of pages in `source`.
    pub fn page_count(&self, source: usize) -> Result<usize, PdfError> {
        Ok(self.pages_of(source)?.len())
    }

    /// The object of page `page` (zero-based) of `source`, as numbered in the output.
    pub fn page_id(&self, source: usize, page: u32) -> Result<ObjectId, PdfError> {
        self.pages_of(source)?
            .get(page as usize)
            .copied()
            .ok_or_else(|| {
                PdfError::InvalidLayout(format!("source {source} has no page {}", page + 1))
            })
    }

    /// The output id of an annotation object, given its id in the original `source` file.
    pub fn object_id(&self, source: usize, original: ObjectId) -> Option<ObjectId> {
        match source {
            0 => Some(original),
            n => self.imports.get(n - 1)?.annots.get(&original).copied(),
        }
    }

    /// Adds a new object, returning its id.
    pub fn add_object(&mut self, object: Object) -> ObjectId {
        self.inc.new_document.add_object(object)
    }

    /// Replaces an existing object of a source (by output id) in this revision.
    pub fn replace_object(&mut self, id: ObjectId, object: Object) {
        self.replaced.insert(id, object);
    }

    /// Copies page `page` (zero-based) of the PDF `bytes` into this revision as a form XObject
    /// with `bbox` (in the page's own space), returning its id. Everything the page draws with
    /// (fonts, images, graphics states) comes along, renumbered after this revision's objects.
    /// Used for appearance streams laid out by another PDF writer (ADR-0009 §5).
    pub fn import_page_as_form(
        &mut self,
        bytes: &[u8],
        page: u32,
        bbox: [f32; 4],
    ) -> Result<ObjectId, PdfError> {
        let mut doc = parse(bytes, None)?;
        doc.renumber_objects_with(self.inc.new_document.max_id + 1);
        let page_id = *doc
            .get_pages()
            .get(&(page + 1))
            .ok_or_else(|| PdfError::InvalidLayout(format!("no page {}", page + 1)))?;
        let content = doc
            .get_page_content_with_limit(page_id, crate::MAX_STREAM_BYTES)
            .map_err(|e| PdfError::Parse(e.to_string()))?;
        let page_dict = flattened_page(&doc, page_id)?;
        let resources = match page_dict.get(b"Resources") {
            Ok(Object::Reference(id)) => doc.get_dictionary(*id)?.clone(),
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        };
        let mut ids = BTreeSet::new();
        collect_references(&doc, &Object::Dictionary(resources.clone()), &mut ids)?;
        for id in ids {
            if let Ok(object) = doc.get_object(id) {
                self.inc.new_document.set_object(id, object.clone());
            }
        }
        self.inc.new_document.max_id = self.inc.new_document.max_id.max(doc.max_id);

        let mut form = Dictionary::new();
        form.set("Type", Object::Name(b"XObject".to_vec()));
        form.set("Subtype", Object::Name(b"Form".to_vec()));
        form.set(
            "BBox",
            Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()),
        );
        form.set("Resources", Object::Dictionary(resources));
        let mut stream = lopdf::Stream::new(form, content);
        // Compression only fails on filters we didn't ask for; uncompressed is still valid.
        let _ = stream.compress();
        Ok(self.add_object(Object::Stream(stream)))
    }

    /// Sets the final `/Annots` array of a page.
    pub fn set_page_annots(
        &mut self,
        source: usize,
        page: u32,
        annots: Vec<Object>,
    ) -> Result<(), PdfError> {
        self.set_page_entry(source, page, "Annots", Object::Array(annots))
    }

    /// Sets one entry of a page's dictionary in this revision (e.g. `/Tabs`).
    pub fn set_page_entry(
        &mut self,
        source: usize,
        page: u32,
        key: &'static str,
        value: Object,
    ) -> Result<(), PdfError> {
        self.page_id(source, page)?;
        let entries = self.page_entries.entry((source, page)).or_default();
        entries.retain(|(k, _)| *k != key);
        entries.push((key, value));
        Ok(())
    }

    /// Reserves an id for an object written later with [`Self::replace_object`], so objects
    /// can refer to each other before both exist (a reply's `/IRT` to a new parent).
    pub fn reserve_id(&mut self) -> ObjectId {
        self.inc.new_document.new_object_id()
    }

    /// Replaces the page list. Without a layout, the base document keeps its pages.
    pub fn set_layout(&mut self, layout: Vec<LayoutPage>) -> Result<(), PdfError> {
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
        for entry in &layout {
            self.page_id(entry.source, entry.page)?;
        }
        self.layout = Some(layout);
        Ok(())
    }

    /// Writes the update and checks that the result parses with the expected page count.
    pub fn finish(mut self) -> Result<Vec<u8>, PdfError> {
        let expected = match self.layout.take() {
            Some(layout) => {
                self.write_layout(&layout)?;
                layout.len()
            }
            None => {
                if let Some((source, _)) = self.page_entries.keys().find(|(s, _)| *s != 0) {
                    return Err(PdfError::InvalidLayout(format!(
                        "pages of source {source} are not in the document"
                    )));
                }
                let changed: Vec<(u32, Vec<(&'static str, Object)>)> = self
                    .page_entries
                    .drain()
                    .map(|((_, page), entries)| (page, entries))
                    .collect();
                for (page, entries) in changed {
                    let id = self.page_id(0, page)?;
                    let mut dict = self.inc.get_prev_documents().get_dictionary(id)?.clone();
                    for (key, value) in entries {
                        dict.set(key, value);
                    }
                    self.inc
                        .new_document
                        .set_object(id, Object::Dictionary(dict));
                }
                self.base_pages.len()
            }
        };
        for (id, object) in std::mem::take(&mut self.replaced) {
            self.inc.new_document.set_object(id, object);
        }

        let base_len = self.inc.get_prev_documents_bytes().len();
        let mut out = Vec::with_capacity(base_len + 4096);
        self.inc
            .save_to(&mut out)
            .map_err(|e| PdfError::Write(e.to_string()))?;

        // Never hand back a file we can't read ourselves. An encrypted base stays encrypted, so
        // the check needs its password.
        let check = parse(&out, self.password.as_deref())
            .map_err(|e| PdfError::Write(format!("output is unreadable: {e}")))?;
        let written = check.get_pages().len();
        if written != expected {
            return Err(PdfError::Write(format!(
                "output has {written} pages, expected {expected}"
            )));
        }
        Ok(out)
    }

    fn pages_of(&self, source: usize) -> Result<&[ObjectId], PdfError> {
        match source {
            0 => Ok(&self.base_pages),
            n => self
                .imports
                .get(n - 1)
                .map(|i| i.pages.as_slice())
                .ok_or_else(|| PdfError::InvalidLayout(format!("no source {n}"))),
        }
    }

    fn write_layout(&mut self, layout: &[LayoutPage]) -> Result<(), PdfError> {
        let root_id = self.root_id;
        let mut kids = Vec::with_capacity(layout.len());
        for entry in layout {
            let page_id = self.page_id(entry.source, entry.page)?;
            let (page, dependencies) = {
                let doc = self.source(entry.source)?;
                let mut page = flattened_page(doc, page_id)?;
                let rotate = page.get(b"Rotate").ok().and_then(as_degrees).unwrap_or(0);
                page.set(
                    "Rotate",
                    Object::Integer(i64::from(rotate + entry.rotate_by.degrees()) % 360),
                );
                page.set("Parent", Object::Reference(root_id));
                if entry.source != 0 {
                    // `/StructParents` indexes the import's structure tree, which isn't copied;
                    // left in place it would point into the base file's tree instead.
                    page.remove(b"StructParents");
                }
                for (key, value) in self
                    .page_entries
                    .get(&(entry.source, entry.page))
                    .into_iter()
                    .flatten()
                {
                    page.set(*key, value.clone());
                }

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
                (page, dependencies)
            };
            for (id, mut object) in dependencies {
                // Objects added to this revision (new annotations) are already in place.
                if !self.inc.new_document.objects.contains_key(&id) {
                    // Same reason as `/StructParents` above, for annotations and forms.
                    match &mut object {
                        Object::Dictionary(d) => {
                            d.remove(b"StructParent");
                            d.remove(b"StructParents");
                        }
                        Object::Stream(st) => {
                            st.dict.remove(b"StructParent");
                            st.dict.remove(b"StructParents");
                        }
                        _ => {}
                    }
                    self.inc.new_document.set_object(id, object);
                }
            }
            self.inc
                .new_document
                .set_object(page_id, Object::Dictionary(page));
            kids.push(Object::Reference(page_id));
        }

        let mut root = self
            .inc
            .get_prev_documents()
            .get_dictionary(root_id)?
            .clone();
        for key in INHERITABLE {
            // Every page now carries its own copy; stop the root leaking values onto imports.
            root.remove(key);
        }
        root.set("Count", Object::Integer(kids.len() as i64));
        root.set("Kids", Object::Array(kids));
        self.inc
            .new_document
            .set_object(root_id, Object::Dictionary(root));
        Ok(())
    }
}

/// For each page in order, the references in its `/Annots` array.
fn annotation_refs(doc: &Document) -> Vec<Vec<ObjectId>> {
    doc.get_pages()
        .into_values()
        .map(|page| {
            doc.get_dictionary(page)
                .and_then(|d| d.get_deref(b"Annots", doc))
                .and_then(Object::as_array)
                .map(|items| items.iter().filter_map(|o| o.as_reference().ok()).collect())
                .unwrap_or_default()
        })
        .collect()
}

/// Parses and, if needed, decrypts a source document.
pub fn load(source: Source<'_>) -> Result<Document, PdfError> {
    let mut doc = parse(source.bytes, source.password)?;
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
