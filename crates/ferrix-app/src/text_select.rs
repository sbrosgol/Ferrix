//! Selecting text on PDF pages, copying it, and marking it up (design system › Annotations ›
//! Text selection). The text model is `ferrix-text`; this module loads it per page and turns
//! pointer input into selections.

use std::sync::Arc;

use ferrix_core::{AnnotEdit, AnnotKind, Annotation, MarkupStyle, PageKey, Rgb};
use ferrix_engine::PdfEngine;
use ferrix_text::{PageText, Selection};
use ferrix_types::{DocId, PageTransform, PdfBox, PdfPoint, Point};
use iced::Task;

use crate::{
    app::{App, Message},
    markup::{COLORS, Tool},
    worker::flatten,
};

/// A text selection on one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSelection {
    pub(crate) page: PageKey,
    pub(crate) sel: Selection,
    /// The pointer is still down.
    pub(crate) dragging: bool,
}

/// Highlight colours offered on the selection bar: yellow, green, blue, red.
pub(crate) const HIGHLIGHTS: [usize; 4] = [2, 3, 4, 0];

impl Tool {
    /// The markup a text tool makes.
    pub(crate) fn markup_style(self) -> Option<MarkupStyle> {
        match self {
            Self::Highlight => Some(MarkupStyle::Highlight),
            Self::Underline => Some(MarkupStyle::Underline),
            Self::StrikeOut => Some(MarkupStyle::StrikeOut),
            Self::Squiggly => Some(MarkupStyle::Squiggly),
            _ => None,
        }
    }
}

impl App {
    /// The page's text, when loaded.
    pub(crate) fn page_text(&self, page: PageKey) -> Option<&Arc<PageText>> {
        self.doc.as_ref()?.texts.get(&page)
    }

    /// The intrinsic page transform (no extra rotation) of a page: text boxes are in that space.
    pub(crate) fn intrinsic_transform(&self, page: PageKey) -> Option<PageTransform> {
        let doc = self.doc.as_ref()?;
        let entry = doc.state.pages.entries().iter().find(|e| e.key == page)?;
        doc.sources
            .get(entry.source.0 as usize)?
            .transforms
            .get(entry.page.0 as usize)
            .copied()
    }

    /// Loads the text of visible pages that don't have it yet, one at a time.
    pub(crate) fn load_texts(&mut self) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        let height = self.viewport_height();
        let Some(doc) = self
            .doc
            .as_mut()
            .filter(|d| d.is_measured() && !d.text_busy)
        else {
            return Task::none();
        };
        let visible = doc.strip.visible(doc.scroll, height);
        let next = visible
            .filter_map(|i| doc.state.pages.get(i).copied())
            .find(|e| !doc.texts.contains_key(&e.key));
        let Some(entry) = next else {
            return Task::none();
        };
        let Some(page) = doc.page_ref(&entry) else {
            return Task::none();
        };
        doc.text_busy = true;
        let doc_id = doc.info.id;
        let key = entry.key;
        Task::perform(
            async move {
                flatten(engine.call(move |e| e.text_page(page.doc, page.page)).await)
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::TextLoaded(doc_id, key, result.map(|t| Arc::new(PageText::new(&t))))
            },
        )
    }

    pub(crate) fn text_loaded(
        &mut self,
        doc_id: DocId,
        page: PageKey,
        result: Result<Arc<PageText>, String>,
    ) -> Task<Message> {
        let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
            return Task::none();
        };
        doc.text_busy = false;
        // A page without text (or one PDFium can't read) is remembered as empty.
        doc.texts.insert(
            page,
            result.unwrap_or_else(|_| Arc::new(PageText::default())),
        );
        self.load_texts()
    }

    /// The caret position under `at` (PDF user space) on `page`.
    fn caret(&self, page: PageKey, at: PdfPoint) -> Option<usize> {
        let point = self.intrinsic_transform(page)?.to_page(at.x, at.y);
        self.page_text(page)?.caret_at(point)
    }

    /// Whether `at` is over text on `page`.
    pub(crate) fn over_text(&self, page: PageKey, at: PdfPoint) -> bool {
        let (Some(t), Some(text)) = (self.intrinsic_transform(page), self.page_text(page)) else {
            return false;
        };
        text.is_over_text(t.to_page(at.x, at.y))
    }

    /// Starts a text selection; `clicks` 2 selects a word, 3 a line.
    pub(crate) fn text_press(&mut self, page: PageKey, at: PdfPoint, clicks: u8) -> bool {
        let Some(caret) = self.caret(page, at) else {
            self.text_selection = None;
            return false;
        };
        let shift = self.modifiers.shift();
        let sel = match (clicks, self.text_selection) {
            (_, Some(current)) if shift && current.page == page => Selection {
                anchor: current.sel.anchor,
                focus: caret,
            },
            (2 | 3, _) => {
                let Some(text) = self.page_text(page) else {
                    return false;
                };
                let i = caret.min(text.len().saturating_sub(1));
                let r = if clicks == 2 {
                    text.word_at(i)
                } else {
                    text.line_at(i)
                };
                Selection {
                    anchor: r.start,
                    focus: r.end,
                }
            }
            _ => Selection::caret(caret),
        };
        self.text_selection = Some(TextSelection {
            page,
            sel,
            dragging: true,
        });
        true
    }

    pub(crate) fn text_drag(&mut self, page: PageKey, at: PdfPoint) {
        let caret = self.caret(page, at);
        if let (Some(s), Some(caret)) = (self.text_selection.as_mut(), caret)
            && s.page == page
            && s.dragging
        {
            s.sel.focus = caret;
        }
    }

    /// Ends a drag. With a text markup tool, the selection becomes markup right away.
    pub(crate) fn text_release(&mut self) -> Task<Message> {
        let Some(s) = self.text_selection.as_mut() else {
            return Task::none();
        };
        s.dragging = false;
        if s.sel.is_empty() {
            self.text_selection = None;
            return Task::none();
        }
        let tool = self.markup.as_ref().map(|m| m.tool);
        match tool.and_then(Tool::markup_style) {
            Some(style) => {
                let color = self
                    .markup
                    .as_ref()
                    .map(|m| m.style(tool.unwrap_or(Tool::Highlight)).color)
                    .unwrap_or(COLORS[2].1);
                self.mark_selection(style, color)
            }
            None => Task::none(),
        }
    }

    /// The selection's quads in PDF user space (top-left, top-right, bottom-left, bottom-right),
    /// one per line.
    pub(crate) fn selection_quads(&self) -> Option<(PageKey, Vec<[PdfPoint; 4]>)> {
        let s = self.text_selection?;
        let t = self.intrinsic_transform(s.page)?;
        let text = self.page_text(s.page)?;
        let quads: Vec<[PdfPoint; 4]> = text
            .boxes(s.sel.range())
            .into_iter()
            .map(|r| {
                [
                    t.to_pdf(Point::new(r.x0, r.y0)),
                    t.to_pdf(Point::new(r.x1, r.y0)),
                    t.to_pdf(Point::new(r.x0, r.y1)),
                    t.to_pdf(Point::new(r.x1, r.y1)),
                ]
            })
            .collect();
        (!quads.is_empty()).then_some((s.page, quads))
    }

    /// Turns the selection into a text-markup annotation of `style` in `color`.
    pub(crate) fn mark_selection(&mut self, style: MarkupStyle, color: Rgb) -> Task<Message> {
        let Some((page, quads)) = self.selection_quads() else {
            return Task::none();
        };
        if !self.may_annotate(page) {
            return Task::none();
        }
        let rect = PdfBox::bounding(quads.iter().flatten().copied()).unwrap_or_default();
        let mut annot = Annotation::new(AnnotKind::Markup { style, quads }, rect);
        annot.props.color = Some(color);
        // Multiply keeps the text under a highlight dark at full opacity, as in Acrobat.
        annot.props.opacity = 1.0;
        annot.props.name = Some(ferrix_annot::unique_name());
        let annot = self.with_author(annot);
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let id = doc.state.annots.new_id();
        let index = doc.state.annots.page(page).len();
        let label = match style {
            MarkupStyle::Highlight => "Highlight",
            MarkupStyle::Underline => "Underline",
            MarkupStyle::StrikeOut => "Strike Through",
            MarkupStyle::Squiggly => "Squiggly Underline",
        };
        self.text_selection = None;
        self.apply_annot(
            AnnotEdit::Add {
                page,
                index,
                id,
                annot: Box::new(annot),
            },
            label,
        )
    }

    /// Copies the selected text.
    pub(crate) fn copy_text(&mut self) -> Task<Message> {
        let Some(s) = self.text_selection else {
            return Task::none();
        };
        let Some(text) = self.page_text(s.page).map(|t| t.text(s.sel.range())) else {
            return Task::none();
        };
        self.notice = Some("Copied".into());
        iced::clipboard::write(text)
    }

    /// Selects all text of the current page (⌘A).
    pub(crate) fn select_page_text(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(page) = doc.key_at(doc.current) else {
            return;
        };
        let len = self.page_text(page).map_or(0, |t| t.len());
        if len > 0 {
            self.text_selection = Some(TextSelection {
                page,
                sel: Selection {
                    anchor: 0,
                    focus: len,
                },
                dragging: false,
            });
        }
    }
}
