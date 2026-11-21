//! Annotations in the structure tree of tagged PDFs (ISO 32000-2 §14.7.4.4, PDF/UA-1 §7.18.1):
//! each new annotation gets an `/Annot` structure element whose `/K` is an object reference to
//! it, a `/StructParent` key in the parent tree pointing back, and its page `/Tabs /S`.
//! Deleting an annotation removes its element and key again.
//!
//! Only files that are already tagged are touched. Everything is read with bounds (depth and
//! node count), and a tree that doesn't look as the specification describes is left alone:
//! the annotation is then written untagged and the caller tells the user.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

/// Deepest number tree followed, and most nodes visited per lookup.
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 10_000;
/// Stands for a parent tree written directly inside the structure tree root.
const DIRECT: ObjectId = (0, u16::MAX);

/// Structure-tree changes collected while one revision is written.
#[derive(Debug)]
pub(crate) struct Tagger {
    root: ObjectId,
    /// Root of the parent tree.
    tree: ObjectId,
    next_key: i64,
    /// Dictionaries changed so far (struct tree root, elements, number tree nodes).
    changed: HashMap<ObjectId, Dictionary>,
    /// New `/Annot` elements.
    elements: Vec<(ObjectId, Dictionary)>,
    /// Parent tree entries to append, keys ascending.
    nums: Vec<(i64, ObjectId)>,
    /// Pages to give `/Tabs /S`, as (source, page index).
    pages: HashSet<u32>,
}

impl Tagger {
    /// `None` for untagged files; `Some(Err(()))` for tagged files whose structure tree Ferrix
    /// can't safely extend.
    pub(crate) fn load(doc: &Document) -> Option<Result<Self, ()>> {
        let catalog = doc.catalog().ok()?;
        let marked = catalog
            .get_deref(b"MarkInfo", doc)
            .and_then(Object::as_dict)
            .and_then(|m| m.get(b"Marked"))
            .and_then(Object::as_bool)
            .unwrap_or(false);
        let root = catalog.get(b"StructTreeRoot").ok()?;
        if !marked {
            return None;
        }
        Some(Self::load_tree(doc, root).ok_or(()))
    }

    fn load_tree(doc: &Document, root: &Object) -> Option<Self> {
        let root_id = root.as_reference().ok()?;
        let root_dict = doc.get_dictionary(root_id).ok()?.clone();
        let mut changed = HashMap::new();
        let tree = match root_dict.get(b"ParentTree") {
            Ok(Object::Reference(id)) => {
                doc.get_dictionary(*id).ok()?;
                *id
            }
            Ok(Object::Dictionary(d)) => {
                changed.insert(DIRECT, d.clone());
                DIRECT
            }
            Ok(_) => return None,
            Err(_) => {
                changed.insert(
                    DIRECT,
                    Dictionary::from_iter([("Nums", Object::Array(vec![]))]),
                );
                DIRECT
            }
        };
        // Keys in use anywhere count, even if /ParentTreeNextKey is missing or too low.
        let used = doc
            .objects
            .values()
            .filter_map(|o| match o {
                Object::Dictionary(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            })
            .flat_map(|d| [d.get(b"StructParents"), d.get(b"StructParent")])
            .filter_map(|k| k.ok()?.as_i64().ok())
            .max()
            .map_or(0, |k| k + 1);
        let declared = root_dict
            .get(b"ParentTreeNextKey")
            .and_then(Object::as_i64)
            .unwrap_or(0);
        changed.insert(root_id, root_dict);
        let tagger = Self {
            root: root_id,
            tree,
            next_key: used.max(declared),
            changed,
            elements: Vec::new(),
            nums: Vec::new(),
            pages: HashSet::new(),
        };
        // The rightmost leaf must be reachable to append to it.
        tagger.rightmost_path(doc)?;
        Some(tagger)
    }

    fn dict(&self, doc: &Document, id: ObjectId) -> Option<Dictionary> {
        self.changed
            .get(&id)
            .cloned()
            .or_else(|| doc.get_dictionary(id).ok().cloned())
    }

    /// Number tree nodes from the root to the leaf holding the largest keys.
    fn rightmost_path(&self, doc: &Document) -> Option<Vec<ObjectId>> {
        let mut path = vec![self.tree];
        loop {
            let node = self.dict(doc, *path.last()?)?;
            if node.has(b"Nums") {
                node.get(b"Nums").ok()?.as_array().ok()?;
                return Some(path);
            }
            let kids = node.get(b"Kids").ok()?.as_array().ok()?;
            let last = kids.last()?.as_reference().ok()?;
            if path.len() >= MAX_DEPTH || path.contains(&last) {
                return None;
            }
            path.push(last);
        }
    }

    /// The path to the leaf holding `key`, and its value.
    fn lookup(&self, doc: &Document, key: i64) -> Option<(Vec<ObjectId>, Object)> {
        let mut budget = MAX_NODES;
        self.lookup_in(doc, vec![self.tree], key, &mut budget)
    }

    fn lookup_in(
        &self,
        doc: &Document,
        path: Vec<ObjectId>,
        key: i64,
        budget: &mut usize,
    ) -> Option<(Vec<ObjectId>, Object)> {
        *budget = budget.checked_sub(1)?;
        let node = self.dict(doc, *path.last()?)?;
        if let Ok(nums) = node.get(b"Nums").and_then(Object::as_array) {
            return nums
                .as_chunks::<2>()
                .0
                .iter()
                .find(|pair| pair[0].as_i64().ok() == Some(key))
                .map(|pair| (path.clone(), pair[1].clone()));
        }
        for kid in node.get(b"Kids").ok()?.as_array().ok()? {
            let kid = kid.as_reference().ok()?;
            if path.len() >= MAX_DEPTH || path.contains(&kid) {
                return None;
            }
            let inside = self
                .dict(doc, kid)
                .and_then(|k| limits(&k))
                .is_none_or(|(lo, hi)| (lo..=hi).contains(&key));
            if inside {
                let mut next = path.clone();
                next.push(kid);
                if let Some(found) = self.lookup_in(doc, next, key, budget) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// The element new annotations on `page` go under: the one owning the page's last marked
    /// content, else the document's first element, else the root.
    fn parent_for(&self, doc: &Document, page: ObjectId) -> ObjectId {
        let from_page = doc
            .get_dictionary(page)
            .ok()
            .and_then(|p| p.get(b"StructParents").ok()?.as_i64().ok())
            .and_then(|key| self.lookup(doc, key))
            .and_then(|(_, value)| {
                let value = match value {
                    Object::Reference(id) => doc.get_object(id).ok()?.clone(),
                    other => other,
                };
                value
                    .as_array()
                    .ok()?
                    .iter()
                    .rev()
                    .find_map(|o| o.as_reference().ok())
            })
            .filter(|id| self.dict(doc, *id).is_some());
        from_page
            .or_else(|| {
                let root = self.dict(doc, self.root)?;
                match root.get(b"K").ok()? {
                    Object::Reference(id) => Some(*id),
                    Object::Array(kids) => kids.iter().find_map(|k| k.as_reference().ok()),
                    _ => None,
                }
                .filter(|id| self.dict(doc, *id).is_some_and(|d| d.has(b"S")))
            })
            .unwrap_or(self.root)
    }

    /// Tags the annotation `annot` (its dictionary is `dict`, not yet written) on page
    /// `page_id` (page index `page`), as element `element`.
    pub(crate) fn tag(
        &mut self,
        doc: &Document,
        (page_id, page): (ObjectId, u32),
        annot: ObjectId,
        element: ObjectId,
        dict: &mut Dictionary,
        alt: &str,
    ) {
        let key = self.next_key;
        self.next_key += 1;
        let parent = self.parent_for(doc, page_id);
        let mut objr = Dictionary::new();
        objr.set("Type", Object::Name(b"OBJR".to_vec()));
        objr.set("Obj", Object::Reference(annot));
        objr.set("Pg", Object::Reference(page_id));
        let mut elem = Dictionary::new();
        elem.set("Type", Object::Name(b"StructElem".to_vec()));
        elem.set("S", Object::Name(b"Annot".to_vec()));
        elem.set("P", Object::Reference(parent));
        elem.set("Pg", Object::Reference(page_id));
        elem.set("K", Object::Dictionary(objr));
        if !alt.is_empty() {
            elem.set("Alt", lopdf::text_string(alt));
        }
        self.elements.push((element, elem));
        if let Some(mut p) = self.dict(doc, parent) {
            let mut kids = match p.get(b"K") {
                Ok(Object::Array(a)) => a.clone(),
                Ok(other) => vec![other.clone()],
                Err(_) => Vec::new(),
            };
            kids.push(Object::Reference(element));
            p.set("K", Object::Array(kids));
            self.changed.insert(parent, p);
        }
        self.nums.push((key, element));
        self.pages.insert(page);
        dict.set("StructParent", Object::Integer(key));
    }

    /// Removes the element of a deleted annotation (its `/StructParent` is `key`) from its
    /// parent and the parent tree. Anything unexpected is left as it is.
    pub(crate) fn untag(&mut self, doc: &Document, key: i64) {
        let Some((path, Object::Reference(element))) = self.lookup(doc, key) else {
            return;
        };
        let Some(elem) = self.dict(doc, element) else {
            return;
        };
        if elem.get(b"S").and_then(Object::as_name).ok() != Some(b"Annot".as_slice()) {
            return;
        }
        if let Some(parent) = elem.get(b"P").ok().and_then(|p| p.as_reference().ok())
            && let Some(mut p) = self.dict(doc, parent)
        {
            match p.get(b"K") {
                Ok(Object::Array(kids)) => {
                    let kids: Vec<Object> = kids
                        .iter()
                        .filter(|k| k.as_reference().ok() != Some(element))
                        .cloned()
                        .collect();
                    p.set("K", Object::Array(kids));
                }
                Ok(Object::Reference(id)) if *id == element => {
                    p.remove(b"K");
                }
                _ => {}
            }
            self.changed.insert(parent, p);
        }
        if let Some(leaf_id) = path.last().copied()
            && let Some(mut leaf) = self.dict(doc, leaf_id)
            && let Ok(nums) = leaf.get(b"Nums").and_then(Object::as_array)
        {
            let nums: Vec<Object> = nums
                .as_chunks::<2>()
                .0
                .iter()
                .filter(|pair| pair[0].as_i64().ok() != Some(key))
                .flatten()
                .cloned()
                .collect();
            leaf.set("Nums", Object::Array(nums));
            self.changed.insert(leaf_id, leaf);
        }
        // Limits may now be wider than the keys: still correct for lookups (§7.9.7).
    }

    /// The objects to write and the pages to give `/Tabs /S`.
    pub(crate) fn finish(mut self, doc: &Document) -> (Vec<(ObjectId, Dictionary)>, Vec<u32>) {
        if !self.nums.is_empty() {
            let path = self.rightmost_path(doc).unwrap_or_default();
            let Some(&leaf_id) = path.last() else {
                return (Vec::new(), Vec::new());
            };
            let mut leaf = self.dict(doc, leaf_id).unwrap_or_default();
            let mut nums = leaf
                .get(b"Nums")
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default();
            for (key, element) in &self.nums {
                nums.push(Object::Integer(*key));
                nums.push(Object::Reference(*element));
            }
            leaf.set("Nums", Object::Array(nums));
            self.changed.insert(leaf_id, leaf);
            let high = self.nums.last().map_or(0, |(k, _)| *k);
            // Intermediate nodes and the leaf cover the new keys.
            for id in path.iter().skip(1) {
                if let Some(mut node) = self.dict(doc, *id)
                    && let Some((lo, hi)) = limits(&node)
                    && hi < high
                {
                    node.set(
                        "Limits",
                        Object::Array(vec![Object::Integer(lo), Object::Integer(high)]),
                    );
                    self.changed.insert(*id, node);
                }
            }
        }
        let mut root = self.dict(doc, self.root).unwrap_or_default();
        root.set("ParentTreeNextKey", Object::Integer(self.next_key));
        if let Some(direct) = self.changed.remove(&DIRECT) {
            root.set("ParentTree", Object::Dictionary(direct));
        }
        self.changed.insert(self.root, root);
        let objects = self.changed.into_iter().chain(self.elements).collect();
        (objects, self.pages.into_iter().collect())
    }
}

fn limits(node: &Dictionary) -> Option<(i64, i64)> {
    let l = node.get(b"Limits").ok()?.as_array().ok()?;
    Some((l.first()?.as_i64().ok()?, l.get(1)?.as_i64().ok()?))
}

/// What a screen reader says for an annotation: its text, or what it is.
pub(crate) fn alt_text(annot: &ferrix_core::Annotation) -> String {
    use ferrix_core::{AnnotKind, MarkupStyle, StampContent};
    if let Some(text) = annot
        .props
        .contents
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        return text.to_owned();
    }
    match &annot.kind {
        AnnotKind::Note { .. } | AnnotKind::Reply { .. } => "Comment".into(),
        AnnotKind::Ink { .. } => "Drawing".into(),
        AnnotKind::Square => "Rectangle".into(),
        AnnotKind::Circle => "Ellipse".into(),
        AnnotKind::Line { .. } => "Line".into(),
        AnnotKind::FreeText { .. } => "Text box".into(),
        AnnotKind::Markup { style, .. } => match style {
            MarkupStyle::Highlight => "Highlighted text".into(),
            MarkupStyle::Underline => "Underlined text".into(),
            MarkupStyle::StrikeOut => "Struck-out text".into(),
            MarkupStyle::Squiggly => "Text marked with a squiggly underline".into(),
        },
        AnnotKind::Stamp { content } => match content {
            StampContent::Standard(name) => crate::stamp::standard(name).map_or_else(
                || "Stamp".into(),
                |(label, _)| format!("Stamp: {}", label.to_lowercase()),
            ),
            StampContent::Image(_) | StampContent::Kept => "Stamp".into(),
        },
        AnnotKind::Other { subtype } => subtype.clone(),
    }
}
