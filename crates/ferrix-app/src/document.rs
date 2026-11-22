//! The open document as the UI sees it: its page list and history, selection and thumbnails.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use ferrix_core::{AnnotSet, DocEdit, DocState, EditError, History, PageEntry, PageKey, PageList};
use ferrix_engine::DocInfo;
use ferrix_io::FileStamp;
use ferrix_pdf::LayoutPage;
use ferrix_types::{DocId, Rotation, Size};

use crate::{
    render::{PageImage, PageRef},
    strip::Strip,
};

/// Assumed for pages not measured yet (US Letter), so the layout has something to show.
const UNMEASURED: Size = Size {
    width: 612.0,
    height: 792.0,
};

/// Where a document's bytes come from.
#[derive(Debug, Clone)]
pub enum Origin {
    File(PathBuf),
    /// Created or converted in memory and not saved yet.
    Untitled {
        name: String,
        /// Where Save As proposes to put it, e.g. beside the converted file.
        suggested: Option<PathBuf>,
        bytes: Arc<Vec<u8>>,
    },
}

impl Origin {
    pub(crate) fn untitled(name: String, suggested: Option<PathBuf>, bytes: Vec<u8>) -> Self {
        Self::Untitled {
            name,
            suggested,
            bytes: Arc::new(bytes),
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Untitled { .. } => None,
        }
    }

    pub(crate) fn file_name(&self) -> String {
        match self {
            Self::File(path) => file_name(path),
            Self::Untitled { name, .. } => name.clone(),
        }
    }

    /// The location Save As starts from.
    pub(crate) fn save_location(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Untitled { suggested, .. } => suggested.as_deref(),
        }
    }

    /// Reads the bytes. Blocking for files.
    pub(crate) fn read(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::File(path) => {
                std::fs::read(path).map_err(|e| format!("{}: {e}", file_name(path)))
            }
            Self::Untitled { bytes, .. } => Ok(bytes.as_ref().clone()),
        }
    }
}

/// A PDF whose pages appear in the document. Index 0 is the document itself.
#[derive(Debug)]
pub(crate) struct Source {
    pub(crate) origin: Origin,
    pub(crate) doc: DocId,
    /// Size of each page in points, without extra rotation; empty until measured.
    pub(crate) sizes: Vec<Size>,
    /// Each page's user space → page space mapping; empty until annotations are read.
    pub(crate) transforms: Vec<ferrix_types::PageTransform>,
}

impl Source {
    pub(crate) fn new(origin: Origin, doc: DocId) -> Self {
        Self {
            origin,
            doc,
            sizes: Vec::new(),
            transforms: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Thumbnail {
    pub(crate) rotation: Rotation,
    /// The engine document it was rendered from; a newer annotation preview makes it stale.
    pub(crate) source: DocId,
    pub(crate) image: PageImage,
}

/// How a thumbnail click changes the selection.
/// A page rendered for the main view.
#[derive(Debug)]
pub(crate) struct Shown {
    pub(crate) rotation: Rotation,
    pub(crate) source: DocId,
    pub(crate) zoom: f32,
    pub(crate) scale_factor: f32,
    pub(crate) image: PageImage,
    /// A low-resolution base under tiles, rather than the exact page.
    pub(crate) base: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    Replace,
    Toggle,
    Extend,
}

#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) origin: Origin,
    /// File size and mtime when opened, to detect changes made by other apps.
    pub(crate) stamp: Option<FileStamp>,
    pub(crate) info: DocInfo,
    pub(crate) sources: Vec<Source>,
    /// Pages and annotations (ADR-0009), edited together through one history.
    pub(crate) state: DocState,
    pub(crate) history: History<DocState>,
    /// Index of the page on screen: the one filling most of the view.
    pub(crate) current: usize,
    pub(crate) selection: BTreeSet<PageKey>,
    anchor: Option<PageKey>,
    /// Every page stacked for scrolling, at the current zoom.
    pub(crate) strip: Strip,
    /// Vertical scroll position of the page view, in logical pixels.
    pub(crate) scroll: f32,
    /// How far down the current page the view starts, as a fraction of its height; kept
    /// when zoom or the pages change so the view stays on the same spot.
    pub(crate) scroll_within: f32,
    /// Rendered pages near the view.
    pub(crate) images: HashMap<PageKey, Shown>,
    pub(crate) render_busy: bool,
    /// Sharp tiles of pages too large to render whole.
    pub(crate) tiles: ferrix_render::TileCache<crate::render::TileKey, crate::render::Tile>,
    /// Horizontal scroll position of the page view, in logical pixels.
    pub(crate) scroll_x: f32,
    /// Pages the engine couldn't render; shown blank and not tried again, so the error
    /// appears once rather than every time the page scrolls into view.
    pub(crate) render_failed: BTreeSet<PageKey>,
    pub(crate) thumbnails: HashMap<PageKey, Thumbnail>,
    pub(crate) thumbnail_busy: bool,
    /// The sidebar's thumbnail column: scroll offset and visible height, in logical pixels.
    pub(crate) thumb_scroll: f32,
    pub(crate) thumb_view: f32,
    /// What the document allows for annotations; `None` until read (ADR-0009).
    pub(crate) annot_policy: Option<crate::annotate::Policy>,
    /// The document's own bytes, once read for annotations; previews are built from them.
    pub(crate) base_bytes: Option<std::sync::Arc<Vec<u8>>>,
    /// The annotations the screen shows (the file's, or the last preview's).
    pub(crate) previewed: AnnotSet,
    /// Increments per preview request; older results are discarded.
    pub(crate) preview_gen: u64,
    /// The user agreed to annotate this signed document (asked once per document).
    pub(crate) annotate_confirmed: bool,
    /// The tagged-PDF notice was shown.
    pub(crate) tagged_noted: bool,
    /// Each page's text, loaded as pages come into view (`ferrix-text`).
    pub(crate) texts: HashMap<PageKey, std::sync::Arc<ferrix_text::PageText>>,
    pub(crate) text_busy: bool,
    /// Each page's links, read with its text.
    pub(crate) links: HashMap<PageKey, crate::navigate::Links>,
    /// The outline, flattened; `None` until read.
    pub(crate) outline: Option<Vec<crate::navigate::OutlineNode>>,
    /// Back and Forward through jumps.
    pub(crate) trail: crate::navigate::Trail,
    /// The password it was opened with. Encrypted documents are read-only until saving can
    /// keep their encryption.
    pub(crate) encrypted: Option<ferrix_engine::Secret>,
    /// The search of the find bar, while it is open.
    pub(crate) search: Option<crate::pdf_find::Search>,
}

impl Document {
    pub(crate) fn new(origin: Origin, info: DocInfo) -> Self {
        let pages = PageList::identity(info.page_count);
        let mut doc = Self {
            state: DocState::new(pages, AnnotSet::new()),
            stamp: origin.path().and_then(|p| FileStamp::of(p).ok()),
            sources: vec![Source::new(origin.clone(), info.id)],
            origin,
            info,
            history: History::new(),
            current: 0,
            selection: BTreeSet::new(),
            anchor: None,
            strip: Strip::default(),
            scroll: 0.0,
            scroll_within: 0.0,
            images: HashMap::new(),
            render_busy: false,
            tiles: ferrix_render::TileCache::default(),
            scroll_x: 0.0,
            render_failed: BTreeSet::new(),
            thumbnails: HashMap::new(),
            thumbnail_busy: false,
            thumb_scroll: 0.0,
            thumb_view: 800.0,
            annot_policy: None,
            base_bytes: None,
            previewed: AnnotSet::new(),
            preview_gen: 0,
            annotate_confirmed: false,
            tagged_noted: false,
            texts: HashMap::new(),
            text_busy: false,
            links: HashMap::new(),
            outline: None,
            trail: crate::navigate::Trail::default(),
            encrypted: None,
            search: None,
        };
        if let Some(key) = doc.key_at(0) {
            doc.select_only(key);
        }
        doc
    }

    /// A document restored from a crash-recovery journal; its page list is unsaved work.
    /// `sources` are the inserted documents, already open in the engine.
    pub(crate) fn recovered(
        origin: Origin,
        info: DocInfo,
        sources: Vec<Source>,
        pages: PageList,
        annots: Option<AnnotSet>,
    ) -> Self {
        let mut doc = Self::new(origin, info);
        doc.sources.extend(sources);
        doc.state.pages = pages;
        if let Some(annots) = annots {
            doc.state.annots = annots;
        }
        doc.history = History::unsaved();
        if let Some(key) = doc.key_at(0) {
            doc.select_only(key);
        }
        doc
    }

    pub(crate) fn name(&self) -> String {
        self.info
            .metadata
            .title
            .clone()
            .unwrap_or_else(|| self.origin.file_name())
    }

    pub(crate) fn len(&self) -> usize {
        self.state.pages.len()
    }

    /// Encrypted, with or without a password to open: shown, never edited or saved
    /// (ADR-0003), since a save would have to encrypt what it adds.
    pub(crate) fn is_read_only(&self) -> bool {
        self.info.encrypted || self.encrypted.is_some()
    }

    /// Unsaved edits, or a document that was never saved at all.
    pub(crate) fn is_dirty(&self) -> bool {
        self.history.is_dirty() || self.is_untitled()
    }

    pub(crate) fn is_untitled(&self) -> bool {
        self.origin.path().is_none()
    }

    pub(crate) fn key_at(&self, index: usize) -> Option<PageKey> {
        self.state.pages.get(index).map(|e| e.key)
    }

    pub(crate) fn page_ref(&self, entry: &PageEntry) -> Option<PageRef> {
        let source = self.sources.get(entry.source.0 as usize)?;
        Some(PageRef {
            doc: source.doc,
            page: entry.page,
            rotation: entry.rotation,
        })
    }

    /// Whether the document's own pages have been measured, so they can be laid out.
    pub(crate) fn is_measured(&self) -> bool {
        self.sources.first().is_some_and(|s| !s.sizes.is_empty())
    }

    /// Displayed size of a page in points, extra rotation included.
    pub(crate) fn page_size(&self, entry: &PageEntry) -> Size {
        let size = self
            .sources
            .get(entry.source.0 as usize)
            .and_then(|s| s.sizes.get(entry.page.0 as usize))
            .copied()
            .unwrap_or(UNMEASURED);
        if entry.rotation.is_sideways() {
            Size::new(size.height, size.width)
        } else {
            size
        }
    }

    pub(crate) fn page_sizes(&self) -> Vec<Size> {
        self.state
            .pages
            .entries()
            .iter()
            .map(|e| self.page_size(e))
            .collect()
    }

    /// Size of the current page in points, as displayed.
    pub(crate) fn current_size(&self) -> Option<Size> {
        self.state
            .pages
            .get(self.current)
            .map(|e| self.page_size(e))
    }

    /// The pages an action applies to: the selection in document order, or the current page.
    pub(crate) fn targets(&self) -> Vec<PageKey> {
        let selected: Vec<PageKey> = self
            .state
            .pages
            .entries()
            .iter()
            .map(|e| e.key)
            .filter(|k| self.selection.contains(k))
            .collect();
        if selected.is_empty() {
            self.key_at(self.current).into_iter().collect()
        } else {
            selected
        }
    }

    pub(crate) fn select_only(&mut self, key: PageKey) {
        self.selection.clear();
        self.selection.insert(key);
        self.anchor = Some(key);
    }

    pub(crate) fn click(&mut self, key: PageKey, click: Click) {
        match click {
            Click::Replace => self.select_only(key),
            Click::Toggle => {
                if !self.selection.remove(&key) {
                    self.selection.insert(key);
                }
                self.anchor = Some(key);
            }
            Click::Extend => {
                let anchor = self
                    .anchor
                    .and_then(|a| self.state.pages.position(a))
                    .unwrap_or(self.current);
                let Some(end) = self.state.pages.position(key) else {
                    return;
                };
                let (from, to) = (anchor.min(end), anchor.max(end));
                self.selection = (from..=to).filter_map(|i| self.key_at(i)).collect();
            }
        }
        if let Some(index) = self.state.pages.position(key) {
            self.current = index;
        }
    }

    /// Applies an edit through the undo history, keeping the current page in view.
    pub(crate) fn edit(
        &mut self,
        edit: impl Into<DocEdit>,
        label: String,
    ) -> Result<(), EditError> {
        let keep = self.key_at(self.current);
        self.history.perform(&mut self.state, edit.into(), label)?;
        self.after_change(keep);
        Ok(())
    }

    pub(crate) fn undo(&mut self) -> Option<String> {
        let keep = self.key_at(self.current);
        let label = self.history.undo(&mut self.state)?;
        self.after_change(keep);
        Some(label)
    }

    pub(crate) fn redo(&mut self) -> Option<String> {
        let keep = self.key_at(self.current);
        let label = self.history.redo(&mut self.state)?;
        self.after_change(keep);
        Some(label)
    }

    fn after_change(&mut self, keep: Option<PageKey>) {
        let last = self.len().saturating_sub(1);
        self.current = keep
            .and_then(|k| self.state.pages.position(k))
            .unwrap_or(self.current.min(last));
        let present: BTreeSet<PageKey> = self.state.pages.entries().iter().map(|e| e.key).collect();
        self.selection.retain(|k| present.contains(k));
        self.thumbnails.retain(|k, _| present.contains(k));
        self.images.retain(|k, _| present.contains(k));
        self.render_failed.retain(|k| present.contains(k));
        if self.selection.is_empty()
            && let Some(key) = self.key_at(self.current)
        {
            self.select_only(key);
        }
    }

    /// The next page whose thumbnail is missing or shows an outdated rotation.
    /// The thumbnail column for the current pages.
    pub(crate) fn thumb_column(&self) -> crate::thumbs::Column {
        crate::thumbs::Column::new(&self.page_sizes())
    }

    /// The next thumbnail to render: missing or stale, near the visible part of the sidebar,
    /// the current page's first. Thumbnails far from view are dropped to bound memory.
    pub(crate) fn next_thumbnail(&mut self) -> Option<(PageKey, PageRef)> {
        let near = self.thumb_column().near(self.thumb_scroll, self.thumb_view);
        let keep: std::collections::HashSet<PageKey> = near
            .clone()
            .chain(std::iter::once(self.current))
            .filter_map(|i| self.key_at(i))
            .collect();
        self.thumbnails.retain(|k, _| keep.contains(k));
        let entries = self.state.pages.entries();
        std::iter::once(self.current)
            .chain(near)
            .filter_map(|i| entries.get(i))
            .find(|e| {
                let source = self.sources.get(e.source.0 as usize).map(|s| s.doc);
                self.thumbnails
                    .get(&e.key)
                    .is_none_or(|t| t.rotation != e.rotation || Some(t.source) != source)
            })
            .and_then(|e| Some((e.key, self.page_ref(e)?)))
    }

    /// The page list as written on save: source 0 is the document, `n` is `sources[n]`.
    pub(crate) fn layout(&self) -> Vec<LayoutPage> {
        self.state
            .pages
            .entries()
            .iter()
            .map(|e| LayoutPage {
                source: e.source.0 as usize,
                page: e.page.0,
                rotate_by: e.rotation,
            })
            .collect()
    }

    /// Inserted documents, in source order (matching [`Document::layout`]).
    pub(crate) fn imports(&self) -> Vec<Origin> {
        self.sources
            .iter()
            .skip(1)
            .map(|s| s.origin.clone())
            .collect()
    }

    /// Takes in a source's annotations as read from its file. Pages that already hold
    /// annotations (recovered after a crash) keep them. Returns whether the screen, which shows
    /// the file, may now differ from the model and needs a preview (ADR-0009 §7).
    pub(crate) fn adopt_annotations(
        &mut self,
        source: ferrix_core::SourceId,
        loaded: crate::annotate::Loaded,
    ) -> bool {
        let entries: Vec<PageEntry> = self
            .state
            .pages
            .entries()
            .iter()
            .filter(|e| e.source == source)
            .copied()
            .collect();
        let mut recovered = false;
        for entry in entries {
            if self.state.annots.is_loaded(entry.key) {
                recovered = true;
            } else {
                let list = loaded
                    .pages
                    .get(entry.page.0 as usize)
                    .cloned()
                    .unwrap_or_default();
                self.state.annots.insert_loaded(entry.key, list);
            }
        }
        if let Some(s) = self.sources.get_mut(source.0 as usize) {
            s.transforms = loaded.transforms;
        }
        if source == ferrix_core::SourceId::BASE {
            self.annot_policy = Some(loaded.policy);
            self.base_bytes = Some(loaded.bytes);
        }
        if recovered {
            // The screen shows the file, not the recovered edits. An empty `previewed` never
            // equals them, so a preview is built; it comes out empty if nothing differs.
            self.previewed = AnnotSet::new();
            true
        } else {
            // Nothing edited yet: the screen (the file) matches the model.
            self.previewed = self.state.annots.clone();
            false
        }
    }

    /// Every engine document this document holds open: its sources, the original as well as an
    /// annotation preview standing in for it.
    pub(crate) fn engine_docs(&self) -> Vec<DocId> {
        let mut ids: Vec<DocId> = self.sources.iter().map(|s| s.doc).collect();
        if !ids.contains(&self.info.id) {
            ids.push(self.info.id);
        }
        ids
    }
}

/// True if every page refers to an existing page of its source (`page_counts[source]`).
pub(crate) fn pages_fit(pages: &PageList, page_counts: &[u32]) -> bool {
    !pages.is_empty()
        && pages.entries().iter().all(|e| {
            page_counts
                .get(e.source.0 as usize)
                .is_some_and(|&count| e.page.0 < count)
        })
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// "Page" or "3 Pages", for undo labels.
pub(crate) fn pages_label(verb: &str, count: usize) -> String {
    if count == 1 {
        format!("{verb} Page")
    } else {
        format!("{verb} {count} Pages")
    }
}

#[cfg(test)]
mod tests {
    use ferrix_core::Edit;
    use ferrix_engine::DocMetadata;

    use super::*;

    fn document(pages: u32) -> Document {
        Document::new(
            Origin::File(PathBuf::from("/tmp/ferrix-test.pdf")),
            DocInfo {
                id: DocId(1),
                page_count: pages,
                pdf_version: None,
                metadata: DocMetadata::default(),
                encrypted: false,
            },
        )
    }

    fn loaded(pages: usize) -> crate::annotate::Loaded {
        crate::annotate::Loaded {
            pages: vec![
                vec![ferrix_core::Annotation::new(
                    ferrix_core::AnnotKind::Square,
                    ferrix_types::PdfBox::default(),
                )];
                pages
            ],
            policy: crate::annotate::Policy {
                permission: ferrix_annot::Permission::Allowed,
                untaggable: false,
            },
            transforms: Vec::new(),
            bytes: std::sync::Arc::new(Vec::new()),
        }
    }

    #[test]
    fn a_freshly_opened_document_shows_its_annotations_without_a_preview() {
        let mut doc = document(2);
        assert!(!doc.adopt_annotations(ferrix_core::SourceId::BASE, loaded(2)));
        assert_eq!(doc.state.annots.len(), 2);
        assert_eq!(doc.previewed, doc.state.annots);
    }

    /// Regression: recovered annotation edits weren't shown until the next edit, because the
    /// screen (the file) was taken to match the recovered model.
    #[test]
    fn recovered_annotations_need_a_preview_once_the_file_is_read() {
        let fresh = document(2);
        let mut annots = AnnotSet::new();
        let first = fresh.key_at(0).expect("page");
        annots.insert_loaded(first, []); // everything on page 1 was deleted before the crash
        let mut doc = Document::recovered(
            fresh.origin.clone(),
            fresh.info.clone(),
            Vec::new(),
            fresh.state.pages.clone(),
            Some(annots),
        );
        assert!(doc.adopt_annotations(ferrix_core::SourceId::BASE, loaded(2)));
        assert_eq!(
            doc.state.annots.page(first).len(),
            0,
            "the recovered page keeps its (deleted) state"
        );
        assert_ne!(doc.previewed, doc.state.annots, "a preview is due");
    }

    #[test]
    fn shift_click_selects_a_range_from_the_anchor() {
        let mut doc = document(6);
        let (k1, k4) = (doc.key_at(1).expect("page"), doc.key_at(4).expect("page"));
        doc.click(k1, Click::Replace);
        doc.click(k4, Click::Extend);
        assert_eq!(doc.targets().len(), 4);
        assert_eq!(doc.current, 4);
    }

    #[test]
    fn deleting_the_current_page_moves_to_a_neighbour() {
        let mut doc = document(3);
        let last = doc.key_at(2).expect("page");
        doc.click(last, Click::Replace);
        doc.edit(Edit::Remove { keys: vec![last] }, pages_label("Delete", 1))
            .expect("applies");
        assert_eq!(doc.current, 1);
        assert_eq!(doc.targets(), [doc.key_at(1).expect("page")]);
        assert!(doc.is_dirty());
        doc.undo();
        assert!(!doc.is_dirty());
        assert_eq!(doc.len(), 3);
    }

    #[test]
    fn a_failed_page_is_forgotten_once_deleted() {
        let mut doc = document(3);
        let (k0, k1) = (doc.key_at(0).expect("page"), doc.key_at(1).expect("page"));
        doc.render_failed.extend([k0, k1]);
        doc.edit(Edit::Remove { keys: vec![k1] }, pages_label("Delete", 1))
            .expect("applies");
        assert_eq!(doc.render_failed, BTreeSet::from([k0]));
    }

    #[test]
    fn layout_maps_sources_and_rotation() {
        let mut doc = document(2);
        let k0 = doc.key_at(0).expect("page");
        doc.edit(
            Edit::Rotate {
                keys: vec![k0],
                by: Rotation::R90,
            },
            pages_label("Rotate", 1),
        )
        .expect("applies");
        let layout = doc.layout();
        assert_eq!(layout[0].rotate_by, Rotation::R90);
        assert_eq!((layout[1].source, layout[1].page), (0, 1));
    }
}
