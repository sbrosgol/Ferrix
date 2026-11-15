//! Editing state for a written document: caret and selection, cursor movement, hit testing,
//! formatting commands, and undo history. UI-free, so every behaviour is unit-tested here; the app
//! only turns input into calls and draws what [`Editor`] reports.

use unicode_segmentation::UnicodeSegmentation;

use crate::edit::{Applied, Edit, Fragment, Settings};
use crate::fonts::Fonts;
use crate::layout::{Layout, LayoutCache, Line, resolve};
use crate::model::{
    Align, Cell, CharFormat, Color, Document, Image, ImageData, List, ListKind, MAX_TABLE_COLUMNS,
    Paragraph, ParagraphStyle, Position, TextRange,
};

/// A rectangle on a page, in points from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where the caret goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Up,
    Down,
    PageUp,
    PageDown,
    DocumentStart,
    DocumentEnd,
}

/// How many clicks a mouse press is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clicks {
    Single,
    /// Selects a word.
    Double,
    /// Selects a paragraph.
    Triple,
}

/// A change to the table at the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableOp {
    RowAbove,
    RowBelow,
    ColumnLeft,
    ColumnRight,
    DeleteRow,
    DeleteColumn,
    DeleteTable,
}

/// Character formatting that can be switched on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Bold,
    Italic,
    Underline,
    Strikethrough,
}

/// The formatting at the caret (or the start of the selection), for the toolbar.
#[derive(Debug, Clone, PartialEq)]
pub struct CaretFormat {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub color: Color,
    pub style: ParagraphStyle,
    pub align: Align,
    pub line_spacing: f32,
    pub list: Option<ListKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    anchor: Position,
    focus: Position,
}

impl Selection {
    fn caret(at: Position) -> Self {
        Self {
            anchor: at,
            focus: at,
        }
    }

    fn range(self) -> TextRange {
        TextRange::between(self.anchor, self.focus)
    }
}

/// Consecutive edits of one kind in one paragraph join into a single undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    Typing(usize),
    Deleting(usize),
}

impl Merge {
    fn block(self) -> usize {
        match self {
            Self::Typing(b) | Self::Deleting(b) => b,
        }
    }
}

#[derive(Debug, Clone)]
struct Step {
    forward: Edit,
    inverse: Edit,
    label: &'static str,
    before: Selection,
    after: Selection,
    /// For mergeable steps: the merge kind and the paragraph as it was before the first edit.
    merge: Option<(Merge, Paragraph)>,
}

/// How many lines Page Up and Page Down move.
const LINES_PER_PAGE_MOVE: usize = 25;

/// A document being edited.
pub struct Editor {
    doc: Document,
    fonts: Fonts,
    cache: LayoutCache,
    layout: Layout,
    selection: Selection,
    /// Horizontal position kept while moving up and down.
    goal_x: Option<f32>,
    /// Formatting chosen with nothing selected, applied to the next typed text.
    pending: Option<CharFormat>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// `undo.len()` when the document was last saved; `None` once that state is unreachable.
    saved: Option<usize>,
    /// Whether the next edit may join the last undo step.
    can_merge: bool,
    revision: u64,
    applied: Vec<Edit>,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("paragraphs", &self.doc.paragraphs.len())
            .field("pages", &self.layout.pages.len())
            .field("selection", &self.selection)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Opens a document for editing, with the caret at its start.
    pub fn new(doc: Document, mut fonts: Fonts) -> Self {
        let mut cache = LayoutCache::default();
        let layout = Layout::cached(&doc, &mut fonts, &mut cache);
        Self {
            doc,
            fonts,
            cache,
            layout,
            selection: Selection::caret(Position::START),
            goal_x: None,
            pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
            saved: Some(0),
            can_merge: false,
            revision: 0,
            applied: Vec::new(),
        }
    }

    /// Opens a document that has never been saved, or was recovered: it counts as changed.
    pub fn unsaved(doc: Document, fonts: Fonts) -> Self {
        let mut editor = Self::new(doc, fonts);
        editor.saved = None;
        editor
    }

    /// Gives back the fonts, to reuse for the next document.
    pub fn into_fonts(self) -> Fonts {
        self.fonts
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    pub fn fonts_mut(&mut self) -> &mut Fonts {
        &mut self.fonts
    }

    /// Draws a page of the current layout with `scale` pixels per point.
    pub fn rasterize(
        &mut self,
        rasterizer: &mut crate::raster::Rasterizer,
        page: usize,
        scale: f32,
    ) -> Option<crate::raster::Raster> {
        rasterizer.page(&self.layout, page, scale, &mut self.fonts)
    }

    /// A detached copy of what saving needs, to write the PDF on another thread.
    pub fn export_job(&self) -> crate::export::ExportJob {
        crate::export::ExportJob::new(&self.doc, &self.layout, &self.fonts)
    }

    /// Changes whenever the document, the selection or the caret formatting changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn selection(&self) -> TextRange {
        self.selection.range()
    }

    /// The end of the selection that moves.
    pub fn focus(&self) -> Position {
        self.selection.focus
    }

    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.undo.len())
    }

    /// Records that the current state is what is saved on disk.
    pub fn mark_saved(&mut self) {
        self.saved = Some(self.undo.len());
        self.can_merge = false;
        self.revision += 1;
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|s| s.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|s| s.label)
    }

    /// Edits applied since the last call, in order (undo and redo included), for the crash
    /// recovery journal.
    pub fn take_applied(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.applied)
    }

    // ── Undo ──────────────────────────────────────────────────────────────────

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        if self.doc.apply(&step.inverse).is_err() {
            // Cannot happen for inverses produced by `apply`; drop the history rather than
            // leave it inconsistent.
            self.undo.clear();
            self.redo.clear();
            self.saved = None;
            return false;
        }
        self.applied.push(step.inverse.clone());
        self.selection = step.before;
        self.redo.push(step);
        self.after_change();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        if self.doc.apply(&step.forward).is_err() {
            self.redo.clear();
            return false;
        }
        self.applied.push(step.forward.clone());
        self.selection = step.after;
        self.undo.push(step);
        self.after_change();
        true
    }

    fn after_change(&mut self) {
        self.can_merge = false;
        self.goal_x = None;
        self.pending = None;
        self.clamp_selection();
        self.relayout();
    }

    fn relayout(&mut self) {
        self.layout = Layout::cached(&self.doc, &mut self.fonts, &mut self.cache);
        self.revision += 1;
    }

    fn clamp_selection(&mut self) {
        let clamp = |doc: &Document, p: Position| {
            let block = p.block.min(doc.paragraphs.len() - 1);
            let paragraph = &doc.paragraphs[block];
            let mut offset = p.offset.min(paragraph.len());
            while !paragraph.is_boundary(offset) {
                offset -= 1;
            }
            Position::new(block, offset)
        };
        self.selection.anchor = clamp(&self.doc, self.selection.anchor);
        self.selection.focus = clamp(&self.doc, self.selection.focus);
    }

    /// Applies an edit as one undo step and puts the caret where `after` says.
    fn commit(
        &mut self,
        edit: Edit,
        label: &'static str,
        merge: Option<Merge>,
        after: impl FnOnce(&Applied) -> Selection,
    ) {
        let before = self.selection;
        let original = merge.map(|m| self.doc.paragraphs[m.block()].clone());
        let Ok(applied) = self.doc.apply(&edit) else {
            return;
        };
        self.applied.push(edit.clone());
        let after = after(&applied);

        if self.saved.is_some_and(|s| s > self.undo.len()) {
            self.saved = None;
        }
        self.redo.clear();

        let may_join = self.can_merge && self.saved != Some(self.undo.len());
        let joined = match (merge, self.undo.last_mut()) {
            (Some(kind), Some(last)) if may_join && last.after == before => match &last.merge {
                Some((k, first)) if *k == kind => {
                    let block = kind.block();
                    let now = self.doc.paragraphs[block].clone();
                    last.forward = replace_paragraph(block, first.len(), now.clone());
                    last.inverse = replace_paragraph(block, now.len(), first.clone());
                    last.after = after;
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !joined {
            let step = match (merge, original) {
                (Some(kind), Some(first)) => {
                    let block = kind.block();
                    let now = self.doc.paragraphs[block].clone();
                    Step {
                        forward: replace_paragraph(block, first.len(), now.clone()),
                        inverse: replace_paragraph(block, now.len(), first.clone()),
                        label,
                        before,
                        after,
                        merge: Some((kind, first)),
                    }
                }
                _ => Step {
                    forward: edit,
                    inverse: applied.inverse,
                    label,
                    before,
                    after,
                    merge: None,
                },
            };
            self.undo.push(step);
        }

        self.selection = after;
        self.can_merge = merge.is_some();
        self.goal_x = None;
        self.pending = None;
        self.relayout();
    }

    // ── Typing and deleting ───────────────────────────────────────────────────

    /// Types text over the selection. Line breaks start new paragraphs.
    pub fn type_text(&mut self, text: &str) {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
            .collect();
        if text.is_empty() {
            return;
        }
        let multiline = text.contains(['\n', '\r']);
        let label = if multiline { "Paste" } else { "Typing" };
        self.insert(&text, label, !multiline);
    }

    /// Pastes plain text over the selection as one undo step.
    pub fn paste(&mut self, text: &str) {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
            .collect();
        if !text.is_empty() {
            self.insert(&text, "Paste", false);
        }
    }

    fn insert(&mut self, text: &str, label: &'static str, mergeable: bool) {
        self.clear_selection_across_cells();
        let range = self.selection.range();
        let Ok(mut edit) = self.doc.insert_text(range, text) else {
            return;
        };
        if let (Some(format), Edit::Replace { with, .. }) = (self.pending.clone(), &mut edit) {
            *with = with.map_formats(|_| format.clone());
        }
        let merge = (mergeable && range.start.block == range.end.block)
            .then_some(Merge::Typing(range.start.block));
        self.commit(edit, label, merge, |a| Selection::caret(a.range.end));
    }

    /// Splits the paragraph at the caret (Enter).
    pub fn enter(&mut self) {
        self.clear_selection_across_cells();
        let range = self.selection.range();
        // Enter in an empty list item ends the list, as in every word processor.
        let paragraph = &self.doc.paragraphs[range.start.block];
        if range.is_empty() && paragraph.is_empty() && paragraph.format.list.is_some() {
            return self.format_paragraphs("End List", |_, f| f.list = None);
        }
        if let Ok(edit) = self.doc.split_paragraph(range) {
            self.commit(edit, "New Paragraph", None, |a| {
                Selection::caret(a.range.end)
            });
        }
    }

    /// Deletes the selection, or the character (or word) before the caret.
    pub fn backspace(&mut self, word: bool) {
        let range = self.selection.range();
        if !range.is_empty() {
            return self.delete_range(range, "Delete");
        }
        let at = range.start;
        // At the start of a paragraph, Backspace first takes away what sits before its text:
        // a list bullet, a picture, a page break.
        if at.offset == 0 {
            let paragraph = &self.doc.paragraphs[at.block];
            if paragraph.format.list.is_some() {
                return self.format_paragraphs("Remove from List", |_, f| f.list = None);
            }
            if paragraph.image.is_some() {
                return self.remove_image();
            }
            if paragraph.format.page_break_before {
                return self.format_paragraphs("Remove Page Break", |_, f| {
                    f.page_break_before = false;
                });
            }
        }
        if at.offset > 0 {
            let text = self.doc.paragraphs[at.block].text();
            let from = if word {
                word_left(&text, at.offset)
            } else {
                prev_grapheme(&text, at.offset)
            };
            let range = TextRange::between(Position::new(at.block, from), at);
            if let Ok(edit) = self.doc.delete(range) {
                self.commit(edit, "Delete", Some(Merge::Deleting(at.block)), |a| {
                    Selection::caret(a.range.start)
                });
            }
        } else if at.block > 0 && self.same_cell(at.block - 1, at.block) {
            let prev = Position::new(at.block - 1, self.doc.paragraphs[at.block - 1].len());
            self.delete_range(TextRange::between(prev, at), "Delete");
        }
    }

    /// Deletes the selection, or the character (or word) after the caret.
    pub fn delete_forward(&mut self, word: bool) {
        let range = self.selection.range();
        if !range.is_empty() {
            return self.delete_range(range, "Delete");
        }
        let at = range.start;
        let text = self.doc.paragraphs[at.block].text();
        if at.offset < text.len() {
            let to = if word {
                word_right(&text, at.offset)
            } else {
                next_grapheme(&text, at.offset)
            };
            let range = TextRange::between(at, Position::new(at.block, to));
            if let Ok(edit) = self.doc.delete(range) {
                self.commit(edit, "Delete", Some(Merge::Deleting(at.block)), |a| {
                    Selection::caret(a.range.start)
                });
            }
        } else if at.block + 1 < self.doc.paragraphs.len() && self.same_cell(at.block, at.block + 1)
        {
            self.delete_range(
                TextRange::between(at, Position::new(at.block + 1, 0)),
                "Delete",
            );
        }
    }

    fn delete_range(&mut self, range: TextRange, label: &'static str) {
        if let Some(edit) = self.clear_across_cells(range) {
            let start = range.start;
            self.commit(edit, label, None, move |_| Selection::caret(start));
            return;
        }
        if let Ok(edit) = self.doc.delete(range) {
            self.commit(edit, label, None, |a| Selection::caret(a.range.start));
        }
    }

    /// Whether two paragraphs are in the same table cell (or both outside tables), so text may
    /// flow between them.
    fn same_cell(&self, a: usize, b: usize) -> bool {
        self.doc.paragraphs[a].cell == self.doc.paragraphs[b].cell
    }

    /// Deleting a range that crosses table cells clears the text of each paragraph in it
    /// instead of joining them, so the table keeps its shape.
    fn clear_across_cells(&self, range: TextRange) -> Option<Edit> {
        let cells_differ = (range.start.block..range.end.block).any(|b| !self.same_cell(b, b + 1));
        if !cells_differ {
            return None;
        }
        let edits: Vec<Edit> = (range.start.block..=range.end.block)
            .rev()
            .filter_map(|b| {
                let len = self.doc.paragraphs[b].len();
                let from = if b == range.start.block {
                    range.start.offset
                } else {
                    0
                };
                let to = if b == range.end.block {
                    range.end.offset
                } else {
                    len
                };
                let part = TextRange::between(Position::new(b, from), Position::new(b, to));
                (!part.is_empty())
                    .then(|| self.doc.delete(part).ok())
                    .flatten()
            })
            .collect();
        Some(Edit::Batch(edits))
    }

    /// Before typing over a selection that crosses table cells: clears it and collapses the
    /// selection to its start, so the typing goes into the first cell. Returns whether it did.
    fn clear_selection_across_cells(&mut self) {
        let range = self.selection.range();
        if !range.is_empty() && self.clear_across_cells(range).is_some() {
            self.delete_range(range, "Delete");
        }
    }

    /// The selected text, paragraphs separated by line breaks.
    pub fn selected_text(&self) -> String {
        self.doc
            .extract(self.selection.range())
            .map(|f| f.text())
            .unwrap_or_default()
    }

    /// The selection with its formatting, for the clipboard.
    pub fn copy_fragment(&self) -> Option<Fragment> {
        let range = self.selection.range();
        (!range.is_empty())
            .then(|| self.doc.extract(range).ok())
            .flatten()
    }

    /// Pastes formatted paragraphs over the selection as one undo step. The first pasted
    /// paragraph joins the paragraph at the caret and takes on its style; further paragraphs
    /// keep their own.
    pub fn paste_fragment(&mut self, fragment: &Fragment) {
        self.clear_selection_across_cells();
        let range = self.selection.range();
        let target = &self.doc.paragraphs[range.start.block];
        let mut parts = fragment.paragraphs().to_vec();
        if target.cell.is_some() {
            // No tables inside tables: pasted cells become plain paragraphs.
            for p in &mut parts {
                p.cell = None;
            }
        } else {
            // Pasted tables get table numbers of their own.
            let mut next = self.next_table_id();
            let mut renumbered: std::collections::HashMap<u32, u32> =
                std::collections::HashMap::new();
            for p in &mut parts {
                if let Some(cell) = &mut p.cell {
                    cell.table = *renumbered.entry(cell.table).or_insert_with(|| {
                        next += 1;
                        next - 1
                    });
                }
            }
            // A table must not merge with the paragraphs around the caret.
            if parts.first().is_some_and(|p| p.cell.is_some()) {
                parts.insert(0, Paragraph::new(ParagraphStyle::Normal));
            }
            if parts.len() > 1 && parts.last().is_some_and(|p| p.cell.is_some()) {
                parts.push(target.continuation());
            }
        }
        let image = parts[0].image.take();
        parts[0].style = target.style;
        parts[0].format = target.format;
        parts[0].image = target.image.clone().or(image);
        if parts.len() > 1 {
            // The text after the caret stays in a paragraph like the one it was in.
            let last = parts.len() - 1;
            if parts[last].is_empty() && parts[last].image.is_none() {
                parts[last] = target.continuation();
            }
        }
        let Some(with) = Fragment::new(parts) else {
            return;
        };
        if self.doc.validate_fragment(&with).is_err() {
            return;
        }
        self.commit(Edit::Replace { range, with }, "Paste", None, |a| {
            Selection::caret(a.range.end)
        });
    }

    /// Removes the selected text and returns it.
    pub fn cut(&mut self) -> String {
        let text = self.selected_text();
        let range = self.selection.range();
        if !range.is_empty() {
            self.delete_range(range, "Cut");
        }
        text
    }

    // ── Formatting ────────────────────────────────────────────────────────────

    /// Switches a character format on or off for the selection (or for the next typed text).
    pub fn toggle(&mut self, toggle: Toggle) {
        let on = !self.selection_has(toggle);
        let label = match toggle {
            Toggle::Bold => "Bold",
            Toggle::Italic => "Italic",
            Toggle::Underline => "Underline",
            Toggle::Strikethrough => "Strikethrough",
        };
        // Matching the style's own value clears the override, so runs merge back together.
        self.format_chars(label, move |f, style| match toggle {
            Toggle::Bold => f.bold = (on != style.defaults().bold).then_some(on),
            Toggle::Italic => f.italic = on.then_some(true),
            Toggle::Underline => f.underline = on,
            Toggle::Strikethrough => f.strikethrough = on,
        });
    }

    pub fn set_family(&mut self, family: &str) {
        let family = family.to_owned();
        self.format_chars("Font", move |f, _| f.family = Some(family.clone()));
    }

    pub fn set_size(&mut self, size: f32) {
        let size = size.clamp(1.0, 1000.0);
        self.format_chars("Font Size", move |f, _| f.size = Some(size));
    }

    pub fn set_color(&mut self, color: Color) {
        let color = (color != Color::BLACK).then_some(color);
        self.format_chars("Text Colour", move |f, _| f.color = color);
    }

    /// Removes character formatting from the selection, leaving the paragraph style's.
    pub fn clear_formatting(&mut self) {
        self.format_chars("Clear Formatting", |f, _| *f = CharFormat::default());
    }

    fn format_chars(
        &mut self,
        label: &'static str,
        change: impl Fn(&mut CharFormat, ParagraphStyle),
    ) {
        let range = self.selection.range();
        if range.is_empty() {
            let mut format = self.pending.clone().unwrap_or_else(|| {
                self.doc.paragraphs[range.start.block].format_at(range.start.offset)
            });
            change(&mut format, self.doc.paragraphs[range.start.block].style);
            self.pending = Some(format);
            self.revision += 1;
            return;
        }
        let selection = self.selection;
        if let Ok(edit) = self.doc.format_text(range, change) {
            self.commit(edit, label, None, |_| selection);
        }
    }

    /// Whether the whole selection (or the caret's format) has a toggle switched on.
    fn selection_has(&self, toggle: Toggle) -> bool {
        let get = |f: &crate::layout::ResolvedFormat| match toggle {
            Toggle::Bold => f.bold,
            Toggle::Italic => f.italic,
            Toggle::Underline => f.underline,
            Toggle::Strikethrough => f.strikethrough,
        };
        let range = self.selection.range();
        if range.is_empty() {
            let format = self.caret_char_format();
            let paragraph = &self.doc.paragraphs[range.start.block];
            return get(&resolve(&self.doc, paragraph, &format));
        }
        let Ok(fragment) = self.doc.extract(range) else {
            return false;
        };
        let mut any = false;
        for (k, part) in fragment.paragraphs().iter().enumerate() {
            let paragraph = &self.doc.paragraphs[range.start.block + k];
            for run in &part.runs {
                any = true;
                if !get(&resolve(&self.doc, paragraph, &run.format)) {
                    return false;
                }
            }
        }
        any
    }

    pub fn set_style(&mut self, style: ParagraphStyle) {
        self.format_paragraphs("Style", move |s, _| *s = style);
    }

    pub fn set_align(&mut self, align: Align) {
        self.format_paragraphs("Alignment", move |_, f| f.align = Some(align));
    }

    pub fn set_line_spacing(&mut self, spacing: f32) {
        let spacing = spacing.clamp(0.5, 5.0);
        self.format_paragraphs("Line Spacing", move |_, f| f.line_spacing = Some(spacing));
    }

    fn format_paragraphs(
        &mut self,
        label: &'static str,
        change: impl Fn(&mut ParagraphStyle, &mut crate::model::ParagraphFormat),
    ) {
        let selection = self.selection;
        let pending = self.pending.clone();
        if let Ok(edit) = self.doc.format_paragraphs(selection.range(), change) {
            self.commit(edit, label, None, |_| selection);
            self.pending = pending;
        }
    }

    /// Changes the title, page setup or base font.
    pub fn set_settings(&mut self, settings: Settings) {
        let selection = self.selection;
        self.commit(Edit::Settings(settings), "Page Setup", None, |_| selection);
    }

    /// The formatting the toolbar shows.
    pub fn caret_format(&self) -> CaretFormat {
        let at = self.selection.range().start;
        let paragraph = &self.doc.paragraphs[at.block];
        let resolved = resolve(&self.doc, paragraph, &self.caret_char_format());
        CaretFormat {
            family: resolved.family,
            size: resolved.size,
            bold: resolved.bold,
            italic: resolved.italic,
            underline: resolved.underline,
            strikethrough: resolved.strikethrough,
            color: resolved.color,
            style: paragraph.style,
            align: paragraph.format.align.unwrap_or_default(),
            line_spacing: paragraph.format.line_spacing.unwrap_or(1.0),
            list: paragraph.format.list.map(|l| l.kind),
        }
    }

    /// The character format at the caret, or of the first selected character.
    fn caret_char_format(&self) -> CharFormat {
        if let Some(format) = &self.pending {
            return format.clone();
        }
        let range = self.selection.range();
        let paragraph = &self.doc.paragraphs[range.start.block];
        if range.is_empty() {
            paragraph.format_at(range.start.offset)
        } else {
            let text = paragraph.text();
            paragraph.format_at(next_grapheme(&text, range.start.offset))
        }
    }

    // ── Lists, breaks and pictures ────────────────────────────────────────────

    /// Makes the selected paragraphs list items of `kind`, or plain paragraphs if they all
    /// already are.
    pub fn toggle_list(&mut self, kind: ListKind) {
        let range = self.selection.range();
        let all = self.doc.paragraphs[range.start.block..=range.end.block]
            .iter()
            .all(|p| p.format.list.is_some_and(|l| l.kind == kind));
        let label = match kind {
            ListKind::Bullet => "Bulleted List",
            ListKind::Numbered => "Numbered List",
        };
        self.format_paragraphs(label, move |_, f| {
            f.list = if all {
                None
            } else {
                Some(List {
                    kind,
                    level: f.list.map_or(0, |l| l.level),
                })
            };
        });
    }

    /// Tab: in a list, nests the item one level deeper (Shift+Tab: one level out); elsewhere
    /// types a tab.
    pub fn tab(&mut self, back: bool) {
        let range = self.selection.range();
        if self.doc.paragraphs[range.start.block].cell.is_some() {
            return self.next_cell(back);
        }
        let in_list = self.doc.paragraphs[range.start.block].format.list.is_some();
        if in_list && (range.start.offset == 0 || !range.is_empty()) {
            let label = if back {
                "Decrease Indent"
            } else {
                "Increase Indent"
            };
            self.format_paragraphs(label, move |_, f| {
                if let Some(list) = &mut f.list {
                    list.level = if back {
                        list.level.saturating_sub(1)
                    } else {
                        (list.level + 1).min(List::MAX_LEVEL)
                    };
                }
            });
        } else if !back {
            self.type_text("\t");
        }
    }

    /// Starts a new page at the caret.
    pub fn page_break(&mut self) {
        let range = self.selection.range();
        let Ok(Edit::Replace { range, with }) = self.doc.split_paragraph(range) else {
            return;
        };
        let mut parts = with.paragraphs().to_vec();
        if let Some(last) = parts.last_mut() {
            last.format.page_break_before = true;
        }
        let Some(with) = Fragment::new(parts) else {
            return;
        };
        self.commit(Edit::Replace { range, with }, "Page Break", None, |a| {
            Selection::caret(a.range.end)
        });
    }

    /// Puts a picture in its own centred paragraph at the caret (or in the caret's paragraph,
    /// if that is empty and has none).
    pub fn insert_image(&mut self, data: ImageData) {
        let width = data.natural_width().min(self.doc.page.content_width());
        let image = Image {
            data: std::sync::Arc::new(data),
            width,
        };
        let range = self.selection.range();
        let current = &self.doc.paragraphs[range.start.block];
        let reuse = range.is_empty() && current.is_empty() && current.image.is_none();
        let mut picture = Paragraph::new(ParagraphStyle::Normal);
        picture.format.align = Some(Align::Center);
        picture.image = Some(image);
        let parts = if reuse {
            vec![picture]
        } else {
            let mut before = current.empty_like();
            before.runs.clear();
            let after = self.doc.paragraphs[range.end.block].continuation();
            vec![before, picture, after]
        };
        let Some(with) = Fragment::new(parts) else {
            return;
        };
        self.commit(Edit::Replace { range, with }, "Insert Picture", None, |a| {
            Selection::caret(a.range.end)
        });
    }

    /// Removes the picture from the caret's paragraph, keeping its text.
    pub fn remove_image(&mut self) {
        let range = self.selection.range();
        let paragraph = &self.doc.paragraphs[range.start.block];
        if paragraph.image.is_none() {
            return;
        }
        let whole = TextRange::between(
            Position::new(range.start.block, 0),
            Position::new(range.start.block, paragraph.len()),
        );
        let Ok(mut fragment) = self.doc.extract(whole) else {
            return;
        };
        let mut parts = fragment.paragraphs().to_vec();
        parts[0].image = None;
        if let Some(f) = Fragment::new(parts) {
            fragment = f;
        }
        let selection = self.selection;
        self.commit(
            Edit::Replace {
                range: whole,
                with: fragment,
            },
            "Delete Picture",
            None,
            |_| selection,
        );
    }

    /// Whether the caret's paragraph has a picture.
    pub fn has_image(&self) -> bool {
        self.doc.paragraphs[self.selection.focus.block]
            .image
            .is_some()
    }

    // ── Tables ────────────────────────────────────────────────────────────────

    /// Whether the caret is in a table.
    pub fn in_table(&self) -> bool {
        self.doc.paragraphs[self.selection.focus.block]
            .cell
            .is_some()
    }

    /// Inserts an empty table at the caret, with the caret in its first cell.
    pub fn insert_table(&mut self, rows: u32, cols: u32) {
        let (rows, cols) = (rows.clamp(1, 100), cols.clamp(1, MAX_TABLE_COLUMNS));
        let range = self.selection.range();
        let current = &self.doc.paragraphs[range.start.block];
        if current.cell.is_some() || self.doc.paragraphs[range.end.block].cell.is_some() {
            return; // tables inside tables are not supported
        }
        let table = self.next_table_id();
        let mut parts = Vec::new();
        let reuse = range.is_empty() && current.is_empty() && current.image.is_none();
        if !reuse {
            parts.push(current.empty_like());
        }
        for row in 0..rows {
            for col in 0..cols {
                let mut cell = Paragraph::new(ParagraphStyle::Normal);
                cell.cell = Some(Cell { table, row, col });
                parts.push(cell);
            }
        }
        // Text after the caret, or an empty paragraph to carry on typing after the table.
        let mut after = self.doc.paragraphs[range.end.block].continuation();
        after.format.list = None;
        parts.push(after);
        let first = if reuse {
            range.start.block
        } else {
            range.start.block + 1
        };
        let Some(with) = Fragment::new(parts) else {
            return;
        };
        self.commit(
            Edit::Replace { range, with },
            "Insert Table",
            None,
            move |_| Selection::caret(Position::new(first, 0)),
        );
    }

    fn next_table_id(&self) -> u32 {
        self.doc
            .paragraphs
            .iter()
            .filter_map(|p| p.cell.map(|c| c.table + 1))
            .max()
            .unwrap_or(0)
    }

    /// The paragraphs of the table containing `block`: `start..end`.
    fn table_span(&self, block: usize) -> Option<(usize, usize, u32)> {
        let table = self.doc.paragraphs.get(block)?.cell?.table;
        let in_table = |b: usize| {
            self.doc.paragraphs[b]
                .cell
                .is_some_and(|c| c.table == table)
        };
        let mut start = block;
        while start > 0 && in_table(start - 1) {
            start -= 1;
        }
        let mut end = block + 1;
        while end < self.doc.paragraphs.len() && in_table(end) {
            end += 1;
        }
        Some((start, end, table))
    }

    /// A table's cells as rows of columns of paragraphs. Missing cells become empty ones.
    fn table_grid(&self, start: usize, end: usize) -> Vec<Vec<Vec<Paragraph>>> {
        let (mut rows, mut cols) = (1, 1);
        for p in &self.doc.paragraphs[start..end] {
            if let Some(c) = p.cell {
                rows = rows.max(c.row as usize + 1);
                cols = cols.max(c.col as usize + 1);
            }
        }
        let mut grid = vec![vec![Vec::new(); cols]; rows];
        for p in &self.doc.paragraphs[start..end] {
            if let Some(c) = p.cell {
                grid[c.row as usize][c.col as usize].push(p.clone());
            }
        }
        grid
    }

    /// Replaces a table's paragraphs with a rebuilt grid (or removes the table if the grid is
    /// empty) as one undo step, and puts the caret at the start of cell `focus`.
    fn rebuild_table(
        &mut self,
        start: usize,
        end: usize,
        table: u32,
        grid: Vec<Vec<Vec<Paragraph>>>,
        focus: (usize, usize),
        label: &'static str,
    ) {
        let cols = grid.first().map_or(0, Vec::len);
        let mut parts = Vec::new();
        let mut caret = start;
        if grid.is_empty() || cols == 0 {
            parts.push(Paragraph::new(ParagraphStyle::Normal));
        } else {
            for (r, row) in grid.into_iter().enumerate() {
                for (c, mut cell) in row.into_iter().enumerate() {
                    if (r, c) == focus {
                        caret = start + parts.len();
                    }
                    if cell.is_empty() {
                        cell.push(Paragraph::new(ParagraphStyle::Normal));
                    }
                    for mut p in cell {
                        p.cell = Some(Cell {
                            table,
                            row: r as u32,
                            col: c as u32,
                        });
                        parts.push(p);
                    }
                }
            }
        }
        let range = TextRange::between(
            Position::new(start, 0),
            Position::new(end - 1, self.doc.paragraphs[end - 1].len()),
        );
        let Some(with) = Fragment::new(parts) else {
            return;
        };
        self.commit(Edit::Replace { range, with }, label, None, move |_| {
            Selection::caret(Position::new(caret, 0))
        });
    }

    /// Changes the table at the caret.
    pub fn table_op(&mut self, op: TableOp) {
        let focus = self.selection.focus;
        let Some((start, end, table)) = self.table_span(focus.block) else {
            return;
        };
        let Some(cell) = self.doc.paragraphs[focus.block].cell else {
            return;
        };
        let (row, col) = (cell.row as usize, cell.col as usize);
        let mut grid = self.table_grid(start, end);
        let cols = grid[0].len();
        let empty_row = || vec![Vec::new(); cols];
        let (focus, label) = match op {
            TableOp::RowAbove => {
                grid.insert(row, empty_row());
                ((row, col), "Insert Row")
            }
            TableOp::RowBelow => {
                grid.insert(row + 1, empty_row());
                ((row + 1, col), "Insert Row")
            }
            TableOp::ColumnLeft | TableOp::ColumnRight => {
                if cols as u32 >= MAX_TABLE_COLUMNS {
                    return;
                }
                let at = if op == TableOp::ColumnLeft {
                    col
                } else {
                    col + 1
                };
                for r in &mut grid {
                    r.insert(at, Vec::new());
                }
                ((row, at), "Insert Column")
            }
            TableOp::DeleteRow => {
                grid.remove(row);
                ((row.min(grid.len().saturating_sub(1)), col), "Delete Row")
            }
            TableOp::DeleteColumn => {
                for r in &mut grid {
                    r.remove(col);
                }
                let cols = grid[0].len();
                if cols == 0 {
                    grid.clear();
                }
                ((row, col.min(cols.saturating_sub(1))), "Delete Column")
            }
            TableOp::DeleteTable => {
                grid.clear();
                ((0, 0), "Delete Table")
            }
        };
        self.rebuild_table(start, end, table, grid, focus, label);
    }

    /// Tab in a table: to the next cell (adding a row after the last one); Shift+Tab: back.
    fn next_cell(&mut self, back: bool) {
        let focus = self.selection.focus;
        let (Some((start, end, _)), Some(cell)) = (
            self.table_span(focus.block),
            self.doc.paragraphs[focus.block].cell,
        ) else {
            return;
        };
        let grid = self.table_grid(start, end);
        let (rows, cols) = (grid.len(), grid[0].len());
        let index = cell.row as usize * cols + cell.col as usize;
        let target = if back {
            match index.checked_sub(1) {
                Some(i) => i,
                None => return,
            }
        } else if index + 1 < rows * cols {
            index + 1
        } else {
            // From the last cell: a new row, starting in its first cell.
            self.table_op(TableOp::RowBelow);
            let row = rows as u32;
            if let Some((start, end, _)) = self.table_span(self.selection.focus.block)
                && let Some(b) = (start..end).find(|&b| {
                    self.doc.paragraphs[b]
                        .cell
                        .is_some_and(|x| x.row == row && x.col == 0)
                })
            {
                self.select(TextRange::caret(Position::new(b, 0)));
            }
            return;
        };
        let (r, c) = ((target / cols) as u32, (target % cols) as u32);
        if let Some(b) = (start..end).find(|&b| {
            self.doc.paragraphs[b]
                .cell
                .is_some_and(|x| x.row == r && x.col == c)
        }) {
            // Like other word processors: select the cell's text, so typing replaces it.
            let len = self.doc.paragraphs[b].len();
            self.select(TextRange::between(
                Position::new(b, 0),
                Position::new(b, len),
            ));
        }
    }

    // ── Find and replace ──────────────────────────────────────────────────────

    /// Every occurrence of `query`, in document order. Matches never span paragraphs.
    pub fn find_all(&self, query: &str, match_case: bool) -> Vec<TextRange> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        for (block, paragraph) in self.doc.paragraphs.iter().enumerate() {
            let text = paragraph.text();
            for (start, end) in find_in(&text, query, match_case) {
                found.push(TextRange::between(
                    Position::new(block, start),
                    Position::new(block, end),
                ));
            }
        }
        found
    }

    /// Selects the next (or previous) match after the selection, wrapping around the
    /// document. Returns whether there was one.
    pub fn find_next(&mut self, query: &str, match_case: bool, backwards: bool) -> bool {
        let matches = self.find_all(query, match_case);
        let current = self.selection.range();
        let next = if backwards {
            matches
                .iter()
                .rev()
                .find(|m| m.start < current.start)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|m| {
                    m.start > current.start || (m.start == current.start && current.is_empty())
                })
                .or_else(|| matches.first())
        };
        match next.copied() {
            Some(range) => {
                self.select(range);
                true
            }
            None => false,
        }
    }

    /// Replaces the selection if it is a match, then selects the next match.
    pub fn replace(&mut self, query: &str, replacement: &str, match_case: bool) -> bool {
        let range = self.selection.range();
        if !range.is_empty()
            && self.find_all(query, match_case).contains(&range)
            && let Ok(edit) = self.doc.insert_text(range, replacement)
        {
            self.commit(edit, "Replace", None, |a| Selection::caret(a.range.end));
        }
        self.find_next(query, match_case, false)
    }

    /// Replaces every match as one undoable step; returns how many.
    pub fn replace_all(&mut self, query: &str, replacement: &str, match_case: bool) -> usize {
        let matches = self.find_all(query, match_case);
        // Back to front, so earlier matches keep their positions.
        let edits: Vec<Edit> = matches
            .iter()
            .rev()
            .filter_map(|range| self.doc.insert_text(*range, replacement).ok())
            .collect();
        let count = edits.len();
        if count > 0 {
            let caret = self.selection.focus;
            self.commit(Edit::Batch(edits), "Replace All", None, |_| {
                Selection::caret(caret)
            });
        }
        count
    }

    // ── Moving and selecting ──────────────────────────────────────────────────

    pub fn select_all(&mut self) {
        self.set_selection(Selection {
            anchor: Position::START,
            focus: self.doc.end(),
        });
    }

    /// Selects `range`, with the caret at its end.
    pub fn select(&mut self, range: TextRange) {
        if self.doc.check(range.start).is_ok() && self.doc.check(range.end).is_ok() {
            self.set_selection(Selection {
                anchor: range.start,
                focus: range.end,
            });
        }
    }

    fn set_selection(&mut self, selection: Selection) {
        if selection != self.selection || self.pending.is_some() {
            self.selection = selection;
            self.pending = None;
            self.revision += 1;
        }
        self.can_merge = false;
    }

    /// Moves the caret, or extends the selection when `extend` is set.
    pub fn move_caret(&mut self, motion: Motion, extend: bool) {
        let range = self.selection.range();
        let focus = self.selection.focus;
        let vertical = matches!(
            motion,
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown
        );
        // Moving up and down keeps to the column where the vertical movement started.
        let goal = if vertical {
            self.goal_x.or_else(|| self.caret_rect().map(|(_, r)| r.x))
        } else {
            None
        };
        self.goal_x = goal;
        // Without shift, Left and Right first collapse a selection to its edge.
        let target = match motion {
            Motion::Left if !extend && !range.is_empty() => range.start,
            Motion::Right if !extend && !range.is_empty() => range.end,
            _ => self.motion_target(focus, motion),
        };
        let selection = if extend {
            Selection {
                anchor: self.selection.anchor,
                focus: target,
            }
        } else {
            Selection::caret(target)
        };
        self.set_selection(selection);
        self.goal_x = goal;
    }

    fn motion_target(&mut self, at: Position, motion: Motion) -> Position {
        let paragraphs = &self.doc.paragraphs;
        let text = paragraphs[at.block].text();
        match motion {
            Motion::Left if at.offset > 0 => {
                Position::new(at.block, prev_grapheme(&text, at.offset))
            }
            Motion::WordLeft if at.offset > 0 => {
                Position::new(at.block, word_left(&text, at.offset))
            }
            Motion::Left | Motion::WordLeft if at.block > 0 => {
                Position::new(at.block - 1, paragraphs[at.block - 1].len())
            }
            Motion::Right if at.offset < text.len() => {
                Position::new(at.block, next_grapheme(&text, at.offset))
            }
            Motion::WordRight if at.offset < text.len() => {
                Position::new(at.block, word_right(&text, at.offset))
            }
            Motion::Right | Motion::WordRight if at.block + 1 < paragraphs.len() => {
                Position::new(at.block + 1, 0)
            }
            Motion::Left | Motion::WordLeft | Motion::Right | Motion::WordRight => at,
            Motion::LineStart => self
                .line_of(at)
                .map_or(at, |(_, line)| Position::new(at.block, line.range.start)),
            Motion::LineEnd => self.line_of(at).map_or(at, |(_, line)| {
                Position::new(at.block, self.visible_end(line))
            }),
            Motion::DocumentStart => Position::START,
            Motion::DocumentEnd => self.doc.end(),
            Motion::Up => self.vertical(at, -1),
            Motion::Down => self.vertical(at, 1),
            Motion::PageUp => self.vertical(at, -(LINES_PER_PAGE_MOVE as isize)),
            Motion::PageDown => self.vertical(at, LINES_PER_PAGE_MOVE as isize),
        }
    }

    /// The position `lines` lines above (negative) or below the caret, at the goal column.
    fn vertical(&self, at: Position, lines: isize) -> Position {
        let Some((mut page, line)) = self.line_of(at) else {
            return at;
        };
        let x = self.goal_x.unwrap_or_else(|| self.x_at(line, at.offset));
        let mut line = line;
        for _ in 0..lines.unsigned_abs() {
            match self.next_line(page, line, x, lines > 0) {
                Some((p, l)) => {
                    page = p;
                    line = l;
                }
                None if lines > 0 => return self.doc.end(),
                None => return Position::START,
            }
        }
        Position::new(line.block, self.hit_line(line, x))
    }

    /// The nearest line below (or above) `from`, preferring one under column `x`. In a table,
    /// that keeps the caret in its column.
    fn next_line(&self, page: usize, from: &Line, x: f32, down: bool) -> Option<(usize, &Line)> {
        let key = |p: usize, l: &Line| (p, l.top);
        let start = key(page, from);
        let mut candidates: Vec<(usize, &Line)> = self
            .lines()
            .filter(|(p, l)| {
                let k = key(*p, l);
                if down {
                    k.0 > start.0 || (k.0 == start.0 && k.1 > start.1 + 0.01)
                } else {
                    k.0 < start.0 || (k.0 == start.0 && k.1 < start.1 - 0.01)
                }
            })
            .collect();
        candidates.sort_by(|a, b| {
            let (ka, kb) = (key(a.0, a.1), key(b.0, b.1));
            let order = ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1));
            if down { order } else { order.reverse() }
        });
        let contains = |l: &Line| x >= l.left - 0.5 && x <= l.left + l.width + 0.5;
        candidates
            .iter()
            .find(|(_, l)| contains(l))
            .or(candidates.first())
            .copied()
    }

    /// Places the caret (or extends the selection) at a point on a page.
    pub fn click(&mut self, page: usize, x: f32, y: f32, clicks: Clicks, extend: bool) {
        let at = self.hit(page, x, y);
        let selection = match clicks {
            Clicks::Single if extend => Selection {
                anchor: self.selection.anchor,
                focus: at,
            },
            Clicks::Single => Selection::caret(at),
            Clicks::Double => {
                let text = self.doc.paragraphs[at.block].text();
                let (start, end) = word_at(&text, at.offset);
                Selection {
                    anchor: Position::new(at.block, start),
                    focus: Position::new(at.block, end),
                }
            }
            Clicks::Triple => Selection {
                anchor: Position::new(at.block, 0),
                focus: Position::new(at.block, self.doc.paragraphs[at.block].len()),
            },
        };
        self.goal_x = None;
        self.set_selection(selection);
    }

    /// Extends the selection to a point while the mouse is dragged.
    pub fn drag(&mut self, page: usize, x: f32, y: f32) {
        let focus = self.hit(page, x, y);
        self.set_selection(Selection {
            anchor: self.selection.anchor,
            focus,
        });
    }

    // ── Geometry ──────────────────────────────────────────────────────────────

    /// Every line with its page, in reading order.
    fn lines(&self) -> impl Iterator<Item = (usize, &Line)> {
        self.layout
            .pages
            .iter()
            .enumerate()
            .flat_map(|(p, page)| page.lines.iter().map(move |l| (p, l)))
    }

    /// The line showing a position. A position where a wrapped line ends shows at the start of
    /// the next line.
    fn line_of(&self, at: Position) -> Option<(usize, &Line)> {
        let mut last = None;
        for (page, line) in self.lines().filter(|(_, l)| l.block == at.block) {
            if line.range.start <= at.offset && at.offset < line.range.end {
                return Some((page, line));
            }
            if line.range.start <= at.offset {
                last = Some((page, line));
            }
        }
        last
    }

    /// Where a line ends for the caret: before the spaces a wrapped line ends with.
    fn visible_end(&self, line: &Line) -> usize {
        let paragraph = &self.doc.paragraphs[line.block];
        if line.range.end >= paragraph.len() {
            return line.range.end;
        }
        let text = paragraph.text();
        let trimmed = text[line.range.clone()].trim_end().len();
        if trimmed == 0 {
            line.range.end
        } else {
            line.range.start + trimmed
        }
    }

    /// The x position of a caret at `offset` on `line`.
    fn x_at(&self, line: &Line, offset: usize) -> f32 {
        if line.glyphs.is_empty() {
            return match line.align {
                Align::Left | Align::Justify => line.left,
                Align::Center => line.left + line.width / 2.0,
                Align::Right => line.left + line.width,
            };
        }
        let edges = |g: &crate::layout::PlacedGlyph| {
            if g.rtl {
                (g.x + g.advance, g.x)
            } else {
                (g.x, g.x + g.advance)
            }
        };
        if let Some(g) = line.glyphs.iter().find(|g| g.range.start == offset) {
            return edges(g).0;
        }
        if let Some(g) = line.glyphs.iter().find(|g| g.range.end == offset) {
            return edges(g).1;
        }
        if let Some(g) = line
            .glyphs
            .iter()
            .find(|g| g.range.start < offset && offset < g.range.end)
        {
            let (start, end) = edges(g);
            let t = (offset - g.range.start) as f32 / (g.range.end - g.range.start) as f32;
            return start + (end - start) * t;
        }
        line.glyphs.last().map_or(line.left, |g| edges(g).1)
    }

    /// The offset on `line` nearest to `x`.
    fn hit_line(&self, line: &Line, x: f32) -> usize {
        let mut best = (f32::INFINITY, line.range.start);
        for g in &line.glyphs {
            let (left, right) = if g.rtl {
                (g.range.end, g.range.start)
            } else {
                (g.range.start, g.range.end)
            };
            for (edge, offset) in [(g.x, left), (g.x + g.advance, right)] {
                let distance = (edge - x).abs();
                if distance < best.0 {
                    best = (distance, offset);
                }
            }
        }
        best.1.clamp(line.range.start, self.visible_end(line))
    }

    /// The text position nearest to a point on a page.
    pub fn hit(&self, page: usize, x: f32, y: f32) -> Position {
        let page = page.min(self.layout.pages.len() - 1);
        let lines = &self.layout.pages[page].lines;
        // The line under the point: first by column (horizontal distance), then by height.
        // Plain text spans the full width, so this only matters in tables.
        let distance = |l: &Line| {
            let outside = |v: f32, lo: f32, hi: f32| (lo - v).max(v - hi).max(0.0);
            (
                outside(x, l.left, l.left + l.width),
                outside(y, l.top, l.top + l.height),
            )
        };
        let Some(line) = lines.iter().min_by(|a, b| {
            let (da, db) = (distance(a), distance(b));
            da.0.total_cmp(&db.0).then(da.1.total_cmp(&db.1))
        }) else {
            return self.doc.end();
        };
        Position::new(line.block, self.hit_line(line, x))
    }

    /// Where to draw the caret: its page and rectangle (zero width).
    pub fn caret_rect(&self) -> Option<(usize, Rect)> {
        let at = self.selection.focus;
        let (page, line) = self.line_of(at)?;
        Some((
            page,
            Rect {
                x: self.x_at(line, at.offset),
                y: line.top,
                width: 0.0,
                height: line.height,
            },
        ))
    }

    /// Rectangles covering the selection, by page.
    pub fn selection_rects(&self) -> Vec<(usize, Rect)> {
        self.range_rects(self.selection.range())
    }

    /// Rectangles covering any range of text (search matches, for example), by page.
    pub fn range_rects(&self, range: TextRange) -> Vec<(usize, Rect)> {
        if range.is_empty() {
            return Vec::new();
        }
        let mut rects = Vec::new();
        for (page, line) in self.lines() {
            if line.block < range.start.block || line.block > range.end.block {
                continue;
            }
            let lo = if line.block == range.start.block {
                range.start.offset.max(line.range.start)
            } else {
                line.range.start
            };
            let hi = if line.block == range.end.block {
                range.end.offset.min(line.range.end)
            } else {
                line.range.end
            };
            let last_line = line.range.end >= self.doc.paragraphs[line.block].len();
            // The paragraph break is selected too: show it as a little extra width.
            let past_end = last_line && line.block < range.end.block;
            if lo >= hi && !past_end {
                continue;
            }
            let (a, b) = if lo < hi {
                let (a, b) = (self.x_at(line, lo), self.x_at(line, hi));
                (a.min(b), a.max(b))
            } else {
                let x = self.x_at(line, line.range.end);
                (x, x)
            };
            let extra = if past_end { line.height * 0.3 } else { 0.0 };
            rects.push((
                page,
                Rect {
                    x: a,
                    y: line.top,
                    width: b - a + extra,
                    height: line.height,
                },
            ));
        }
        rects
    }

    /// Words in the document, for the status bar.
    pub fn word_count(&self) -> usize {
        self.doc
            .paragraphs
            .iter()
            .map(|p| p.text().unicode_words().count())
            .sum()
    }
}

/// An edit replacing the whole text of paragraph `block` (currently `len` bytes long).
fn replace_paragraph(block: usize, len: usize, with: Paragraph) -> Edit {
    Edit::Replace {
        range: TextRange::between(Position::new(block, 0), Position::new(block, len)),
        with: Fragment::single(with),
    }
}

/// Byte ranges of `query` in `text`, without overlaps. Case-insensitive matching compares
/// lower-cased characters one for one.
fn find_in(text: &str, query: &str, match_case: bool) -> Vec<(usize, usize)> {
    let fold = |c: char| -> char {
        if match_case {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let query: Vec<char> = query.chars().map(fold).collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + query.len() <= chars.len() {
        if chars[i..i + query.len()]
            .iter()
            .zip(&query)
            .all(|((_, c), q)| fold(*c) == *q)
        {
            let start = chars[i].0;
            let end = chars.get(i + query.len()).map_or(text.len(), |(o, _)| *o);
            found.push((start, end));
            i += query.len();
        } else {
            i += 1;
        }
    }
    found
}

fn prev_grapheme(text: &str, offset: usize) -> usize {
    text[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_grapheme(text: &str, offset: usize) -> usize {
    text[offset..]
        .graphemes(true)
        .next()
        .map_or(offset, |g| offset + g.len())
}

fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

/// The start of the word before `offset`.
fn word_left(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .rev()
        .find(|(i, w)| *i < offset && is_word(w))
        .map_or(0, |(i, _)| i)
}

/// The end of the word after `offset`.
fn word_right(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .map(|(i, w)| (i, i + w.len(), w))
        .find(|(_, end, w)| *end > offset && is_word(w))
        .map_or(text.len(), |(_, end, _)| end)
}

/// The word (or run of spaces or punctuation) around `offset`.
fn word_at(text: &str, offset: usize) -> (usize, usize) {
    let mut found = (offset, offset);
    for (i, w) in text.split_word_bound_indices() {
        let end = i + w.len();
        if i <= offset && offset < end {
            return (i, end);
        }
        if end == offset {
            found = (i, end);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn editor(paragraphs: &[&str]) -> Editor {
        let mut doc = Document::new();
        doc.paragraphs = paragraphs
            .iter()
            .map(|t| Paragraph::new(ParagraphStyle::Normal).with_text(t, CharFormat::default()))
            .collect();
        Editor::new(doc, Fonts::bundled())
    }

    fn texts(e: &Editor) -> Vec<String> {
        e.document()
            .paragraphs
            .iter()
            .map(Paragraph::text)
            .collect()
    }

    fn at(block: usize, offset: usize) -> Position {
        Position::new(block, offset)
    }

    #[test]
    fn typing_moves_the_caret_and_undoes_in_one_step() {
        let mut e = editor(&[""]);
        for c in ["H", "e", "l", "l", "o"] {
            e.type_text(c);
        }
        assert_eq!(texts(&e), ["Hello"]);
        assert_eq!(e.focus(), at(0, 5));
        assert!(e.is_dirty());
        assert_eq!(e.undo_label(), Some("Typing"));
        assert!(e.undo());
        assert_eq!(texts(&e), [""]);
        assert!(!e.is_dirty());
        assert!(!e.undo());
        assert!(e.redo());
        assert_eq!(texts(&e), ["Hello"]);
        assert_eq!(e.focus(), at(0, 5));
    }

    #[test]
    fn moving_the_caret_starts_a_new_undo_step() {
        let mut e = editor(&[""]);
        e.type_text("ab");
        e.move_caret(Motion::Left, false);
        e.type_text("X");
        assert_eq!(texts(&e), ["aXb"]);
        e.undo();
        assert_eq!(texts(&e), ["ab"]);
        e.undo();
        assert_eq!(texts(&e), [""]);
    }

    #[test]
    fn backspace_deletes_graphemes_words_and_joins_paragraphs() {
        let mut e = editor(&["one", "two e\u{301}"]);
        e.move_caret(Motion::DocumentEnd, false);
        e.backspace(false);
        // "é" written as e + combining accent goes in one keystroke.
        assert_eq!(texts(&e), ["one", "two "]);
        e.backspace(true);
        assert_eq!(texts(&e), ["one", ""]);
        e.backspace(false);
        assert_eq!(texts(&e), ["one"]);
        assert_eq!(e.focus(), at(0, 3));
        // The two deletions within "two é" were one step; joining was another.
        e.undo();
        assert_eq!(texts(&e), ["one", ""]);
        e.undo();
        assert_eq!(texts(&e), ["one", "two e\u{301}"]);
    }

    #[test]
    fn delete_forward_and_typing_over_a_selection() {
        let mut e = editor(&["hello world"]);
        e.move_caret(Motion::DocumentStart, false);
        e.delete_forward(true);
        assert_eq!(texts(&e), [" world"]);
        e.move_caret(Motion::DocumentEnd, true);
        e.type_text("x");
        assert_eq!(texts(&e), ["x"]);
    }

    #[test]
    fn enter_and_paste_split_paragraphs() {
        let mut e = editor(&["ab"]);
        e.move_caret(Motion::Right, false);
        e.enter();
        assert_eq!(texts(&e), ["a", "b"]);
        assert_eq!(e.focus(), at(1, 0));
        e.paste("1\n2");
        assert_eq!(texts(&e), ["a", "1", "2b"]);
        assert_eq!(e.focus(), at(2, 1));
        e.undo();
        assert_eq!(texts(&e), ["a", "b"]);
    }

    #[test]
    fn word_and_line_motions() {
        let mut e = editor(&["alpha beta, gamma"]);
        e.move_caret(Motion::WordRight, false);
        assert_eq!(e.focus(), at(0, 5));
        e.move_caret(Motion::WordRight, false);
        assert_eq!(e.focus(), at(0, 10));
        e.move_caret(Motion::WordLeft, false);
        assert_eq!(e.focus(), at(0, 6));
        e.move_caret(Motion::LineEnd, false);
        assert_eq!(e.focus(), at(0, 17));
        e.move_caret(Motion::LineStart, true);
        assert_eq!(e.selection(), TextRange::between(at(0, 0), at(0, 17)));
        e.move_caret(Motion::Right, false);
        assert_eq!(
            e.focus(),
            at(0, 17),
            "Right collapses to the selection's end"
        );
    }

    #[test]
    fn up_and_down_keep_the_column() {
        // Same letters on both long lines, so the column lands on the same offset.
        let mut e = editor(&["abcdef", "x", "abcdef"]);
        e.move_caret(Motion::Right, false);
        e.move_caret(Motion::Right, false);
        e.move_caret(Motion::Right, false);
        e.move_caret(Motion::Down, false);
        assert_eq!(e.focus(), at(1, 1), "short line: end of it");
        e.move_caret(Motion::Down, false);
        assert_eq!(e.focus(), at(2, 3), "back to the remembered column");
        e.move_caret(Motion::Down, false);
        assert_eq!(e.focus(), e.document().end());
        e.move_caret(Motion::PageUp, false);
        assert_eq!(e.focus(), Position::START);
    }

    #[test]
    fn wrapped_lines_have_their_own_start_and_end() {
        let text = "word ".repeat(60);
        let mut e = editor(&[text.trim_end()]);
        let first_end = e.layout().pages[0].lines[0].range.end;
        e.move_caret(Motion::LineEnd, false);
        let caret = e.focus().offset;
        assert!(caret < first_end, "caret stays before the wrapping space");
        let (_, rect) = e.caret_rect().unwrap();
        assert_eq!(
            rect.y,
            e.layout().pages[0].lines[0].top,
            "and on the first line"
        );
        e.move_caret(Motion::Down, false);
        e.move_caret(Motion::LineStart, false);
        assert_eq!(e.focus().offset, first_end);
    }

    #[test]
    fn clicks_place_the_caret_and_select_words_and_paragraphs() {
        let mut e = editor(&["Hello brave world", "Second"]);
        let line = e.layout().pages[0].lines[0].clone();
        let g = &line.glyphs[7];
        let y = line.top + line.height / 2.0;
        e.click(0, g.x + 0.1, y, Clicks::Single, false);
        assert_eq!(e.focus(), at(0, 7));
        e.click(0, g.x + 0.1, y, Clicks::Double, false);
        assert_eq!(e.selected_text(), "brave");
        e.click(0, g.x, y, Clicks::Triple, false);
        assert_eq!(e.selected_text(), "Hello brave world");
        // Far left, right and below the text.
        e.click(0, 0.0, y, Clicks::Single, false);
        assert_eq!(e.focus(), at(0, 0));
        e.click(0, 590.0, y, Clicks::Single, false);
        assert_eq!(e.focus(), at(0, 17));
        e.click(0, 100.0, 800.0, Clicks::Single, false);
        assert_eq!(e.focus().block, 1);
        // Dragging extends from where the press was.
        e.click(0, 0.0, y, Clicks::Single, false);
        e.drag(0, g.x + 0.1, y);
        assert_eq!(e.selected_text(), "Hello b");
    }

    #[test]
    fn caret_and_selection_geometry_follow_the_layout() {
        let mut e = editor(&["abc", "", "def"]);
        let (page, rect) = e.caret_rect().unwrap();
        assert_eq!(page, 0);
        assert!((rect.x - 72.0).abs() < 0.5);
        e.select_all();
        let rects = e.selection_rects();
        assert_eq!(
            rects.len(),
            3,
            "one per line, including the empty paragraph"
        );
        assert!(rects.iter().all(|(_, r)| r.width > 0.0));
        assert_eq!(e.selected_text(), "abc\n\ndef");
    }

    #[test]
    fn toggling_bold_on_a_selection_and_for_new_text() {
        let mut e = editor(&["plain text"]);
        e.select(TextRange::between(at(0, 0), at(0, 5)));
        e.toggle(Toggle::Bold);
        assert!(e.caret_format().bold);
        assert_eq!(e.document().paragraphs[0].runs.len(), 2);
        assert_eq!(
            e.selection(),
            TextRange::between(at(0, 0), at(0, 5)),
            "selection kept"
        );
        e.toggle(Toggle::Bold);
        assert!(!e.caret_format().bold);
        assert_eq!(e.document().paragraphs[0].runs.len(), 1);

        // With only a caret, the format applies to what is typed next.
        e.move_caret(Motion::DocumentEnd, false);
        let revision = e.revision();
        e.toggle(Toggle::Italic);
        assert!(e.revision() > revision, "toolbar sees the change");
        assert!(e.caret_format().italic);
        e.type_text("!");
        let runs = &e.document().paragraphs[0].runs;
        assert_eq!(runs.last().unwrap().text, "!");
        assert_eq!(runs.last().unwrap().format.italic, Some(true));
        e.type_text("?");
        assert_eq!(e.document().paragraphs[0].runs.last().unwrap().text, "!?");
    }

    #[test]
    fn headings_are_bold_until_unbolded() {
        let mut e = editor(&["Title"]);
        e.set_style(ParagraphStyle::Heading1);
        assert!(e.caret_format().bold);
        e.select_all();
        e.toggle(Toggle::Bold);
        assert!(!e.caret_format().bold);
        assert_eq!(e.document().paragraphs[0].runs[0].format.bold, Some(false));
    }

    #[test]
    fn paragraph_formatting_applies_to_every_touched_paragraph() {
        let mut e = editor(&["a", "b", "c"]);
        e.select(TextRange::between(at(0, 1), at(1, 0)));
        e.set_align(Align::Center);
        e.set_line_spacing(1.5);
        let p = &e.document().paragraphs;
        assert_eq!(p[0].format.align, Some(Align::Center));
        assert_eq!(p[1].format.align, Some(Align::Center));
        assert_eq!(p[2].format.align, None);
        assert_eq!(e.caret_format().line_spacing, 1.5);
        e.undo();
        e.undo();
        assert_eq!(e.document().paragraphs[0].format.align, None);
    }

    #[test]
    fn saving_marks_the_state_and_undo_past_it_is_dirty() {
        let mut e = editor(&[""]);
        e.type_text("a");
        e.mark_saved();
        assert!(!e.is_dirty());
        e.type_text("b");
        assert!(e.is_dirty());
        assert_eq!(texts(&e), ["ab"]);
        e.undo();
        assert!(!e.is_dirty(), "typing after a save is a new undo step");
        e.undo();
        assert!(e.is_dirty());
        e.type_text("z");
        e.undo();
        assert!(e.is_dirty(), "the saved state is no longer reachable");
    }

    #[test]
    fn the_journal_replays_to_the_same_document() {
        let mut e = editor(&["start"]);
        let original = e.document().clone();
        e.move_caret(Motion::DocumentEnd, false);
        e.type_text(" more");
        e.enter();
        e.type_text("next");
        e.select_all();
        e.toggle(Toggle::Underline);
        e.undo();
        e.redo();
        e.backspace(false);
        let mut replay = original;
        for edit in e.take_applied() {
            replay.apply(&edit).unwrap();
        }
        assert_eq!(&replay, e.document());
        assert!(e.take_applied().is_empty());
    }

    #[test]
    fn cut_removes_and_returns_the_selection() {
        let mut e = editor(&["one two"]);
        e.select(TextRange::between(at(0, 3), at(0, 7)));
        assert_eq!(e.cut(), " two");
        assert_eq!(texts(&e), ["one"]);
        assert_eq!(e.undo_label(), Some("Cut"));
    }

    #[test]
    fn page_setup_is_undoable() {
        let mut e = editor(&["x"]);
        let mut settings = e.document().settings();
        settings.page = settings.page.landscape();
        e.set_settings(settings);
        assert!(e.document().page.width > e.document().page.height);
        e.undo();
        assert!(e.document().page.width < e.document().page.height);
    }

    #[test]
    fn word_helpers() {
        assert_eq!(word_at("hi there", 4), (3, 8));
        assert_eq!(word_at("hi there", 8), (3, 8));
        assert_eq!(word_at("hi there", 2), (2, 3));
        assert_eq!(word_left("a bc", 4), 2);
        assert_eq!(word_right("a bc", 0), 1);
        assert_eq!(prev_grapheme("ab", 0), 0);
        assert_eq!(next_grapheme("ab", 2), 2);
    }
}
