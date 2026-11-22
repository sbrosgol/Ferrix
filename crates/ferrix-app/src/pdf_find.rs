//! Find in PDFs (architecture §5.5, design system › Find): the query is matched page by page as
//! each page's text loads, so results stream in; the first match at or after the current page
//! is shown as soon as it is found.

use std::{collections::HashMap, ops::Range};

use ferrix_core::PageKey;
use ferrix_text::{Query, Selection};
use iced::Task;

use crate::{
    app::{App, Message},
    text_select::TextSelection,
};

/// A search in the open PDF.
#[derive(Debug, Clone)]
pub(crate) struct Search {
    query: Query,
    /// Matches of each page searched so far, as character ranges.
    pub(crate) hits: HashMap<PageKey, Vec<Range<usize>>>,
    /// The match shown: its page and its index there.
    pub(crate) current: Option<(PageKey, usize)>,
    /// Until a match is shown: the page index to look from.
    jump_from: Option<usize>,
}

impl Search {
    /// Matches so far, in document order.
    pub(crate) fn total(&self) -> usize {
        self.hits.values().map(Vec::len).sum()
    }
}

impl App {
    /// Starts (or restarts) the search for the find bar's query in the open PDF.
    pub(crate) fn search_pdf(&mut self) -> Task<Message> {
        let Some(find) = self.find.as_ref() else {
            return Task::none();
        };
        let query = Query::new(&find.query, find.match_case);
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        doc.search = query.map(|query| {
            let hits = doc
                .texts
                .iter()
                .map(|(key, text)| (*key, text.find(&query)))
                .filter(|(_, h)| !h.is_empty())
                .collect();
            Search {
                query,
                hits,
                current: None,
                jump_from: Some(doc.current),
            }
        });
        Task::batch([self.jump_to_first(), self.load_texts()])
    }

    /// Matches a page whose text just loaded.
    pub(crate) fn search_page(&mut self, page: PageKey) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let (Some(search), Some(text)) = (doc.search.as_mut(), doc.texts.get(&page)) else {
            return Task::none();
        };
        let hits = text.find(&search.query);
        if !hits.is_empty() {
            search.hits.insert(page, hits);
        }
        self.jump_to_first()
    }

    /// Shows the first match at or after the page the search started from, once every page
    /// up to it has been searched.
    fn jump_to_first(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let Some(search) = doc.search.as_mut() else {
            return Task::none();
        };
        let Some(from) = search.jump_from else {
            return Task::none();
        };
        let n = doc.state.pages.len();
        for step in 0..n {
            let Some(entry) = doc.state.pages.get((from + step) % n) else {
                break;
            };
            if !doc.texts.contains_key(&entry.key) {
                // Not searched yet: wait for it.
                return Task::none();
            }
            if search.hits.contains_key(&entry.key) {
                search.current = Some((entry.key, 0));
                search.jump_from = None;
                return self.show_match();
            }
        }
        search.jump_from = None;
        Task::none()
    }

    /// The next (or previous) match, wrapping around the document.
    pub(crate) fn step_match(&mut self, back: bool) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let Some(search) = doc.search.as_mut() else {
            return Task::none();
        };
        let order: Vec<(PageKey, usize)> = doc
            .state
            .pages
            .entries()
            .iter()
            .flat_map(|e| {
                let count = search.hits.get(&e.key).map_or(0, Vec::len);
                (0..count).map(move |i| (e.key, i))
            })
            .collect();
        if order.is_empty() {
            return Task::none();
        }
        let next = match search
            .current
            .and_then(|c| order.iter().position(|o| *o == c))
        {
            Some(i) if back => (i + order.len() - 1) % order.len(),
            Some(i) => (i + 1) % order.len(),
            // From the current page.
            None => {
                let page = doc.current;
                let at_or_after = order.iter().position(|(key, _)| {
                    doc.state
                        .pages
                        .entries()
                        .iter()
                        .position(|e| e.key == *key)
                        .is_some_and(|i| i >= page)
                });
                at_or_after.unwrap_or(0)
            }
        };
        search.current = Some(order[next]);
        search.jump_from = None;
        self.show_match()
    }

    /// The current match and its 1-based position among all matches found so far.
    pub(crate) fn match_position(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let search = doc.search.as_ref()?;
        let (page, i) = search.current?;
        let before: usize = doc
            .state
            .pages
            .entries()
            .iter()
            .take_while(|e| e.key != page)
            .map(|e| search.hits.get(&e.key).map_or(0, Vec::len))
            .sum();
        Some(before + i + 1)
    }

    /// Whether pages remain to be searched.
    pub(crate) fn searching(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| {
            d.search.is_some()
                && d.state
                    .pages
                    .entries()
                    .iter()
                    .any(|e| !d.texts.contains_key(&e.key))
        })
    }

    /// Scrolls the current match into view, a third of the way down.
    fn show_match(&mut self) -> Task<Message> {
        let height = self.viewport_height();
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let Some((page, i)) = doc.search.as_ref().and_then(|s| s.current) else {
            return Task::none();
        };
        let Some(index) = doc.state.pages.entries().iter().position(|e| e.key == page) else {
            return Task::none();
        };
        let range = doc
            .search
            .as_ref()
            .and_then(|s| s.hits.get(&page)?.get(i).cloned());
        let y = range
            .and_then(|r| self.range_quads(page, r))
            .and_then(|quads| {
                let entry = doc.state.pages.get(index)?;
                let t = doc
                    .sources
                    .get(entry.source.0 as usize)?
                    .transforms
                    .get(entry.page.0 as usize)?
                    .rotated(entry.rotation);
                let top = quads
                    .iter()
                    .flatten()
                    .map(|p| t.to_page(p.x, p.y).y)
                    .fold(f32::INFINITY, f32::min);
                top.is_finite().then_some(top)
            })
            .unwrap_or(0.0);
        self.place_page(index);
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let scale = doc.strip.zoom * crate::render::LOGICAL_PX_PER_POINT;
        let Some(&page_top) = doc.strip.tops.get(index) else {
            return Task::none();
        };
        let max = (doc.strip.content.height - height).max(0.0);
        let target = (page_top + y * scale - height / 3.0).clamp(0.0, max);
        // Already in view: don't move the page under the reader's eyes.
        let visible = target >= doc.scroll
            && page_top + y * scale <= doc.scroll + height - 40.0
            && page_top + y * scale >= doc.scroll;
        if let Some(doc) = self.doc.as_mut() {
            doc.current = index;
        }
        self.sync_page_input();
        if visible {
            return Task::none();
        }
        Task::batch([
            self.scroll_pages_to(target),
            self.follow_in_sidebar(),
            self.render_pages(),
        ])
    }

    /// Closes Find on a PDF; the current match stays selected, so ⌘F, Esc, ⌘C copies it.
    pub(crate) fn close_pdf_find(&mut self) {
        self.find = None;
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Some(search) = doc.search.take() else {
            return;
        };
        if let Some((page, i)) = search.current
            && let Some(range) = search.hits.get(&page).and_then(|h| h.get(i))
        {
            self.text_selection = Some(TextSelection {
                page,
                sel: Selection {
                    anchor: range.start,
                    focus: range.end,
                },
                dragging: false,
            });
        }
    }
}
