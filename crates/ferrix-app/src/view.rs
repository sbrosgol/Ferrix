//! Layout: toolbar, page sidebar, canvas, status bar and dialogs.

use ferrix_types::Rotation;
use iced::{
    Alignment, Element, Font, Length, Theme, font,
    widget::{
        Space, button, column, container, image, mouse_area, opaque, pick_list, row, scrollable,
        stack, svg, text, text_input, tooltip,
    },
};

use ferrix_compose::{Align, ListKind, Toggle};

use crate::{
    app::{
        App, Choice, FindState, MAX_ZOOM, MIN_ZOOM, Message, NewForm, ORIENTATIONS, PAPERS,
        PageSetupForm, Recovery, THUMB_WIDTH, ZOOM_CHOICES, Zoom, ZoomChoice, find_input_id,
        page_scroll_id, sidebar_scroll_id, writer_scroll_id,
    },
    document::{Document, MAX_THUMBNAILS},
    icon::Icon,
    recovery::{Recoverable, describe},
    render::LOGICAL_PX_PER_POINT,
    render::PageImage,
    theme::{self, Tokens},
    writer::{
        Action, COLORS, MARGINS, SIZES, SPACINGS, STYLES, SizeChoice, SpacingChoice, StyleChoice,
        Writer, color_choice,
    },
    writer_canvas::Canvas,
};

const TOOLBAR_HEIGHT: f32 = 46.0;
const STATUS_HEIGHT: f32 = 26.0;
const SIDEBAR_WIDTH: f32 = 188.0;
const PAGE_MARGIN: f32 = 28.0;
const SCROLLBAR_ALLOWANCE: f32 = 12.0;
/// Space for the macOS traffic lights, which sit over our toolbar.
const TRAFFIC_LIGHTS: f32 = if cfg!(target_os = "macos") { 72.0 } else { 0.0 };

const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

/// Logical size available to a page inside the canvas.
pub(crate) fn canvas_size(window: iced::Size, sidebar: bool) -> iced::Size {
    let sidebar = if sidebar { SIDEBAR_WIDTH + 1.0 } else { 0.0 };
    iced::Size::new(
        (window.width - sidebar - 2.0 * PAGE_MARGIN - SCROLLBAR_ALLOWANCE).max(64.0),
        (window.height - TOOLBAR_HEIGHT - STATUS_HEIGHT - 2.0 - 2.0 * PAGE_MARGIN).max(64.0),
    )
}

impl App {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        let main: Element<'_, Message> = match (&self.doc, &self.page_image) {
            _ if self.writer.is_some() => self.writer_view(),
            (None, _) => self.empty_state(),
            (Some(_), Some(page)) => self.page_view(page),
            (Some(_), None) => container(muted("Loading…", 13.0))
                .center(Length::Fill)
                .style(theme::canvas)
                .into(),
        };

        let body: Element<'_, Message> = match &self.doc {
            Some(doc) if self.sidebar => {
                row![self.sidebar_view(doc), vertical_divider(), main].into()
            }
            _ => main,
        };

        let mut window = column![self.toolbar(), horizontal_divider()];
        if let Some(writer) = &self.writer {
            window = window
                .push(self.format_bar(writer))
                .push(horizontal_divider());
            if let Some(find) = &self.find {
                window = window
                    .push(self.find_bar(writer, find))
                    .push(horizontal_divider());
            }
        }
        let window = window
            .push(container(body).height(Length::Fill))
            .extend([horizontal_divider(), self.status_bar()]);
        if self.confirm.is_some() {
            let name = match (&self.writer, &self.doc) {
                (Some(w), _) => Some(w.name.clone()),
                (None, Some(doc)) => Some(doc.name()),
                (None, None) => None,
            };
            if let Some(name) = name {
                return stack![window, save_changes_dialog(name)].into();
            }
        }
        if let Some(form) = &self.page_setup {
            return stack![window, page_setup_dialog(form)].into();
        }
        if let Some(found) = self.recoveries.first() {
            return stack![window, self.recovery_dialog(found)].into();
        }
        match &self.new_form {
            Some(form) => stack![window, self.new_document_dialog(form)].into(),
            None => window.into(),
        }
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let doc = self.doc.as_ref();
        let writer = self.writer.as_ref();
        let has_doc = doc.is_some();
        let open = has_doc || writer.is_some();
        let (page, count) = doc.map_or((0, 0), |d| (d.current, d.len()));
        let editable = self.can_edit();
        let dirty =
            doc.is_some_and(Document::is_dirty) || writer.is_some_and(|w| w.editor.is_dirty());
        let can_save = dirty || writer.is_some_and(|w| w.path.is_none());
        let name = writer
            .map(|w| w.name.clone())
            .or_else(|| doc.map(Document::name));
        let (undo_label, redo_label) = match writer {
            Some(w) => (
                w.editor.undo_label().map(str::to_owned),
                w.editor.redo_label().map(str::to_owned),
            ),
            None => (
                doc.and_then(|d| d.history.undo_label()).map(str::to_owned),
                doc.and_then(|d| d.history.redo_label()).map(str::to_owned),
            ),
        };

        let title: Element<'_, Message> = match name {
            Some(name) => row![
                text(name)
                    .size(13)
                    .font(SEMIBOLD)
                    .wrapping(text::Wrapping::None),
                muted(if dirty { "Edited" } else { "" }, 12.0),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
            None => text("Ferrix").size(13).font(SEMIBOLD).into(),
        };
        let undo_tip = undo_label
            .as_deref()
            .map_or_else(|| "Undo (⌘Z)".to_owned(), |l| format!("Undo {l} (⌘Z)"));
        let redo_tip = redo_label
            .as_deref()
            .map_or_else(|| "Redo (⇧⌘Z)".to_owned(), |l| format!("Redo {l} (⇧⌘Z)"));
        let left = row![
            Space::new().width(TRAFFIC_LIGHTS),
            icon_button(
                Icon::PanelLeft,
                "Toggle sidebar (⌘\\)",
                has_doc.then_some(Message::ToggleSidebar),
                self.sidebar && has_doc,
            ),
            icon_button(
                Icon::New,
                "New document… (⌘N)",
                Some(Message::NewDialog),
                false
            ),
            icon_button(
                Icon::FolderOpen,
                "Open… (⌘O)",
                Some(Message::OpenDialog),
                false
            ),
            icon_button(
                Icon::Save,
                "Save (⌘S)",
                (can_save && editable).then_some(Message::Save),
                false,
            ),
            icon_button(
                Icon::Printer,
                "Print… (⌘P)",
                (open && self.busy.is_none()).then_some(Message::Print),
                false,
            ),
            toolbar_separator(),
            icon_button(
                Icon::Undo,
                undo_tip,
                (editable && undo_label.is_some()).then_some(Message::Undo),
                false,
            ),
            icon_button(
                Icon::Redo,
                redo_tip,
                (editable && redo_label.is_some()).then_some(Message::Redo),
                false,
            ),
            Space::new().width(8),
            title,
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let page_field = text_input("", if has_doc { &self.page_input } else { "" })
            .on_input_maybe(has_doc.then_some(Message::PageInput))
            .on_submit(Message::PageSubmit)
            .size(13)
            .width(46)
            .padding([4, 6])
            .align_x(Alignment::Center)
            .style(theme::input);
        // A written document's page number is in the status bar; the toolbar keeps its room for
        // the title.
        let center = if writer.is_some() {
            row![]
        } else {
            row![
                icon_button(
                    Icon::ChevronLeft,
                    "Previous page",
                    (has_doc && page > 0).then_some(Message::Previous),
                    false,
                ),
                page_field,
                muted(
                    if has_doc {
                        format!("of {count}")
                    } else {
                        "of –".into()
                    },
                    13.0
                ),
                icon_button(
                    Icon::ChevronRight,
                    "Next page",
                    (has_doc && page + 1 < count).then_some(Message::Next),
                    false,
                ),
            ]
        }
        .spacing(6)
        .align_y(Alignment::Center);

        let zoom = match writer {
            Some(w) => Some(w.zoom / LOGICAL_PX_PER_POINT),
            None => doc.and_then(|d| d.shown).map(|(_, z)| z),
        };
        let zoom_label = zoom.map(|z| ZoomChoice::Percent((z * 100.0).round() as u16));
        let theme_icon = if self.is_dark() {
            Icon::Sun
        } else {
            Icon::Moon
        };
        let right = row![
            icon_button(
                Icon::ZoomOut,
                "Zoom out (⌘−)",
                zoom.filter(|z| *z > MIN_ZOOM + 0.001)
                    .map(|_| Message::ZoomOut),
                false,
            ),
            pick_list(ZOOM_CHOICES, zoom_label, Message::ZoomPicked)
                .placeholder("–")
                .text_size(13)
                .padding([4, 8])
                .width(92)
                .style(theme::select)
                .menu_style(theme::select_menu),
            icon_button(
                Icon::ZoomIn,
                "Zoom in (⌘+)",
                zoom.filter(|z| *z < MAX_ZOOM - 0.001)
                    .map(|_| Message::ZoomIn),
                false,
            ),
            toolbar_separator(),
            icon_button(
                Icon::FitWidth,
                "Fit width (⌘2)",
                open.then_some(Message::FitWidth),
                open && self.zoom == Zoom::FitWidth,
            ),
            icon_button(
                Icon::FitPage,
                "Fit page (⌘1)",
                open.then_some(Message::FitPage),
                open && self.zoom == Zoom::FitPage,
            ),
            toolbar_separator(),
            icon_button(
                theme_icon,
                "Toggle light/dark",
                Some(Message::ToggleTheme),
                false
            ),
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let bar = container(
            row![
                container(left).width(Length::Fill),
                center,
                container(right).width(Length::Fill).align_x(Alignment::End),
            ]
            .align_y(Alignment::Center),
        )
        .height(TOOLBAR_HEIGHT)
        .padding([0, 8])
        .center_y(TOOLBAR_HEIGHT)
        .style(theme::chrome);

        // With the macOS title bar merged into the toolbar, the toolbar must move the window.
        if cfg!(target_os = "macos") {
            mouse_area(bar).on_press(Message::DragWindow).into()
        } else {
            bar.into()
        }
    }

    fn sidebar_view<'a>(&'a self, doc: &'a Document) -> Element<'a, Message> {
        let editable = self.can_edit();
        let targets = doc.targets();
        let positions: Vec<usize> = targets
            .iter()
            .filter_map(|k| doc.pages.position(*k))
            .collect();
        let can_move_up = positions.first().is_some_and(|&p| p > 0);
        let can_move_down = positions.last().is_some_and(|&p| p + 1 < doc.len());
        let can_delete = targets.len() < doc.len();
        let enabled = |ok: bool, message: Message| (editable && ok).then_some(message);

        let actions = row![
            icon_button(
                Icon::RotateLeft,
                "Rotate left (⌘L)",
                enabled(true, Message::Rotate(Rotation::R270)),
                false,
            ),
            icon_button(
                Icon::RotateRight,
                "Rotate right (⌘R)",
                enabled(true, Message::Rotate(Rotation::R90)),
                false,
            ),
            icon_button(
                Icon::MoveUp,
                "Move earlier (⌥↑)",
                enabled(can_move_up, Message::Move { earlier: true }),
                false,
            ),
            icon_button(
                Icon::MoveDown,
                "Move later (⌥↓)",
                enabled(can_move_down, Message::Move { earlier: false }),
                false,
            ),
            icon_button(
                Icon::Delete,
                "Delete (⌫)",
                enabled(can_delete, Message::Delete),
                false,
            ),
            icon_button(
                Icon::InsertPages,
                "Insert pages from a PDF, document or image…",
                enabled(true, Message::InsertDialog),
                false,
            ),
        ]
        .spacing(0);
        let header = container(actions)
            .width(Length::Fill)
            .padding([6, 4])
            .center_x(Length::Fill);

        let thumbs = doc
            .pages
            .entries()
            .iter()
            .take(MAX_THUMBNAILS)
            .enumerate()
            .map(|(i, entry)| {
                let selected = doc.selection.contains(&entry.key);
                let current = i == doc.current;
                let paper: Element<'_, Message> = match doc.thumbnails.get(&entry.key) {
                    Some(thumb) => tiles(&thumb.image),
                    None => Space::new()
                        .width(THUMB_WIDTH)
                        .height(THUMB_WIDTH * 1.3)
                        .into(),
                };
                let label = text((i + 1).to_string())
                    .size(12)
                    .font(if current { SEMIBOLD } else { Font::DEFAULT })
                    .style(move |theme: &Theme| {
                        let t = theme::tokens(theme);
                        text::Style {
                            color: Some(if selected { t.accent } else { t.text_muted }),
                        }
                    });
                let ring = container(container(paper).style(theme::page))
                    .padding(4)
                    .style(move |theme| theme::thumbnail_ring(theme, selected));
                button(column![ring, label].spacing(6).align_x(Alignment::Center))
                    .on_press(Message::ThumbnailClicked(entry.key))
                    .padding(0)
                    .style(theme::plain)
                    .into()
            });
        let list = column(thumbs)
            .spacing(14)
            .padding([12, 0])
            .width(Length::Fill)
            .align_x(Alignment::Center);

        container(column![
            header,
            horizontal_divider(),
            scrollable(list)
                .direction(scrollable::Direction::Vertical(thin_scrollbar()))
                .id(sidebar_scroll_id())
                .height(Length::Fill)
                .style(theme::scrollbar),
        ])
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .style(theme::chrome)
        .into()
    }

    fn page_view<'a>(&self, page: &'a PageImage) -> Element<'a, Message> {
        let paper = container(tiles(page)).style(theme::page);
        let canvas = self.canvas_size();
        let fits_x = page.logical.width <= canvas.width + SCROLLBAR_ALLOWANCE;
        let fits_y = page.logical.height <= canvas.height;

        let view: Element<'a, Message> = if fits_x && fits_y {
            container(paper).center(Length::Fill).into()
        } else {
            let content = container(paper).padding(PAGE_MARGIN);
            let scroll = if fits_x {
                scrollable(content.center_x(Length::Fill))
                    .direction(scrollable::Direction::Vertical(thin_scrollbar()))
            } else {
                scrollable(content).direction(scrollable::Direction::Both {
                    vertical: thin_scrollbar(),
                    horizontal: thin_scrollbar(),
                })
            };
            scroll
                .id(page_scroll_id())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::scrollbar)
                .into()
        };
        container(view).style(theme::canvas).into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let failed = self.engine.is_err();
        let (icon, heading, body) = if failed {
            (
                Icon::Alert,
                "The PDF engine couldn’t start",
                "Ferrix needs the PDFium library. In a development checkout, run \
                 `cargo xtask fetch-pdfium`.",
            )
        } else {
            (
                Icon::FileText,
                "No document open",
                "Open a PDF, Word document or image, drop one anywhere in this window, \
                 or start a new document.",
            )
        };
        let badge = container(
            svg(icon.handle())
                .width(34)
                .height(34)
                .style(theme::icon_colored(if failed {
                    |t: &Tokens| t.danger
                } else {
                    |t: &Tokens| t.accent
                })),
        )
        .padding(21)
        .style(theme::badge);

        let open = button(
            row![
                svg(Icon::FolderOpen.handle())
                    .width(16)
                    .height(16)
                    .style(theme::icon_colored(|t| t.on_accent)),
                text("Open…").size(14).font(SEMIBOLD),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([9, 18])
        .style(theme::primary)
        .on_press_maybe((!failed).then_some(Message::OpenDialog));
        let new = button(
            row![
                svg(Icon::New.handle())
                    .width(16)
                    .height(16)
                    .style(theme::icon_colored(|t| t.text)),
                text("New Document…").size(14),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([9, 18])
        .style(theme::secondary)
        .on_press_maybe((!failed).then_some(Message::NewDialog));

        let shortcut_hint = row![
            kbd(if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Ctrl"
            }),
            kbd("O"),
        ]
        .spacing(4);

        let content = column![
            badge,
            Space::new().height(4),
            text(heading).size(20).font(SEMIBOLD),
            container(muted(body, 14.0).align_x(Alignment::Center)).max_width(380),
            Space::new().height(6),
            row![open, new].spacing(10),
            shortcut_hint,
        ]
        .spacing(10)
        .align_x(Alignment::Center);

        container(content)
            .center(Length::Fill)
            .style(theme::canvas)
            .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let left: Element<'_, Message> = if let Some(busy) = self.busy {
            muted(busy, 12.0).into()
        } else if let Some(error) = &self.error {
            dismissible(
                row![
                    svg(Icon::Alert.handle())
                        .width(14)
                        .height(14)
                        .style(theme::icon_colored(|t| t.danger)),
                    text(error).size(12).style(|theme: &Theme| text::Style {
                        color: Some(theme::tokens(theme).danger),
                    }),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            )
        } else if let Some(notice) = &self.notice {
            dismissible(muted(notice, 12.0).into())
        } else if let Some(w) = &self.writer {
            let words = w.editor.word_count();
            muted(
                format!("{words} word{}", if words == 1 { "" } else { "s" }),
                12.0,
            )
            .into()
        } else if let Some(doc) = &self.doc {
            let count = doc.len();
            let mut parts = vec![format!("{count} page{}", if count == 1 { "" } else { "s" })];
            if let Some(version) = &doc.info.pdf_version {
                parts.push(format!("PDF {version}"));
            }
            if let Some((size, _)) = doc.shown {
                parts.push(format!("{:.0} × {:.0} pt", size.width, size.height));
            }
            if doc.selection.len() > 1 {
                parts.push(format!("{} selected", doc.selection.len()));
            }
            muted(parts.join("  ·  "), 12.0).into()
        } else {
            muted("Ready", 12.0).into()
        };
        let right = match (&self.writer, &self.doc) {
            (Some(w), _) => format!("Page {} of {}", w.current_page() + 1, w.page_count()),
            (None, Some(doc)) => format!("Page {} of {}", doc.current + 1, doc.len()),
            (None, None) => String::new(),
        };
        container(
            row![container(left).width(Length::Fill), muted(right, 12.0)]
                .align_y(Alignment::Center),
        )
        .height(STATUS_HEIGHT)
        .padding([0, 12])
        .center_y(STATUS_HEIGHT)
        .style(theme::chrome)
        .into()
    }
}

impl App {
    /// Offers back unsaved work from a session that ended unexpectedly.
    fn recovery_dialog<'a>(&'a self, found: &'a Recoverable) -> Element<'a, Message> {
        let busy = self.busy.is_some();
        let mut body = column![
            text("Recover unsaved changes?").size(16).font(SEMIBOLD),
            muted(
                format!(
                    "Ferrix closed unexpectedly while “{}” had unsaved changes.",
                    found.name()
                ),
                13.0
            ),
            muted(describe(found), 12.0),
        ]
        .spacing(6);
        if found.base_changed {
            body = body.push(
                row![
                    svg(Icon::Alert.handle())
                        .width(14)
                        .height(14)
                        .style(theme::icon_colored(|t| t.danger)),
                    text(
                        "The original file has changed since then, so these edits can’t be \
                         applied to it."
                    )
                    .size(12)
                    .style(|theme: &Theme| text::Style {
                        color: Some(theme::tokens(theme).danger),
                    }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
        if self.recoveries.len() > 1 {
            body = body.push(muted(
                format!(
                    "{} more documents to review after this one.",
                    self.recoveries.len() - 1
                ),
                12.0,
            ));
        }
        let card = column![
            body,
            Space::new().height(8),
            row![
                button(text("Discard").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press_maybe((!busy).then_some(Message::Recovery(Recovery::Discard))),
                Space::new().width(Length::Fill),
                button(text("Later").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press_maybe((!busy).then_some(Message::Recovery(Recovery::Later))),
                button(text("Recover").size(13).font(SEMIBOLD))
                    .padding([6, 18])
                    .style(theme::primary)
                    .on_press_maybe(
                        (!busy && !found.base_changed)
                            .then_some(Message::Recovery(Recovery::Recover)),
                    ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .padding(20)
        .width(460);
        opaque(
            container(container(card).style(theme::card))
                .center(Length::Fill)
                .style(theme::backdrop),
        )
    }

    /// New Document: blank pages, or a conversion of other files.
    fn new_document_dialog<'a>(&'a self, form: &'a NewForm) -> Element<'a, Message> {
        let field_label = |label: &'a str| muted(label, 12.0);
        let write = column![
            text("Write a document").size(13).font(SEMIBOLD),
            row![
                muted(
                    "A letter, report or form you type and format here, saved as a PDF you can \
                     keep editing in Ferrix. Uses the paper size and orientation below.",
                    12.0
                )
                .width(Length::Fill),
                button(text("Start Writing").size(13).font(SEMIBOLD))
                    .padding([6, 16])
                    .style(theme::primary)
                    .on_press(Message::CreateWriting),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(8);
        let blank = column![
            text("Blank pages").size(13).font(SEMIBOLD),
            row![
                column![
                    field_label("Paper size"),
                    pick_list(PAPERS, Some(form.paper), Message::NewPaper)
                        .text_size(13)
                        .padding([5, 8])
                        .width(110)
                        .style(theme::select)
                        .menu_style(theme::select_menu),
                ]
                .spacing(4),
                column![
                    field_label("Orientation"),
                    pick_list(
                        ORIENTATIONS,
                        Some(form.orientation),
                        Message::NewOrientation
                    )
                    .text_size(13)
                    .padding([5, 8])
                    .width(120)
                    .style(theme::select)
                    .menu_style(theme::select_menu),
                ]
                .spacing(4),
                column![
                    field_label("Pages"),
                    text_input("1", &form.pages)
                        .on_input(Message::NewPages)
                        .on_submit(Message::CreateBlank)
                        .size(13)
                        .padding([5, 8])
                        .width(64)
                        .style(theme::input),
                ]
                .spacing(4),
                Space::new().width(Length::Fill),
                button(text("Create").size(13))
                    .padding([6, 16])
                    .style(theme::secondary)
                    .on_press(Message::CreateBlank),
            ]
            .spacing(12)
            .align_y(Alignment::End),
        ]
        .spacing(10);

        let tools = self
            .converter
            .as_ref()
            .map(|c| c.tools())
            .unwrap_or_default();
        let tools_note = if tools.is_empty() {
            "Images convert directly. To convert Word, Excel or PowerPoint files, install \
             LibreOffice (libreoffice.org)."
                .to_owned()
        } else {
            format!(
                "Office documents are converted with {}.",
                tools.join(" or ")
            )
        };
        let convert = column![
            text("From other files").size(13).font(SEMIBOLD),
            row![
                muted(
                    "Turn a Word document, spreadsheet or presentation into a PDF, or \
                     combine images into one PDF.",
                    12.0
                )
                .width(Length::Fill),
                button(text("Choose Files…").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press(Message::CreateFromFiles),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            muted(tools_note, 11.0),
        ]
        .spacing(8);

        let card = column![
            row![
                text("New Document").size(16).font(SEMIBOLD),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press(Message::CloseNewDialog),
            ]
            .align_y(Alignment::Center),
            horizontal_divider(),
            write,
            horizontal_divider(),
            blank,
            horizontal_divider(),
            convert,
        ]
        .spacing(14)
        .padding(20)
        .width(540);
        opaque(
            container(container(card).style(theme::card))
                .center(Length::Fill)
                .style(theme::backdrop),
        )
    }
}

impl App {
    /// The pages of the document being written, with caret and selection.
    fn writer_view(&self) -> Element<'_, Message> {
        let Some(w) = &self.writer else {
            return Space::new().into();
        };
        let page = w.page_size();
        let available = self.window_size.width - SCROLLBAR_ALLOWANCE;
        let wide = page.width + 2.0 * crate::writer::PAGE_GAP > available;
        let images = w.images();
        let sheets = column(images.iter().map(|image| {
            let content: Element<'_, Message> = match image {
                Some(image) => tiles(image),
                None => Space::new().width(page.width).height(page.height).into(),
            };
            container(content)
                .width(page.width)
                .height(page.height)
                .style(theme::page)
                .into()
        }))
        .spacing(crate::writer::PAGE_GAP)
        .padding(crate::writer::PAGE_GAP)
        .align_x(Alignment::Center)
        .width(if wide {
            Length::Fixed(page.width + 2.0 * crate::writer::PAGE_GAP)
        } else {
            Length::Fill
        });
        let matches = self
            .find
            .as_ref()
            .map(|f| {
                w.editor
                    .find_all(&f.query, f.match_case)
                    .into_iter()
                    .flat_map(|range| w.editor.range_rects(range))
                    .collect()
            })
            .unwrap_or_default();
        let canvas = Canvas::new(page, images.len(), w.zoom, Message::Write)
            .selection(w.editor.selection_rects())
            .caret(w.editor.caret_rect())
            .matches(matches)
            .focused(self.can_edit() && !self.find.as_ref().is_some_and(|f| f.focused))
            .wide(wide);
        let direction = if wide {
            scrollable::Direction::Both {
                vertical: thin_scrollbar(),
                horizontal: thin_scrollbar(),
            }
        } else {
            scrollable::Direction::Vertical(thin_scrollbar())
        };
        container(
            scrollable(stack![sheets, canvas])
                .direction(direction)
                .id(writer_scroll_id())
                .on_scroll(|viewport| {
                    Message::Write(Action::Scrolled(
                        viewport.absolute_offset().y,
                        viewport.bounds().height,
                    ))
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::scrollbar),
        )
        .style(theme::canvas)
        .into()
    }

    /// Paragraph and character formatting for the writer.
    fn format_bar<'a>(&'a self, w: &'a Writer) -> Element<'a, Message> {
        let format = w.editor.caret_format();
        let on = self.can_edit();
        let act = move |action: Action| on.then_some(Message::Write(action));
        let families = w.editor.fonts().families();
        let size_choice = SizeChoice(format.size);
        let spacing = SPACINGS
            .iter()
            .copied()
            .find(|s| (s.0 - format.line_spacing).abs() < 0.01)
            .unwrap_or(SpacingChoice(format.line_spacing));

        let style = pick_list(STYLES, Some(StyleChoice(format.style)), |c| {
            Message::Write(Action::Style(c))
        })
        .text_size(13)
        .padding([4, 8])
        .width(120)
        .style(theme::select)
        .menu_style(theme::select_menu);
        let family = pick_list(families, Some(format.family.clone()), |f| {
            Message::Write(Action::Family(f))
        })
        .text_size(13)
        .padding([4, 8])
        .width(170)
        .style(theme::select)
        .menu_style(theme::select_menu);
        let size = pick_list(SIZES, Some(size_choice), |c| {
            Message::Write(Action::Size(c))
        })
        .text_size(13)
        .padding([4, 8])
        .width(64)
        .style(theme::select)
        .menu_style(theme::select_menu);
        let color = pick_list(COLORS, Some(color_choice(format.color)), |c| {
            Message::Write(Action::Color(c))
        })
        .text_size(13)
        .padding([4, 8])
        .width(104)
        .style(theme::select)
        .menu_style(theme::select_menu);
        let line_spacing = pick_list(SPACINGS, Some(spacing), |c| {
            Message::Write(Action::Spacing(c))
        })
        .text_size(13)
        .padding([4, 8])
        .width(84)
        .style(theme::select)
        .menu_style(theme::select_menu);

        let toggle = |icon, tip, t, active| icon_button(icon, tip, act(Action::Toggle(t)), active);
        let align =
            |icon, tip, a: Align| icon_button(icon, tip, act(Action::Align(a)), format.align == a);
        let bar = row![
            style,
            family,
            size,
            toolbar_separator(),
            toggle(Icon::Bold, "Bold (⌘B)", Toggle::Bold, format.bold),
            toggle(Icon::Italic, "Italic (⌘I)", Toggle::Italic, format.italic),
            toggle(
                Icon::Underline,
                "Underline (⌘U)",
                Toggle::Underline,
                format.underline
            ),
            toggle(
                Icon::Strikethrough,
                "Strikethrough",
                Toggle::Strikethrough,
                format.strikethrough
            ),
            color,
            toolbar_separator(),
            align(Icon::AlignLeft, "Align left", Align::Left),
            align(Icon::AlignCenter, "Centre", Align::Center),
            align(Icon::AlignRight, "Align right", Align::Right),
            align(Icon::AlignJustify, "Justify", Align::Justify),
            line_spacing,
            toolbar_separator(),
            icon_button(
                Icon::BulletList,
                "Bulleted list",
                act(Action::List(ListKind::Bullet)),
                format.list == Some(ListKind::Bullet),
            ),
            icon_button(
                Icon::NumberedList,
                "Numbered list",
                act(Action::List(ListKind::Numbered)),
                format.list == Some(ListKind::Numbered),
            ),
            icon_button(
                Icon::Picture,
                "Insert picture…",
                act(Action::InsertImage),
                false
            ),
            icon_button(
                Icon::PageBreak,
                "Page break (⌘↩)",
                act(Action::PageBreak),
                false
            ),
            toolbar_separator(),
            icon_button(
                Icon::ClearFormatting,
                "Clear formatting",
                act(Action::ClearFormatting),
                false
            ),
            icon_button(
                Icon::PageSetup,
                "Page setup…",
                on.then_some(Message::PageSetupDialog),
                false
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        container(scrollable(bar).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(0).scroller_width(0),
        )))
        .height(42)
        .padding([0, 10])
        .center_y(42)
        .width(Length::Fill)
        .style(theme::chrome)
        .into()
    }
}

impl App {
    /// Find and replace, under the formatting bar.
    fn find_bar<'a>(&'a self, w: &'a Writer, find: &'a FindState) -> Element<'a, Message> {
        let count = w.editor.find_all(&find.query, find.match_case).len();
        let any = count > 0;
        let status = if find.query.is_empty() {
            String::new()
        } else {
            match count {
                0 => "No matches".to_owned(),
                1 => "1 match".to_owned(),
                n => format!("{n} matches"),
            }
        };
        let bar = row![
            svg(Icon::Search.handle())
                .width(16)
                .height(16)
                .style(theme::icon_colored(|t| t.text_muted)),
            text_input("Find", &find.query)
                .id(find_input_id())
                .on_input(Message::FindQuery)
                .on_submit(Message::FindNext)
                .size(13)
                .padding([4, 8])
                .width(220)
                .style(theme::input),
            icon_button(
                Icon::MatchCase,
                "Match case",
                Some(Message::FindCase(!find.match_case)),
                find.match_case,
            ),
            icon_button(
                Icon::MoveUp,
                "Previous match",
                any.then_some(Message::FindPrevious),
                false
            ),
            icon_button(
                Icon::MoveDown,
                "Next match (↩)",
                any.then_some(Message::FindNext),
                false
            ),
            muted(status, 12.0).width(90),
            text_input("Replace with", &find.replacement)
                .on_input(Message::FindReplacement)
                .on_submit(Message::ReplaceOne)
                .size(13)
                .padding([4, 8])
                .width(200)
                .style(theme::input),
            button(text("Replace").size(13))
                .padding([4, 12])
                .style(theme::secondary)
                .on_press_maybe(any.then_some(Message::ReplaceOne)),
            button(text("Replace All").size(13))
                .padding([4, 12])
                .style(theme::secondary)
                .on_press_maybe(any.then_some(Message::ReplaceAll)),
            Space::new().width(Length::Fill),
            icon_button(Icon::Close, "Close (Esc)", Some(Message::FindClose), false),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        container(bar)
            .height(40)
            .padding([0, 10])
            .center_y(40)
            .width(Length::Fill)
            .style(theme::chrome)
            .into()
    }
}

/// Paper, orientation, margins and title of a written document.
fn page_setup_dialog(form: &PageSetupForm) -> Element<'_, Message> {
    let label = |l: &'static str| muted(l, 12.0);
    let fields = row![
        column![
            label("Paper size"),
            pick_list(PAPERS, Some(form.paper), Message::PageSetupPaper)
                .text_size(13)
                .padding([5, 8])
                .width(110)
                .style(theme::select)
                .menu_style(theme::select_menu),
        ]
        .spacing(4),
        column![
            label("Orientation"),
            pick_list(
                ORIENTATIONS,
                Some(form.orientation),
                Message::PageSetupOrientation
            )
            .text_size(13)
            .padding([5, 8])
            .width(120)
            .style(theme::select)
            .menu_style(theme::select_menu),
        ]
        .spacing(4),
        column![
            label("Margins"),
            pick_list(MARGINS, Some(form.margins), Message::PageSetupMargins)
                .text_size(13)
                .padding([5, 8])
                .width(140)
                .style(theme::select)
                .menu_style(theme::select_menu),
        ]
        .spacing(4),
    ]
    .spacing(12);
    let title = column![
        label("Document title (shown by PDF readers)"),
        text_input("Untitled", &form.title)
            .on_input(Message::PageSetupTitle)
            .on_submit(Message::ApplyPageSetup)
            .size(13)
            .padding([5, 8])
            .style(theme::input),
    ]
    .spacing(4);
    let page_text = column![
        label("Header and footer: {page} and {pages} become page numbers"),
        row![
            text_input("Header", &form.header)
                .on_input(Message::PageSetupHeader)
                .size(13)
                .padding([5, 8])
                .style(theme::input),
            text_input("Footer, e.g. Page {page} of {pages}", &form.footer)
                .on_input(Message::PageSetupFooter)
                .size(13)
                .padding([5, 8])
                .style(theme::input),
        ]
        .spacing(8),
        iced::widget::checkbox(form.skip_first)
            .label("Not on the first page")
            .on_toggle(Message::PageSetupSkipFirst)
            .text_size(13)
            .size(16),
    ]
    .spacing(6);
    let archival = column![
        iced::widget::checkbox(form.archival)
            .label("Save as PDF/A-3 for long-term archiving")
            .on_toggle(Message::PageSetupArchival)
            .text_size(13)
            .size(16),
        muted(
            "For records that must stay readable for decades. The file is dated and checked \
             against the standard when saved.",
            11.0
        ),
    ]
    .spacing(4);
    let card = column![
        text("Page Setup").size(16).font(SEMIBOLD),
        fields,
        title,
        page_text,
        archival,
        Space::new().height(4),
        row![
            Space::new().width(Length::Fill),
            button(text("Cancel").size(13))
                .padding([6, 14])
                .style(theme::secondary)
                .on_press(Message::Escape),
            button(text("Apply").size(13).font(SEMIBOLD))
                .padding([6, 18])
                .style(theme::primary)
                .on_press(Message::ApplyPageSetup),
        ]
        .spacing(8),
    ]
    .spacing(14)
    .padding(20)
    .width(540);
    opaque(
        container(container(card).style(theme::card))
            .center(Length::Fill)
            .style(theme::backdrop),
    )
}

/// "Save changes?" before closing or replacing an edited document.
fn save_changes_dialog<'a>(name: String) -> Element<'a, Message> {
    let card = column![
        text(format!("Save changes to “{name}”?"))
            .size(15)
            .font(SEMIBOLD),
        muted("Your changes will be lost if you don’t save them.", 13.0),
        Space::new().height(8),
        row![
            button(text("Don’t Save").size(13))
                .padding([6, 14])
                .style(theme::secondary)
                .on_press(Message::Confirmed(Choice::Discard)),
            Space::new().width(Length::Fill),
            button(text("Cancel").size(13))
                .padding([6, 14])
                .style(theme::secondary)
                .on_press(Message::Confirmed(Choice::Cancel)),
            button(text("Save").size(13).font(SEMIBOLD))
                .padding([6, 18])
                .style(theme::primary)
                .on_press(Message::Confirmed(Choice::Save)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(6)
    .padding(20)
    .width(420);
    opaque(
        container(container(card).style(theme::card))
            .center(Length::Fill)
            .style(theme::backdrop),
    )
}

// ── Small widgets ─────────────────────────────────────────────────────────────

fn thin_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(8)
        .scroller_width(6)
        .margin(2)
}

/// Lays out a page's tiles edge to edge.
fn tiles(page: &PageImage) -> Element<'_, Message> {
    column(page.rows.iter().map(|tiles| {
        row(tiles.iter().map(|(handle, size)| {
            image(handle.clone())
                .width(size.width)
                .height(size.height)
                .into()
        }))
        .into()
    }))
    .width(page.logical.width)
    .height(page.logical.height)
    .into()
}

fn icon_button<'a>(
    icon: Icon,
    label: impl text::IntoFragment<'a>,
    on_press: Option<Message>,
    active: bool,
) -> Element<'a, Message> {
    let status = if on_press.is_some() {
        button::Status::Active
    } else {
        button::Status::Disabled
    };
    let glyph = svg(icon.handle())
        .width(17)
        .height(17)
        .style(theme::icon_for(status, active));
    let button = button(container(glyph).center(30))
        .padding(0)
        .on_press_maybe(on_press)
        .style(if active {
            theme::ghost_active
        } else {
            theme::ghost
        });
    tooltip(
        button,
        container(text(label).size(12))
            .padding([4, 8])
            .style(theme::tooltip),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// Status-bar content that clears when clicked.
fn dismissible(content: Element<'_, Message>) -> Element<'_, Message> {
    button(content)
        .padding(0)
        .style(theme::plain)
        .on_press(Message::DismissStatus)
        .into()
}

fn muted<'a>(content: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(content).size(size).style(|theme: &Theme| text::Style {
        color: Some(theme::tokens(theme).text_muted),
    })
}

fn kbd(key: &str) -> Element<'_, Message> {
    container(text(key).size(11))
        .padding([2, 6])
        .style(theme::kbd)
        .into()
}

fn horizontal_divider<'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .style(theme::divider)
        .into()
}

fn vertical_divider<'a>() -> Element<'a, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(theme::divider)
        .into()
}

fn toolbar_separator<'a>() -> Element<'a, Message> {
    container(container(Space::new().width(1).height(18)).style(theme::divider))
        .padding([0, 6])
        .into()
}
