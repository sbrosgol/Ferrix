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

/// Thumbnails beyond this are not rendered until the virtualized sidebar lands in M1.
pub(crate) const MAX_THUMBNAILS: usize = 500;
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
}

impl Source {
    pub(crate) fn new(origin: Origin, doc: DocId) -> Self {
        Self {
            origin,
            doc,
            sizes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Thumbnail {
    pub(crate) rotation: Rotation,
    pub(crate) image: PageImage,
}

/// How a thumbnail click changes the selection.
/// A page rendered for the main view.
#[derive(Debug)]
pub(crate) struct Shown {
    pub(crate) rotation: Rotation,
    pub(crate) zoom: f32,
    pub(crate) scale_factor: f32,
    pub(crate) image: PageImage,
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
    /// Pages the engine couldn't render; shown blank and not tried again, so the error
    /// appears once rather than every time the page scrolls into view.
    pub(crate) render_failed: BTreeSet<PageKey>,
    pub(crate) thumbnails: HashMap<PageKey, Thumbnail>,
    pub(crate) thumbnail_busy: bool,
    /// What the document allows for annotations; `None` until read (ADR-0009).
    pub(crate) annot_policy: Option<crate::annotate::Policy>,
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
            render_failed: BTreeSet::new(),
            thumbnails: HashMap::new(),
            thumbnail_busy: false,
            annot_policy: None,
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
    pub(crate) fn next_thumbnail(&self) -> Option<(PageKey, PageRef)> {
        self.state
            .pages
            .entries()
            .iter()
            .take(MAX_THUMBNAILS)
            .find(|e| {
                self.thumbnails
                    .get(&e.key)
                    .is_none_or(|t| t.rotation != e.rotation)
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

    pub(crate) fn engine_docs(&self) -> Vec<DocId> {
        self.sources.iter().map(|s| s.doc).collect()
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
            },
        )
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
