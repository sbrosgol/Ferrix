//! Document tabs. The active tab's document lives in the app's own fields (`doc`, `writer` and
//! the per-document state around them), so the rest of the app works on "the document" as
//! before; the other tabs are parked here and swapped in when chosen.
//!
//! Results that arrive for a parked document (pages measured, rendered, outline read) are
//! dropped by their handlers, which match on the active document; activating a tab asks again
//! for whatever it still lacks.

use std::path::Path;

use iced::{
    Task,
    widget::{operation, scrollable},
};

use crate::{
    app::{App, FindState, Layout, Message, Zoom, page_scroll_id, sidebar_scroll_id},
    document::Document,
    recovery::{Journal, WriterJournal},
    writer::Writer,
};

/// Everything that belongs to one tab rather than the window.
#[derive(Debug)]
pub(crate) struct Tab {
    pub(crate) doc: Option<Document>,
    pub(crate) writer: Option<Writer>,
    pub(crate) journal: Option<Journal>,
    pub(crate) writer_journal: Option<WriterJournal>,
    pub(crate) markup: Option<crate::markup::Markup>,
    pub(crate) note: Option<crate::annotate_ui::NoteEditor>,
    pub(crate) signed_prompt: bool,
    pub(crate) text_selection: Option<crate::text_select::TextSelection>,
    pub(crate) zoom: Zoom,
    pub(crate) layout: Layout,
    pub(crate) page_input: String,
    pub(crate) find: Option<FindState>,
    pub(crate) sidebar_tab: crate::annotate_ui::SidebarTab,
}

impl Tab {
    /// An empty tab with the window's current view settings.
    fn empty(zoom: Zoom, layout: Layout) -> Self {
        Self {
            doc: None,
            writer: None,
            journal: None,
            writer_journal: None,
            markup: None,
            note: None,
            signed_prompt: false,
            text_selection: None,
            zoom,
            layout,
            page_input: String::new(),
            find: None,
            sidebar_tab: crate::annotate_ui::SidebarTab::default(),
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(Document::is_dirty)
            || self.writer.as_ref().is_some_and(|w| w.editor.is_dirty())
    }

    /// The name shown on the tab.
    pub(crate) fn title(&self) -> String {
        match (&self.writer, &self.doc) {
            (Some(w), _) => w.name.clone(),
            (None, Some(doc)) => doc.name(),
            (None, None) => "New Tab".into(),
        }
    }

    /// The file behind the tab, if any.
    pub(crate) fn path(&self) -> Option<&Path> {
        match (&self.writer, &self.doc) {
            (Some(w), _) => w.path.as_deref(),
            (None, Some(doc)) => doc.origin.path(),
            (None, None) => None,
        }
    }

    pub(crate) fn is_writing(&self) -> bool {
        self.writer.is_some()
    }
}

/// What the tab bar shows for one tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TabLabel {
    pub(crate) title: String,
    pub(crate) path: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) writing: bool,
}

impl App {
    /// Moves the active tab's state out of the app's fields.
    fn park(&mut self) -> Tab {
        let (zoom, layout) = (self.zoom, self.layout);
        let mut doc = self.doc.take();
        if let Some(doc) = doc.as_mut() {
            // Pixels are cheap to make again and dear to keep for every tab.
            doc.images.clear();
            doc.tiles.clear();
            doc.thumbnails.clear();
        }
        Tab {
            doc,
            writer: self.writer.take(),
            journal: self.journal.take(),
            writer_journal: self.writer_journal.take(),
            markup: self.markup.take(),
            note: self.note.take(),
            signed_prompt: std::mem::take(&mut self.signed_prompt),
            text_selection: self.text_selection.take(),
            zoom,
            layout,
            page_input: std::mem::take(&mut self.page_input),
            find: self.find.take(),
            sidebar_tab: self.sidebar_tab,
        }
    }

    /// Makes `tab` the active one: its state moves into the app's fields.
    fn unpark(&mut self, tab: Tab) -> Task<Message> {
        self.doc = tab.doc;
        self.writer = tab.writer;
        self.journal = tab.journal;
        self.writer_journal = tab.writer_journal;
        self.markup = tab.markup;
        self.note = tab.note;
        self.signed_prompt = tab.signed_prompt;
        self.text_selection = tab.text_selection;
        self.zoom = tab.zoom;
        self.layout = tab.layout;
        self.page_input = tab.page_input;
        self.find = tab.find;
        self.sidebar_tab = tab.sidebar_tab;
        self.link_hint = None;
        self.resume()
    }

    /// Picks up the newly active document where it was: work in flight for it while it was
    /// parked was dropped, so ask again, and put both scrolled views back.
    fn resume(&mut self) -> Task<Message> {
        if self.writer.is_some() {
            self.refresh_writer();
            return operation::snap_to(
                crate::app::writer_scroll_id(),
                scrollable::RelativeOffset::START,
            );
        }
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        doc.render_busy = false;
        doc.thumbnail_busy = false;
        doc.text_busy = false;
        let (id, pages) = (doc.info.id, doc.info.page_count);
        let measure = if doc.is_measured() {
            Task::none()
        } else {
            self.measure(id, pages)
        };
        let outline = match self.doc.as_ref().map(|d| d.outline.is_none()) {
            Some(true) => self.load_outline(id),
            _ => Task::none(),
        };
        let annots = match self.doc.as_ref() {
            Some(d) if d.annot_policy.is_none() => {
                crate::app::load_annotations(&d.origin, id, ferrix_core::SourceId::BASE)
            }
            _ => Task::none(),
        };
        let (top, thumbs) = self
            .doc
            .as_ref()
            .map_or((0.0, 0.0), |d| (d.scroll, d.thumb_scroll));
        Task::batch([
            measure,
            outline,
            annots,
            self.render(),
            operation::scroll_to(
                page_scroll_id(),
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(top),
                },
            ),
            operation::scroll_to(
                sidebar_scroll_id(),
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(thumbs),
                },
            ),
            self.next_thumbnail(),
        ])
    }

    /// Whether tabs can change now: not while a save, conversion or reload is under way, whose
    /// result belongs to the active document.
    pub(crate) fn can_switch_tabs(&self) -> bool {
        self.busy.is_none() && self.confirm.is_none() && !self.reloading()
    }

    /// The labels of every tab, in order, and which is active.
    pub(crate) fn tab_labels(&self) -> (Vec<TabLabel>, usize) {
        let label = |tab: &Tab| TabLabel {
            title: tab.title(),
            path: tab.path().map(|p| p.display().to_string()),
            dirty: tab.is_dirty(),
            writing: tab.is_writing(),
        };
        let labels = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                if i == self.active_tab {
                    TabLabel {
                        title: self.active_title(),
                        path: self.active_path().map(|p| p.display().to_string()),
                        dirty: self.active_dirty(),
                        writing: self.writer.is_some(),
                    }
                } else {
                    label(tab)
                }
            })
            .collect();
        (labels, self.active_tab)
    }

    fn active_title(&self) -> String {
        match (&self.writer, &self.doc) {
            (Some(w), _) => w.name.clone(),
            (None, Some(doc)) => doc.name(),
            (None, None) => "New Tab".into(),
        }
    }

    pub(crate) fn active_path(&self) -> Option<&Path> {
        match (&self.writer, &self.doc) {
            (Some(w), _) => w.path.as_deref(),
            (None, Some(doc)) => doc.origin.path(),
            (None, None) => None,
        }
    }

    pub(crate) fn active_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(Document::is_dirty)
            || self.writer.as_ref().is_some_and(|w| w.editor.is_dirty())
    }

    /// Makes room for a document being opened: a new tab at the end, unless the active tab is
    /// empty, which is then used.
    pub(crate) fn open_in_new_tab(&mut self) {
        if self.doc.is_none() && self.writer.is_none() {
            return;
        }
        let parked = self.park();
        self.tabs[self.active_tab] = parked;
        self.tabs.push(Tab::empty(self.zoom, self.layout));
        self.active_tab = self.tabs.len() - 1;
        self.notice = None;
        self.error = None;
        self.hovered_tab = None;
    }

    /// ⌘T: an empty tab showing the start screen (Open, New Document, Recent).
    pub(crate) fn new_empty_tab(&mut self) -> Task<Message> {
        if !self.can_switch_tabs() {
            return Task::none();
        }
        self.open_in_new_tab();
        Task::none()
    }

    /// Names of the tabs with unsaved changes, left to right.
    pub(crate) fn dirty_tab_names(&self) -> Vec<String> {
        let (labels, _) = self.tab_labels();
        labels
            .into_iter()
            .filter(|l| l.dirty)
            .map(|l| l.title)
            .collect()
    }

    /// Switches to tab `index`.
    pub(crate) fn select_tab(&mut self, index: usize) -> Task<Message> {
        if index == self.active_tab || index >= self.tabs.len() || !self.can_switch_tabs() {
            return Task::none();
        }
        self.menu_open = false;
        let parked = self.park();
        self.tabs[self.active_tab] = parked;
        let target = std::mem::replace(&mut self.tabs[index], Tab::empty(self.zoom, self.layout));
        self.active_tab = index;
        self.notice = None;
        self.unpark(target)
    }

    /// The next or previous tab, wrapping around.
    pub(crate) fn step_tab(&mut self, forward: bool) -> Task<Message> {
        let n = self.tabs.len();
        if n < 2 {
            return Task::none();
        }
        let next = if forward {
            (self.active_tab + 1) % n
        } else {
            (self.active_tab + n - 1) % n
        };
        self.select_tab(next)
    }

    /// The tab whose file is `path`, if one is open.
    pub(crate) fn tab_with(&self, path: &Path) -> Option<usize> {
        let same = |p: &Path| p == path || p.canonicalize().ok() == path.canonicalize().ok();
        if self.active_path().is_some_and(same) {
            return Some(self.active_tab);
        }
        self.tabs.iter().position(|t| t.path().is_some_and(same))
    }

    /// Removes the active tab, whose document was already closed, and shows a neighbour. The
    /// last tab stays, empty.
    pub(crate) fn remove_active_tab(&mut self) -> Task<Message> {
        if self.tabs.len() <= 1 {
            return Task::none();
        }
        self.tabs.remove(self.active_tab);
        // The tab to the right takes its place, or the one to the left at the end.
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        let target = std::mem::replace(
            &mut self.tabs[self.active_tab],
            Tab::empty(self.zoom, self.layout),
        );
        self.unpark(target)
    }

    /// A parked tab with unsaved changes, if any.
    pub(crate) fn dirty_parked_tab(&self) -> Option<usize> {
        self.tabs
            .iter()
            .enumerate()
            .find(|(i, t)| *i != self.active_tab && t.is_dirty())
            .map(|(i, _)| i)
    }

    /// Where each parked PDF was left, for remembering on quit.
    pub(crate) fn parked_views(&self) -> Vec<(std::path::PathBuf, ferrix_store::ViewState)> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active_tab)
            .filter_map(|(_, t)| {
                let doc = t.doc.as_ref().filter(|d| d.is_measured())?;
                let path = doc.origin.path()?.to_path_buf();
                Some((
                    path,
                    ferrix_store::ViewState {
                        page: u32::try_from(doc.current).unwrap_or(0),
                        within: doc.scroll_within,
                        zoom: t.zoom.to_saved(),
                        layout: t.layout,
                    },
                ))
            })
            .collect()
    }

    /// Discards the recovery journals of every parked tab (quitting without saving them).
    pub(crate) fn discard_parked_journals(&mut self) {
        let active = self.active_tab;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i == active {
                continue;
            }
            if let Some(j) = tab.journal.take() {
                j.discard();
            }
            if let Some(j) = tab.writer_journal.take() {
                j.discard();
            }
        }
    }
}

pub(crate) fn first_tab(zoom: Zoom, layout: Layout) -> Vec<Tab> {
    vec![Tab::empty(zoom, layout)]
}
