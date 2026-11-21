//! Markup mode views: the annotation bar, page overlays, the note editor, the Comments list
//! and the signed-document dialog (`docs/design-system.md` › Annotations).

use ferrix_annot::Permission;
use ferrix_core::{AnnotId, AnnotKind, Annotation, PageEntry, Rgb};
use iced::{
    Alignment, Background, Border, Color, Element, Length, Theme,
    widget::{
        Space, button, canvas, column, container, opaque, pick_list, row, scrollable, stack, svg,
        text, text_editor,
    },
};

use crate::{
    annotate_ui::{AnnotMsg, CERTIFIED, SidebarTab},
    app::{App, Message},
    document::Document,
    icon::Icon,
    markup::{self, COLORS, OPACITIES, Tool, WIDTHS},
    markup_canvas::Overlay,
    render::LOGICAL_PX_PER_POINT,
    theme,
    view::{BAR_HEIGHT, CLUSTER_GAP, SEMIBOLD, icon_button, muted, pill, small_chevron},
};

/// An opacity choice for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Opacity(u8);

impl std::fmt::Display for Opacity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} %", self.0)
    }
}

const OPACITY_CHOICES: [Opacity; 4] = [
    Opacity(OPACITIES[0]),
    Opacity(OPACITIES[1]),
    Opacity(OPACITIES[2]),
    Opacity(OPACITIES[3]),
];

fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Select => Icon::Pointer,
        Tool::Note => Icon::StickyNote,
        Tool::Pen => Icon::Pencil,
        Tool::Rectangle => Icon::Square,
        Tool::Ellipse => Icon::Circle,
        Tool::Line => Icon::Minus,
        Tool::Arrow => Icon::ArrowUpRight,
    }
}

fn kind_icon(annot: &Annotation) -> Icon {
    match annot.kind {
        AnnotKind::Note { .. } => Icon::StickyNote,
        AnnotKind::Ink { .. } => Icon::Pencil,
        AnnotKind::Square => Icon::Square,
        AnnotKind::Circle => Icon::Circle,
        AnnotKind::Line { ends, .. } if ends[1] != ferrix_core::LineEnd::None => Icon::ArrowUpRight,
        AnnotKind::Line { .. } => Icon::Minus,
        AnnotKind::Other { .. } => Icon::Comments,
    }
}

/// A plain-language name for an annotation's type.
pub(crate) fn kind_name(annot: &Annotation) -> String {
    match &annot.kind {
        AnnotKind::Note { .. } => "Note".into(),
        AnnotKind::Ink { .. } => "Drawing".into(),
        AnnotKind::Square => "Rectangle".into(),
        AnnotKind::Circle => "Ellipse".into(),
        AnnotKind::Line { ends, .. } if ends[1] != ferrix_core::LineEnd::None => "Arrow".into(),
        AnnotKind::Line { .. } => "Line".into(),
        AnnotKind::Other { subtype } => match subtype.as_str() {
            "FreeText" => "Text box".into(),
            "Highlight" => "Highlight".into(),
            "Underline" => "Underline".into(),
            "StrikeOut" => "Strikethrough".into(),
            "Squiggly" => "Squiggly underline".into(),
            "Stamp" => "Stamp".into(),
            "FileAttachment" => "Attachment".into(),
            "Text" => "Reply".into(),
            other => other.to_owned(),
        },
    }
}

fn iced_color(c: Rgb) -> Color {
    Color::from_rgb(c.r, c.g, c.b)
}

impl App {
    /// The bar under the toolbar in Markup mode: tools, then properties of the selection or
    /// the active tool.
    pub(crate) fn markup_bar<'a>(&'a self, doc: &'a Document) -> Element<'a, Message> {
        let Some(m) = self.markup.as_ref() else {
            return Space::new().into();
        };
        let bar = |content: Element<'a, Message>| -> Element<'a, Message> {
            container(content)
                .height(BAR_HEIGHT)
                .padding([0, 10])
                .center_y(BAR_HEIGHT)
                .width(Length::Fill)
                .style(theme::chrome)
                .into()
        };
        if let Some(Permission::Forbidden(_)) = doc.annot_policy.as_ref().map(|p| &p.permission) {
            return bar(muted(CERTIFIED, 12.0).into());
        }
        let tools = pill(row(Tool::ALL.into_iter().map(|tool| {
            icon_button(
                tool_icon(tool),
                tool.label(),
                Some(Message::Annot(AnnotMsg::Tool(tool))),
                m.tool == tool,
            )
        })));

        let selected = m
            .selected
            .and_then(|id| doc.state.annots.get(id).map(|(_, a)| (id, a)));
        let mut content = row![tools].spacing(CLUSTER_GAP).align_y(Alignment::Center);
        match selected {
            Some((_, annot)) if !markup::editable(annot) => {
                let why = if annot.is_deletable() {
                    "Made in another app. Ferrix can only delete it."
                } else {
                    "Locked by its author."
                };
                content = content.push(muted(why, 12.0));
            }
            _ => {
                let tool = if m.tool == Tool::Select {
                    Tool::Pen
                } else {
                    m.tool
                };
                let style = m.style(tool);
                let (color, width, opacity, is_note) = match selected {
                    Some((_, a)) => (
                        a.props.color.unwrap_or(Rgb::BLACK),
                        a.props.border_width,
                        a.props.opacity,
                        matches!(a.kind, AnnotKind::Note { .. }),
                    ),
                    None => (style.color, style.width, style.opacity, tool == Tool::Note),
                };
                content = content.push(pill(row(COLORS.iter().enumerate().map(
                    |(i, (name, c))| {
                        swatch(
                            *c,
                            close(*c, color),
                            name,
                            Message::Annot(AnnotMsg::Color(i)),
                        )
                    },
                ))));
                if !is_note {
                    content = content.push(pill(row(WIDTHS
                        .iter()
                        .map(|w| width_button(*w, (w - width).abs() < 0.01)))));
                }
                let current = Opacity((opacity * 100.0).round() as u8);
                content = content.push(
                    pick_list(OPACITY_CHOICES, Some(current), |o: Opacity| {
                        Message::Annot(AnnotMsg::Opacity(o.0))
                    })
                    .text_size(13)
                    .padding([4, 8])
                    .width(80)
                    .handle(small_chevron())
                    .style(theme::select)
                    .menu_style(theme::select_menu),
                );
            }
        }
        let mut full = row![content].align_y(Alignment::Center);
        if selected.is_some_and(|(_, a)| a.is_deletable()) {
            full = full
                .push(Space::new().width(Length::Fill))
                .push(icon_button(
                    Icon::Delete,
                    "Delete annotation (⌫)",
                    Some(Message::Annot(AnnotMsg::Delete)),
                    false,
                ));
        }
        bar(full.width(Length::Fill).into())
    }

    /// A page with its overlay in Markup mode, and the note editor if it belongs here.
    pub(crate) fn page_layers<'a>(
        &'a self,
        doc: &'a Document,
        entry: &PageEntry,
        size: iced::Size,
        page: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let Some(m) = self.markup.as_ref() else {
            return page;
        };
        let Some(transform) = doc
            .sources
            .get(entry.source.0 as usize)
            .and_then(|s| s.transforms.get(entry.page.0 as usize))
            .map(|t| t.rotated(entry.rotation))
        else {
            return page;
        };
        let scale = doc.strip.zoom * LOGICAL_PX_PER_POINT;
        let tool = m.tool;
        let overlay = Overlay {
            page: entry.key,
            transform,
            scale,
            annots: doc.state.annots.page(entry.key),
            selected: m.selected,
            gesture: m
                .gesture
                .as_ref()
                .filter(|(p, _)| *p == entry.key)
                .map(|(_, g)| g),
            tool,
            style: m.style(tool),
            shift: self.modifiers.shift(),
            alt: self.modifiers.alt(),
        };
        let mut layers = stack![page, canvas(overlay).width(size.width).height(size.height)];
        if let Some(note) = self.note.as_ref().filter(|n| n.page == entry.key)
            && let Some((_, annot)) = doc.state.annots.get(note.id)
        {
            let r = annot.props.rect;
            let corner = transform.to_page(r.right, r.top);
            let (x, y) = (corner.x * scale + 8.0, corner.y * scale);
            const WIDTH: f32 = 280.0;
            // Flip to the left of the note when there's no room on the right.
            let x = if x + WIDTH > size.width {
                (x - WIDTH - 36.0).max(0.0)
            } else {
                x
            };
            layers = layers.push(
                container(opaque(note_editor(note, annot)))
                    .width(size.width)
                    .height(size.height)
                    .padding(iced::Padding {
                        top: y.clamp(0.0, (size.height - 200.0).max(0.0)),
                        left: x,
                        right: 0.0,
                        bottom: 0.0,
                    }),
            );
        }
        layers.into()
    }

    /// The sidebar header: a Pages | Comments switch.
    pub(crate) fn sidebar_switch<'a>(&'a self, doc: &'a Document) -> Element<'a, Message> {
        let comments = doc.state.annots.len();
        let tab = |label: String, which: SidebarTab| {
            let active = self.sidebar_tab == which;
            button(
                container(text(label).size(12).font(if active {
                    SEMIBOLD
                } else {
                    theme::UI_FONT
                }))
                .center_x(Length::Fill),
            )
            .width(Length::Fill)
            .padding([5, 8])
            .style(if active {
                theme::segment_active
            } else {
                theme::ghost
            })
            .on_press(Message::SidebarTab(which))
        };
        container(
            container(
                row![
                    tab(format!("Pages  {}", doc.len()), SidebarTab::Pages),
                    tab(format!("Comments  {comments}"), SidebarTab::Comments),
                ]
                .spacing(2),
            )
            .padding(2)
            .style(theme::pill),
        )
        .padding([10, 12])
        .into()
    }

    /// Every annotation, grouped by page.
    pub(crate) fn comments_list<'a>(&'a self, doc: &'a Document) -> Element<'a, Message> {
        let selected = self.markup.as_ref().and_then(|m| m.selected);
        let mut list = column![].spacing(2).padding([4, 8]);
        let mut any = false;
        for (n, entry) in doc.state.pages.entries().iter().enumerate() {
            let annots = doc.state.annots.page(entry.key);
            if annots.is_empty() {
                continue;
            }
            any = true;
            list = list.push(
                container(
                    text(format!("Page {}", n + 1))
                        .size(11)
                        .font(SEMIBOLD)
                        .style(theme::overline),
                )
                .padding([8, 6]),
            );
            for (id, annot) in annots {
                list = list.push(comment_row(entry.key, *id, annot, selected == Some(*id)));
            }
        }
        if !any {
            return container(
                column![
                    text("No comments").size(13).font(SEMIBOLD),
                    muted("Turn on Markup to add notes and drawings.", 12.0)
                        .align_x(Alignment::Center),
                ]
                .spacing(4)
                .align_x(Alignment::Center),
            )
            .padding([32, 16])
            .center_x(Length::Fill)
            .into();
        }
        scrollable(list)
            .height(Length::Fill)
            .style(theme::scrollbar)
            .into()
    }

    pub(crate) fn signed_dialog(&self) -> Element<'_, Message> {
        let card = column![
            text("This document is signed").size(17).font(SEMIBOLD),
            muted(
                "The signature stays valid, but signature checkers will report that the \
                 document was changed after signing.",
                13.0,
            ),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                button(text("Cancel").size(13))
                    .padding([7, 16])
                    .style(theme::secondary)
                    .on_press(Message::Annot(AnnotMsg::ConfirmSigned(false))),
                button(text("Add Annotations").size(13))
                    .padding([7, 16])
                    .style(theme::primary)
                    .on_press(Message::Annot(AnnotMsg::ConfirmSigned(true))),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .padding(24)
        .width(480);
        opaque(
            container(container(card).style(theme::card))
                .center(Length::Fill)
                .style(theme::backdrop),
        )
    }
}

fn close(a: Rgb, b: Rgb) -> bool {
    (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
}

fn swatch<'a>(c: Rgb, chosen: bool, name: &'a str, message: Message) -> Element<'a, Message> {
    let dot = container(Space::new().width(16).height(16)).style(move |theme: &Theme| {
        let t = theme::tokens(theme);
        container::Style {
            background: Some(Background::Color(iced_color(c))),
            // A ring keeps black visible on dark chrome; the chosen one gets a stronger ring.
            border: Border {
                color: if chosen { t.text } else { t.border_strong },
                width: if chosen { 2.0 } else { 1.0 },
                radius: 9.0.into(),
            },
            ..container::Style::default()
        }
    });
    iced::widget::tooltip(
        button(container(dot).center(28))
            .padding(0)
            .style(theme::ghost)
            .on_press(message),
        container(text(name).size(12))
            .padding([4, 8])
            .style(theme::tooltip),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

fn width_button<'a>(width: f32, chosen: bool) -> Element<'a, Message> {
    let line = container(Space::new().width(16).height(width.clamp(1.0, 6.0)))
        .style(|theme: &Theme| container::Style::default().background(theme::tokens(theme).text));
    iced::widget::tooltip(
        button(container(line).center(28))
            .padding(0)
            .style(if chosen {
                theme::ghost_active
            } else {
                theme::ghost
            })
            .on_press(Message::Annot(AnnotMsg::Width(width))),
        container(text(format!("{width} pt")).size(12))
            .padding([4, 8])
            .style(theme::tooltip),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

fn comment_row<'a>(
    page: ferrix_core::PageKey,
    id: AnnotId,
    annot: &'a Annotation,
    selected: bool,
) -> Element<'a, Message> {
    let tint = annot.props.color.unwrap_or(Rgb::BLACK);
    let icon = svg(kind_icon(annot).handle())
        .width(16)
        .height(16)
        .style(move |_: &Theme, _| svg::Style {
            color: Some(iced_color(tint)),
        });
    let mut title = kind_name(annot);
    if let Some(author) = annot.props.author.as_deref().filter(|a| !a.is_empty()) {
        title = format!("{title} · {author}");
    }
    let mut head = row![
        text(title)
            .size(13)
            .font(SEMIBOLD)
            .wrapping(text::Wrapping::None)
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if !markup::editable(annot) {
        head = head.push(
            svg(Icon::Lock.handle())
                .width(12)
                .height(12)
                .style(theme::icon_colored(|t| t.text_faint)),
        );
    }
    let mut body = column![head].spacing(2);
    if let Some(contents) = annot.props.contents.as_deref().filter(|c| !c.is_empty()) {
        let preview: String = contents.lines().take(2).collect::<Vec<_>>().join("\n");
        body = body.push(muted(preview, 13.0));
    }
    button(row![icon, body].spacing(10))
        .width(Length::Fill)
        .padding([8, 10])
        .style(move |theme: &Theme, status| {
            let mut style = theme::menu_item(false)(theme, status);
            if selected {
                style.background = Some(Background::Color(theme::tokens(theme).accent_soft));
            }
            style
        })
        .on_press(Message::Annot(AnnotMsg::Select(page, id)))
        .into()
}

fn note_editor<'a>(
    note: &'a crate::annotate_ui::NoteEditor,
    annot: &'a Annotation,
) -> Element<'a, Message> {
    let author = annot
        .props
        .author
        .clone()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| "Note".into());
    let locked = annot
        .props
        .flags
        .has(ferrix_core::AnnotFlags::LOCKED_CONTENTS)
        || !markup::editable(annot);
    let mut editor = text_editor(&note.content)
        .placeholder("Add a note")
        .size(13)
        .height(Length::Fixed(110.0))
        .padding(8);
    if !locked {
        editor = editor.on_action(|a| Message::Annot(AnnotMsg::NoteEdit(a)));
    }
    let mut body = column![text(author).size(13).font(SEMIBOLD), editor].spacing(8);
    if locked {
        body = body.push(muted("The author locked this note’s text.", 12.0));
    }
    body = body.push(
        row![
            icon_button(
                Icon::Delete,
                "Delete note",
                annot
                    .is_deletable()
                    .then_some(Message::Annot(AnnotMsg::NoteDelete)),
                false,
            ),
            Space::new().width(Length::Fill),
            button(text("Done").size(13))
                .padding([6, 16])
                .style(theme::primary)
                .on_press(Message::Annot(AnnotMsg::NoteDone)),
        ]
        .align_y(Alignment::Center),
    );
    container(body)
        .width(280)
        .padding(12)
        .style(theme::popover)
        .into()
}
