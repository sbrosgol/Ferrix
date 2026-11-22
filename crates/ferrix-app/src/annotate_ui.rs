//! Markup mode in the app: turning tool choices and pointer input into annotation edits
//! (ADR-0009 phase 2; design in `docs/design-system.md` › Annotations).

use ferrix_annot::Permission;
use ferrix_core::{AnnotEdit, AnnotId, AnnotKind, AnnotOrigin, DocEdit, PageKey, SourceId};
use iced::{Task, widget::text_editor};

use crate::{
    app::{App, Message},
    markup::{self, COLORS, Gesture, Handle, JOIN_STROKES_MS, Markup, Tool},
    markup_canvas::Pointer,
    render::LOGICAL_PX_PER_POINT,
};

/// Messages of Markup mode.
#[derive(Debug, Clone)]
pub enum AnnotMsg {
    /// Turn Markup mode on or off.
    Toggle,
    Tool(Tool),
    Pointer(PageKey, Pointer),
    /// Pointer input on the page at an index, for scripted use (screenshots, tests).
    PointerAt(usize, Pointer),
    /// A text box's font size.
    TextSize(f32),
    /// A palette colour, by index into [`COLORS`].
    Color(usize),
    Width(f32),
    Opacity(u8),
    /// Delete the selection (and its replies).
    Delete,
    /// Select an annotation, e.g. from the Comments list.
    Select(PageKey, AnnotId),
    NoteEdit(text_editor::Action),
    NoteDone,
    NoteDelete,
    /// The answer to "This document is signed": add annotations anyway?
    ConfirmSigned(bool),
    /// Open the comment popover of an annotation (its text and replies).
    OpenComment(PageKey, AnnotId),
    ReplyEdit(text_editor::Action),
    ReplySend,
    /// Delete a reply (and replies to it).
    ReplyDelete(AnnotId),
    /// The "Your name" field shown before the first reply.
    AuthorInput(String),
    AuthorSave,
    /// Choose a standard stamp for the Stamp tool, by name.
    StampPick(String),
    /// Choose a picture for the Stamp tool.
    StampImage,
    /// The chosen picture, or `None` if the dialog was cancelled.
    StampImageLoaded(Option<Result<ferrix_core::StampImage, String>>),
}

/// The note being edited and its text.
#[derive(Debug)]
pub(crate) struct NoteEditor {
    pub(crate) page: PageKey,
    pub(crate) id: AnnotId,
    pub(crate) content: text_editor::Content,
    /// The comment popover with its thread (notes, and any annotation's comment), rather than
    /// a text box's editor on the page.
    pub(crate) popover: bool,
    /// The reply being written.
    pub(crate) reply: text_editor::Content,
}

/// Which list the sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    #[default]
    Pages,
    Outline,
    Comments,
}

impl App {
    pub(crate) fn annot_update(&mut self, msg: AnnotMsg) -> Task<Message> {
        match msg {
            AnnotMsg::Toggle => {
                if self.markup.is_none() && self.doc.as_ref().is_some_and(|d| d.is_read_only()) {
                    self.notice = Some(crate::app::READ_ONLY_NOTICE.into());
                    return Task::none();
                }
                let close = if self.markup.take().is_none()
                    && self.doc.is_some()
                    && self.writer.is_none()
                {
                    self.markup = Some(Markup::default());
                    Task::none()
                } else {
                    self.close_note()
                };
                // The bar takes room from the pages.
                return Task::batch([close, self.rerender_if_fitting()]);
            }
            AnnotMsg::Tool(tool) => {
                if let Some(m) = self.markup.as_mut() {
                    m.tool = tool;
                    m.gesture = None;
                }
            }
            AnnotMsg::Pointer(page, pointer) => return self.pointer(page, pointer),
            AnnotMsg::PointerAt(index, pointer) => {
                if let Some(page) = self.doc.as_ref().and_then(|d| d.key_at(index)) {
                    return self.pointer(page, pointer);
                }
            }
            AnnotMsg::Color(i) => {
                if let Some((_, color)) = COLORS.get(i) {
                    let color = *color;
                    return self.restyle(
                        |s| s.color = color,
                        |a| match &mut a.kind {
                            // A text box's colour is its text's.
                            AnnotKind::FreeText { color: c, .. } => *c = color,
                            _ => a.props.color = Some(color),
                        },
                    );
                }
            }
            AnnotMsg::TextSize(size) => {
                return self.restyle(
                    |_| {},
                    |a| {
                        if let AnnotKind::FreeText { size: s, .. } = &mut a.kind {
                            *s = size;
                            a.props.rect = text_box_rect(a, size);
                        }
                    },
                );
            }
            AnnotMsg::Width(width) => {
                return self.restyle(|s| s.width = width, |a| a.props.border_width = width);
            }
            AnnotMsg::Opacity(percent) => {
                let opacity = f32::from(percent.min(100)) / 100.0;
                return self.restyle(|s| s.opacity = opacity, |a| a.props.opacity = opacity);
            }
            AnnotMsg::Delete => return self.delete_selected(),
            AnnotMsg::Select(page, id) => {
                if self.markup.is_none() {
                    self.markup = Some(Markup::default());
                }
                if let Some(m) = self.markup.as_mut() {
                    m.tool = Tool::Select;
                    m.selected = Some(id);
                }
                let close = self.close_note();
                self.open_note(page, id);
                let index = self.doc.as_ref().and_then(|d| d.state.pages.position(page));
                if let Some(index) = index {
                    return Task::batch([close, self.go_to(index)]);
                }
                return close;
            }
            AnnotMsg::NoteEdit(action) => {
                if let Some(note) = self.note.as_mut() {
                    note.content.perform(action);
                }
            }
            AnnotMsg::NoteDone => return self.close_note(),
            AnnotMsg::NoteDelete => {
                let id = self.note.take().map(|n| n.id);
                if let Some(m) = self.markup.as_mut() {
                    m.selected = id;
                }
                return self.delete_selected();
            }
            AnnotMsg::OpenComment(page, id) => {
                let close = self.close_note();
                self.open_editor(page, id, true);
                return close;
            }
            AnnotMsg::ReplyEdit(action) => {
                if let Some(note) = self.note.as_mut() {
                    note.reply.perform(action);
                }
            }
            AnnotMsg::ReplySend => return self.send_reply(),
            AnnotMsg::ReplyDelete(id) => return self.delete_thread(id, "Delete Reply"),
            AnnotMsg::StampPick(name) => {
                if let Some(m) = self.markup.as_mut() {
                    m.stamp = ferrix_core::StampContent::Standard(name);
                    m.tool = Tool::Stamp;
                    m.selected = None;
                }
            }
            AnnotMsg::StampImage => {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Choose a Picture")
                    .add_filter("Pictures", &["png", "jpg", "jpeg"]);
                return Task::perform(
                    async move {
                        let file = dialog.pick_file().await?;
                        let bytes = file.read().await;
                        Some(
                            ferrix_annot::stamp::image_from_bytes(bytes).map_err(|e| e.to_string()),
                        )
                    },
                    |result| Message::Annot(AnnotMsg::StampImageLoaded(result)),
                );
            }
            AnnotMsg::StampImageLoaded(None) => {}
            AnnotMsg::StampImageLoaded(Some(result)) => match result {
                Ok(image) => {
                    if let Some(m) = self.markup.as_mut() {
                        m.stamp = ferrix_core::StampContent::Image(image);
                        m.tool = Tool::Stamp;
                        m.selected = None;
                    }
                    self.notice = Some("Click the page to place the picture.".into());
                }
                Err(e) => self.error = Some(format!("Couldn’t use that picture: {e}")),
            },
            AnnotMsg::AuthorInput(name) => self.author_input = name,
            AnnotMsg::AuthorSave => {
                let name = self.author_input.trim().to_owned();
                if !name.is_empty() {
                    self.author = Some(name);
                }
            }
            AnnotMsg::ConfirmSigned(ok) => {
                self.signed_prompt = false;
                if let Some(doc) = self.doc.as_mut() {
                    doc.annotate_confirmed = ok;
                }
            }
        }
        Task::none()
    }

    /// Whether annotations may be added on `page` now; explains why not otherwise.
    pub(crate) fn may_annotate(&mut self, page: PageKey) -> bool {
        if !self.can_edit() {
            return false;
        }
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        if doc
            .state
            .pages
            .entries()
            .iter()
            .any(|e| e.key == page && e.source != SourceId::BASE)
        {
            self.notice = Some("Save first to annotate pages inserted from another file.".into());
            return false;
        }
        match doc.annot_policy.as_ref().map(|p| &p.permission) {
            None => {
                self.notice = Some("Reading the document’s annotations…".into());
                false
            }
            Some(Permission::Forbidden(_)) => {
                self.error = Some(CERTIFIED.into());
                false
            }
            Some(Permission::Signed) if !doc.annotate_confirmed => {
                self.signed_prompt = true;
                false
            }
            Some(_) => true,
        }
    }

    /// Logical pixels per point at the current zoom, for pointer tolerances.
    fn markup_scale(&self) -> f32 {
        self.doc
            .as_ref()
            .map_or(1.0, |d| d.strip.zoom * LOGICAL_PX_PER_POINT)
            .max(0.01)
    }

    fn pointer(&mut self, page: PageKey, pointer: Pointer) -> Task<Message> {
        let scale = self.markup_scale();
        let (shift, alt) = (self.modifiers.shift(), self.modifiers.alt());
        // Outside Markup mode the pointer selects text.
        let tool = self.markup.as_ref().map_or(Tool::Select, |m| m.tool);
        let text_gesture = self.text_selection.is_some_and(|s| s.dragging);
        match pointer {
            Pointer::Click(at, clicks) => {
                let close = self.close_note();
                if (tool == Tool::Select || tool.marks_text()) && self.over_text(page, at) {
                    self.text_press(page, at, clicks);
                }
                return close;
            }
            Pointer::Drag(at) if text_gesture => self.text_drag(page, at),
            Pointer::Release(_) if text_gesture => return self.text_release(),
            Pointer::Press(at) => {
                // Clicking anywhere else keeps the open note's text and closes it.
                let close = self.close_note();
                if tool.marks_text() {
                    if !self.over_text(page, at) || !self.text_press(page, at, 1) {
                        self.text_selection = None;
                        self.notice =
                            Some("No selectable text here. The page may be a scan.".into());
                    }
                    return close;
                }
                if tool == Tool::Select {
                    // An annotation on top wins; otherwise the press selects text.
                    let hit = self.markup.is_some() && self.press_select(page, at, scale);
                    if !hit && self.over_text(page, at) {
                        self.text_press(page, at, 1);
                    } else {
                        self.text_selection = None;
                    }
                    return close;
                }
                if !self.may_annotate(page) {
                    return close;
                }
                if matches!(tool, Tool::Note | Tool::TextBox) {
                    return Task::batch([close, self.place_note(page, at, tool)]);
                }
                if tool == Tool::Stamp {
                    return Task::batch([close, self.place_stamp(page, at)]);
                }
                if let Some(m) = self.markup.as_mut() {
                    m.selected = None;
                    m.gesture = Some((
                        page,
                        Gesture::Draw {
                            start: at,
                            current: at,
                            points: vec![at],
                        },
                    ));
                }
                return close;
            }
            Pointer::Drag(at) => {
                if let Some((_, gesture)) = self.markup.as_mut().and_then(|m| m.gesture.as_mut()) {
                    match gesture {
                        Gesture::Draw {
                            current, points, ..
                        } => {
                            *current = at;
                            if points.len() < ferrix_core::MAX_STROKE_POINTS {
                                points.push(at);
                            }
                        }
                        Gesture::Move { current, .. } | Gesture::Resize { current, .. } => {
                            *current = at;
                        }
                    }
                }
            }
            Pointer::Release(at) => {
                let Some((gesture_page, gesture)) =
                    self.markup.as_mut().and_then(|m| m.gesture.take())
                else {
                    return Task::none();
                };
                return self.finish(gesture_page, gesture, at, scale, shift, alt);
            }
        }
        Task::none()
    }

    /// Selects the annotation under `at` (or grabs a handle); whether anything was hit.
    fn press_select(&mut self, page: PageKey, at: ferrix_types::PdfPoint, scale: f32) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return false;
        };
        let list = doc.state.annots.page(page);
        let Some(m) = self.markup.as_mut() else {
            return false;
        };
        // A handle of the current selection wins over whatever lies beneath it.
        if let Some(sel) = m.selected
            && let Some((_, annot)) = list.iter().find(|(id, _)| *id == sel)
            && markup::editable(annot)
        {
            let grab = 8.0 / scale;
            let handle = markup::handles(annot)
                .into_iter()
                .find(|(_, p)| (p.x - at.x).abs() <= grab && (p.y - at.y).abs() <= grab);
            if let Some((handle, _)) = handle {
                m.gesture = Some((
                    page,
                    Gesture::Resize {
                        handle,
                        current: at,
                    },
                ));
                return true;
            }
        }
        m.selected = markup::hit(list, at, 4.0 / scale);
        let selected = m.selected;
        let movable = m
            .selected
            .is_some_and(|id| list.iter().any(|(i, a)| *i == id && markup::editable(a)));
        if movable {
            m.gesture = Some((
                page,
                Gesture::Move {
                    start: at,
                    current: at,
                },
            ));
        }
        if let Some(id) = selected {
            self.open_note(page, id);
        }
        selected.is_some()
    }

    /// Opens what selecting `id` opens: a note's comment popover, or a text box's editor.
    /// Other annotations open their popover from the Comment button.
    fn open_note(&mut self, page: PageKey, id: AnnotId) {
        let kind = self
            .doc
            .as_ref()
            .and_then(|d| d.state.annots.get(id))
            .map(|(_, a)| a.kind.clone());
        match kind {
            Some(AnnotKind::Note { .. }) => self.open_editor(page, id, true),
            Some(AnnotKind::FreeText { .. }) => self.open_editor(page, id, false),
            _ => {}
        }
    }

    fn open_editor(&mut self, page: PageKey, id: AnnotId, popover: bool) {
        let Some((_, annot)) = self.doc.as_ref().and_then(|d| d.state.annots.get(id)) else {
            return;
        };
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.id == id && n.popover == popover)
        {
            return;
        }
        let text = annot.props.contents.clone().unwrap_or_default();
        self.note = Some(NoteEditor {
            page,
            id,
            content: text_editor::Content::with_text(&text),
            popover,
            reply: text_editor::Content::new(),
        });
    }

    /// The session's author name on a new annotation (none until the user gives one).
    pub(crate) fn with_author(
        &self,
        mut annot: ferrix_core::Annotation,
    ) -> ferrix_core::Annotation {
        if annot.props.author.is_none() {
            annot.props.author = self.author.clone();
        }
        annot
    }

    /// Adds the reply being written to the popover's annotation.
    fn send_reply(&mut self) -> Task<Message> {
        let Some(note) = self.note.as_mut() else {
            return Task::none();
        };
        let text = note.reply.text().trim().to_owned();
        if text.is_empty() {
            return Task::none();
        }
        note.reply = text_editor::Content::new();
        let (page, parent_id) = (note.page, note.id);
        if !self.may_annotate(page) {
            return Task::none();
        }
        let Some(parent) = self
            .doc
            .as_ref()
            .and_then(|d| d.state.annots.get(parent_id))
            .map(|(_, a)| a.clone())
        else {
            return Task::none();
        };
        let to = match parent.origin {
            AnnotOrigin::Existing { object, .. } => ferrix_core::ReplyTo::File(object),
            AnnotOrigin::New => ferrix_core::ReplyTo::New(parent_id),
        };
        let mut reply =
            ferrix_core::Annotation::new(AnnotKind::Reply { state: None }, parent.props.rect);
        reply.reply_to = Some(to);
        reply.props.contents = Some(text);
        reply.props.name = Some(ferrix_annot::unique_name());
        // Dated now, so the thread shows "Just now" before the file is saved.
        reply.props.modified = Some(ferrix_annot::pdf_date(std::time::SystemTime::now()));
        // Readers draw replies nowhere (their appearance is empty); keep them out of the way.
        reply.props.flags = ferrix_core::AnnotFlags(
            ferrix_core::AnnotFlags::PRINT
                | ferrix_core::AnnotFlags::NO_ZOOM
                | ferrix_core::AnnotFlags::NO_ROTATE,
        );
        let reply = self.with_author(reply);
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let id = doc.state.annots.new_id();
        let index = doc.state.annots.page(page).len();
        self.apply_annot(
            AnnotEdit::Add {
                page,
                index,
                id,
                annot: Box::new(reply),
            },
            "Reply",
        )
    }

    /// Places the Stamp tool's stamp centred at `at`, selects it, and returns to Select.
    fn place_stamp(&mut self, page: PageKey, at: ferrix_types::PdfPoint) -> Task<Message> {
        let Some(content) = self.markup.as_ref().map(|m| m.stamp.clone()) else {
            return Task::none();
        };
        let annot = self.with_author(markup::stamp(&content, at));
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let id = doc.state.annots.new_id();
        let index = doc.state.annots.page(page).len();
        let task = self.apply_annot(
            AnnotEdit::Add {
                page,
                index,
                id,
                annot: Box::new(annot),
            },
            "Add Stamp",
        );
        if let Some(m) = self.markup.as_mut() {
            m.selected = Some(id);
            m.tool = Tool::Select;
        }
        task
    }

    /// Removes `id` and its replies in one step, replies first so undo restores parents first.
    fn delete_thread(&mut self, id: AnnotId, label: &str) -> Task<Message> {
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let edits: Vec<DocEdit> = doc
            .state
            .annots
            .thread(id)
            .into_iter()
            .rev()
            .map(|id| DocEdit::Annot(AnnotEdit::Remove { id }))
            .collect();
        self.apply_annot(DocEdit::Batch(edits), label)
    }

    /// Places a note or a text box at `at` and opens its editor.
    fn place_note(
        &mut self,
        page: PageKey,
        at: ferrix_types::PdfPoint,
        tool: Tool,
    ) -> Task<Message> {
        let Some(style) = self.markup.as_ref().map(|m| m.style(tool)) else {
            return Task::none();
        };
        let Some(annot) = markup::create(
            tool,
            style,
            &markup::Drag {
                start: at,
                end: at,
                points: &[],
                shift: false,
                alt: false,
                min: 0.0,
            },
        ) else {
            return Task::none();
        };
        let annot = self.with_author(annot);
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let id = doc.state.annots.new_id();
        let index = doc.state.annots.page(page).len();
        let task = self.apply_annot(
            AnnotEdit::Add {
                page,
                index,
                id,
                annot: Box::new(annot),
            },
            if tool == Tool::TextBox {
                "Add Text Box"
            } else {
                "Add Note"
            },
        );
        if let Some(m) = self.markup.as_mut() {
            m.selected = Some(id);
            m.tool = Tool::Select;
        }
        self.note = Some(NoteEditor {
            page,
            id,
            content: text_editor::Content::new(),
            popover: tool == Tool::Note,
            reply: text_editor::Content::new(),
        });
        task
    }

    fn finish(
        &mut self,
        page: PageKey,
        gesture: Gesture,
        at: ferrix_types::PdfPoint,
        scale: f32,
        shift: bool,
        alt: bool,
    ) -> Task<Message> {
        let Some(m) = self.markup.as_ref() else {
            return Task::none();
        };
        let tool = m.tool;
        let selected = m.selected;
        let current = selected.and_then(|id| {
            self.doc
                .as_ref()?
                .state
                .annots
                .get(id)
                .map(|(_, a)| a.clone())
        });
        match gesture {
            Gesture::Draw { start, points, .. } => {
                let style = m.style(tool);
                let Some(annot) = markup::create(
                    tool,
                    style,
                    &markup::Drag {
                        start,
                        end: at,
                        points: &points,
                        shift,
                        alt,
                        min: 4.0 / scale,
                    },
                ) else {
                    self.notice = Some(match tool {
                        Tool::Line | Tool::Arrow => "Drag to draw a line.".into(),
                        Tool::Ellipse => "Drag to draw an ellipse.".into(),
                        _ => "Drag to draw a rectangle.".into(),
                    });
                    return Task::none();
                };
                let annot = self.with_author(annot);
                if tool == Tool::Pen
                    && let Some(task) = self.join_stroke(page, &annot)
                {
                    return task;
                }
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let id = doc.state.annots.new_id();
                let index = doc.state.annots.page(page).len();
                let label = match tool {
                    Tool::Pen => "Add Drawing",
                    Tool::Rectangle => "Add Rectangle",
                    Tool::Ellipse => "Add Ellipse",
                    Tool::Arrow => "Add Arrow",
                    _ => "Add Line",
                };
                let task = self.apply_annot(
                    AnnotEdit::Add {
                        page,
                        index,
                        id,
                        annot: Box::new(annot),
                    },
                    label,
                );
                if let Some(m) = self.markup.as_mut() {
                    if tool == Tool::Pen {
                        m.last_ink = Some((id, std::time::Instant::now()));
                    } else {
                        m.selected = Some(id);
                    }
                    if !tool.sticky() {
                        m.tool = Tool::Select;
                    }
                }
                task
            }
            Gesture::Move { start, .. } => {
                let (Some(id), Some(annot)) = (selected, current) else {
                    return Task::none();
                };
                let (dx, dy) = (at.x - start.x, at.y - start.y);
                if dx == 0.0 && dy == 0.0 {
                    return Task::none();
                }
                self.apply_annot(
                    AnnotEdit::Set {
                        id,
                        annot: Box::new(markup::moved(&annot, dx, dy)),
                    },
                    "Move Annotation",
                )
            }
            Gesture::Resize { handle, .. } => {
                let (Some(id), Some(annot)) = (selected, current) else {
                    return Task::none();
                };
                let label = if matches!(handle, Handle::Box(..)) {
                    "Resize Annotation"
                } else {
                    "Move Line End"
                };
                self.apply_annot(
                    AnnotEdit::Set {
                        id,
                        annot: Box::new(markup::resized(&annot, handle, at, shift)),
                    },
                    label,
                )
            }
        }
    }

    /// Adds a pen stroke to the ink drawn just before it on the same page, if any.
    fn join_stroke(
        &mut self,
        page: PageKey,
        stroke: &ferrix_core::Annotation,
    ) -> Option<Task<Message>> {
        let (id, when) = self.markup.as_ref()?.last_ink?;
        if when.elapsed().as_millis() > JOIN_STROKES_MS {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let (on, previous) = doc.state.annots.get(id)?;
        let (AnnotKind::Ink { strokes }, AnnotKind::Ink { strokes: new }) =
            (&previous.kind, &stroke.kind)
        else {
            return None;
        };
        if on != page || previous.props.color != stroke.props.color {
            return None;
        }
        let mut joined = previous.clone();
        joined.kind = AnnotKind::Ink {
            strokes: strokes.iter().chain(new).cloned().collect(),
        };
        let task = self.apply_annot(
            AnnotEdit::Set {
                id,
                annot: Box::new(joined),
            },
            "Add Drawing",
        );
        if let Some(m) = self.markup.as_mut() {
            m.last_ink = Some((id, std::time::Instant::now()));
        }
        Some(task)
    }

    /// Changes the selection's style if one is selected and editable, else the tool's.
    fn restyle(
        &mut self,
        tool_style: impl Fn(&mut markup::Style),
        annot: impl Fn(&mut ferrix_core::Annotation),
    ) -> Task<Message> {
        let Some(m) = self.markup.as_mut() else {
            return Task::none();
        };
        let selected = m.selected.and_then(|id| {
            let (_, a) = self.doc.as_ref()?.state.annots.get(id)?;
            markup::editable(a).then(|| (id, a.clone()))
        });
        match selected {
            Some((id, mut changed)) => {
                annot(&mut changed);
                self.apply_annot(
                    AnnotEdit::Set {
                        id,
                        annot: Box::new(changed),
                    },
                    "Change Annotation",
                )
            }
            None => {
                let tool = if m.tool == Tool::Select {
                    Tool::Pen
                } else {
                    m.tool
                };
                let mut style = m.style(tool);
                tool_style(&mut style);
                m.set_style(tool, style);
                Task::none()
            }
        }
    }

    pub(crate) fn delete_selected(&mut self) -> Task<Message> {
        let Some(id) = self.markup.as_mut().and_then(|m| m.selected.take()) else {
            return Task::none();
        };
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let had_text = doc.state.annots.get(id).is_some_and(|(_, a)| {
            matches!(a.origin, AnnotOrigin::Existing { .. })
                && a.props.contents.as_deref().is_some_and(|c| !c.is_empty())
        });
        let task = self.delete_thread(id, "Delete Annotation");
        if had_text && self.error.is_none() {
            self.notice = Some(
                "Annotation deleted. Its text remains in earlier versions of the file until it is \
                 optimized."
                    .into(),
            );
        }
        task
    }

    /// Saves the note editor's text (if changed) and closes it.
    pub(crate) fn close_note(&mut self) -> Task<Message> {
        let Some(note) = self.note.take() else {
            return Task::none();
        };
        let text = note.content.text().trim_end().to_owned();
        let Some((_, annot)) = self.doc.as_ref().and_then(|d| d.state.annots.get(note.id)) else {
            return Task::none();
        };
        let mut changed = annot.clone();
        let is_text_box = matches!(annot.kind, AnnotKind::FreeText { .. });
        if is_text_box && text.is_empty() && markup::editable(annot) {
            // An empty text box means nothing: it goes away.
            return self.apply_annot(AnnotEdit::Remove { id: note.id }, "Delete Text Box");
        }
        changed.props.contents = (!text.is_empty()).then_some(text);
        if let AnnotKind::FreeText { size, .. } = changed.kind {
            changed.props.rect = text_box_rect(&changed, size);
        }
        if changed == *annot || !markup::editable(annot) {
            return Task::none();
        }
        self.apply_annot(
            AnnotEdit::Set {
                id: note.id,
                annot: Box::new(changed),
            },
            if is_text_box {
                "Edit Text Box"
            } else {
                "Edit Note"
            },
        )
    }

    pub(crate) fn apply_annot(&mut self, edit: impl Into<DocEdit>, label: &str) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        // The last line of defence: no path may edit an encrypted document (ADR-0003).
        if doc.is_read_only() {
            self.notice = Some(crate::app::READ_ONLY_NOTICE.into());
            return Task::none();
        }
        match doc.edit(edit, label.to_owned()) {
            Ok(()) => {
                self.notice = None;
                let untaggable = doc.annot_policy.as_ref().is_some_and(|p| p.untaggable);
                if untaggable && !doc.tagged_noted {
                    doc.tagged_noted = true;
                    self.notice = Some(
                        "This document’s accessibility tags can’t be updated, so screen readers \
                         won’t announce new annotations."
                            .into(),
                    );
                }
                self.after_edit()
            }
            Err(e) => {
                self.error = Some(format!("Couldn’t change the annotation: {e}"));
                Task::none()
            }
        }
    }
}

/// Shown when a certified document doesn't allow annotations.
pub(crate) const CERTIFIED: &str = "This document is certified, and its author doesn’t allow annotations. You can read it but not annotate it.";

/// A text box's rectangle for its text at `size`: same top-left corner, sized to fit.
fn text_box_rect(annot: &ferrix_core::Annotation, size: f32) -> ferrix_types::PdfBox {
    let r = annot.props.rect;
    let text = annot.props.contents.as_deref().unwrap_or("");
    let (w, h) = ferrix_annot::free_text::measure(text, size, None);
    ferrix_types::PdfBox {
        left: r.left,
        top: r.top,
        right: r.left + w,
        bottom: r.top - h,
    }
}
