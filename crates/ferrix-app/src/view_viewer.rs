//! Views for reading and finding your way: the sidebar switch and outline, Open Recent, the
//! link confirmation, Back, and the password dialog (design system › Viewer (M1)).

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use ferrix_core::SourceId;
use iced::{
    Alignment, Element, Length, Theme,
    widget::{
        Space, button, column, container, opaque, row, scrollable, stack, svg, text, text_input,
        tooltip,
    },
};

use crate::{
    annotate_ui::SidebarTab,
    app::{App, Layout, Message},
    document::Document,
    icon::Icon,
    navigate::{self, MAX_INDENT, UriKind},
    theme,
    view::{SEMIBOLD, muted, platform_keys},
};

/// Rows of Open Recent in the empty state.
const RECENT_IN_EMPTY_STATE: usize = 6;

impl App {
    /// The sidebar header: Pages | Outline | Comments, as icons with tooltips.
    pub(crate) fn sidebar_switch<'a>(&'a self, _doc: &'a Document) -> Element<'a, Message> {
        let tab = |icon: Icon, label: &'static str, which: SidebarTab| -> Element<'a, Message> {
            let active = self.sidebar_tab == which;
            let glyph = svg(icon.handle())
                .width(16)
                .height(16)
                .style(theme::icon_colored(if active {
                    |t: &theme::Tokens| t.text
                } else {
                    |t: &theme::Tokens| t.text_muted
                }));
            let segment = button(container(glyph).center_x(Length::Fill).center_y(24))
                .width(Length::Fill)
                .padding(2)
                .style(if active {
                    theme::segment_active
                } else {
                    theme::ghost
                })
                .on_press(Message::SidebarTab(which));
            tooltip(
                segment,
                container(text(platform_keys(label)).size(12))
                    .padding([4, 8])
                    .style(theme::tooltip),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .into()
        };
        container(
            container(
                row![
                    tab(Icon::Pages, "Pages (⌥⌘2)", SidebarTab::Pages),
                    tab(Icon::Outline, "Outline (⌥⌘3)", SidebarTab::Outline),
                    tab(Icon::Comments, "Comments (⌥⌘4)", SidebarTab::Comments),
                ]
                .spacing(2),
            )
            .padding(2)
            .style(theme::pill),
        )
        .padding([10, 12])
        .into()
    }

    /// The outline as a tree of rows: chevrons fold, a row jumps to its section.
    pub(crate) fn outline_list<'a>(&'a self, doc: &'a Document) -> Element<'a, Message> {
        let Some(nodes) = doc.outline.as_ref() else {
            return Space::new().into();
        };
        if nodes.is_empty() {
            return container(
                column![
                    text("No outline").size(13),
                    muted("This PDF doesn’t list its chapters or sections.", 12.0)
                        .align_x(Alignment::Center),
                ]
                .spacing(4)
                .max_width(180)
                .align_x(Alignment::Center),
            )
            .padding(iced::Padding {
                top: 24.0,
                ..iced::Padding::ZERO
            })
            .center_x(Length::Fill)
            .into();
        }
        // Where each of the document's own pages is now, after any reordering.
        let mut positions = vec![None; doc.info.page_count as usize];
        for (i, e) in doc.state.pages.entries().iter().enumerate() {
            if e.source == SourceId::BASE
                && let Some(slot) = positions.get_mut(e.page.0 as usize)
            {
                *slot = Some(i);
            }
        }
        let position = |n: &navigate::OutlineNode| {
            n.page
                .and_then(|p| positions.get(p.0 as usize).copied().flatten())
        };
        let visible = navigate::visible_rows(nodes);
        let current = navigate::current_section(nodes, &visible, position, doc.current);
        let alt = self.modifiers.alt();
        let mut list = column![].padding([4, 8]);
        for &i in &visible {
            let node = &nodes[i];
            let target = position(node);
            let chevron: Element<'a, Message> = if node.has_children(i) {
                button(
                    svg(if node.open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    }
                    .handle())
                    .width(14)
                    .height(14)
                    .style(theme::icon_colored(|t| t.text_muted)),
                )
                .padding(1)
                .style(theme::plain)
                .on_press(Message::OutlineToggle(i, alt))
                .into()
            } else {
                Space::new().width(16).into()
            };
            let is_current = current == Some(i);
            let title =
                text(&node.title)
                    .size(13)
                    .font(if is_current { SEMIBOLD } else { theme::UI_FONT });
            let title: Element<'a, Message> = if target.is_some() {
                title.into()
            } else {
                title
                    .style(|t: &Theme| text::Style {
                        color: Some(theme::tokens(t).text_muted),
                    })
                    .into()
            };
            let number: Element<'a, Message> = match target {
                Some(p) => muted((p + 1).to_string(), 12.0).into(),
                None => Space::new().into(),
            };
            let indent = node.depth.min(MAX_INDENT) as f32 * 12.0;
            let content = row![
                Space::new().width(indent),
                chevron,
                container(title).width(Length::Fill),
                number,
            ]
            .spacing(4)
            .align_y(Alignment::Center);
            list = list.push(
                button(content)
                    .width(Length::Fill)
                    .padding([6, 8])
                    .style(theme::menu_item(is_current))
                    .on_press_maybe(target.map(|_| Message::OutlineJump(i))),
            );
        }
        scrollable(list)
            .height(Length::Fill)
            .style(theme::scrollbar)
            .into()
    }

    /// The Open Recent flyout, beside the `⋯` menu.
    pub(crate) fn recent_flyout(&self) -> Element<'_, Message> {
        let mut items = column![].spacing(1);
        for recent in &self.recent {
            items = items.push(recent_row(&recent.path, None));
        }
        items = items.push(container(divider()).padding([4, 0])).push(
            button(text("Clear Recent").size(13))
                .width(Length::Fill)
                .padding([6, 8])
                .style(theme::menu_item(false))
                .on_press(Message::ClearRecent),
        );
        container(items)
            .width(300)
            .padding(6)
            .style(theme::popover)
            .into()
    }

    /// Recent files under the empty state's buttons.
    pub(crate) fn recent_section(&self) -> Option<Element<'_, Message>> {
        if self.recent.is_empty() || self.engine.is_err() {
            return None;
        }
        let now = SystemTime::now();
        let mut list = column![
            container(
                text("Recent")
                    .size(11)
                    .font(SEMIBOLD)
                    .style(theme::overline)
            )
            .padding([0, 8])
        ]
        .spacing(2);
        for recent in self.recent.iter().take(RECENT_IN_EMPTY_STATE) {
            list = list.push(recent_row(&recent.path, Some(when(recent.opened, now))));
        }
        let width = (self.window_size.width - 48.0).clamp(200.0, 400.0);
        Some(container(list).width(width).into())
    }

    /// "Back to page 3", while there is somewhere to go back to.
    pub(crate) fn back_button(&self) -> Option<Element<'_, Message>> {
        let page = self.back_page()?;
        let content = row![
            svg(Icon::Back.handle())
                .width(14)
                .height(14)
                .style(theme::icon_colored(|t| t.text_muted)),
            muted(format!("Back to page {page}"), 12.0),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let back = button(container(content).center_y(20))
            .padding([0, 8])
            .style(theme::ghost)
            .on_press(Message::Back);
        let key = if cfg!(target_os = "macos") {
            "Back (⌘[)"
        } else {
            "Back (Alt+←)"
        };
        Some(
            tooltip(
                back,
                container(text(key).size(12))
                    .padding([4, 8])
                    .style(theme::tooltip),
                tooltip::Position::Top,
            )
            .gap(4)
            .into(),
        )
    }

    /// "Open this link?", before a document sends you to a web page or mail app.
    pub(crate) fn link_dialog<'a>(&self, uri: &'a str) -> Element<'a, Message> {
        let mail = navigate::uri_kind(uri) == UriKind::Mail;
        let (title, line, copy, open) = if mail {
            (
                "Write an email?",
                "It opens in your email app.",
                "Copy Address",
                "Write Email",
            )
        } else {
            (
                "Open this link?",
                "It opens in your browser.",
                "Copy Link",
                "Open Link",
            )
        };
        let shown = if mail {
            uri.strip_prefix("mailto:").unwrap_or(uri)
        } else {
            uri
        };
        let shown = navigate::middle_ellipsis(shown, 240);
        // The host in semibold, so a lookalike domain stands out.
        let address: Element<'a, Message> = match navigate::host_of(&shown) {
            Some(host) if !mail => {
                let (before, after) = shown.split_once(host).unwrap_or((&shown, ""));
                let spans: Vec<iced::widget::text::Span<'a, (), iced::Font>> = vec![
                    iced::widget::span(before.to_owned()),
                    iced::widget::span(host.to_owned()).font(SEMIBOLD),
                    iced::widget::span(after.to_owned()),
                ];
                iced::widget::rich_text(spans).size(13).into()
            }
            _ => text(shown.clone()).size(13).into(),
        };
        let mut body = column![
            row![
                text(title).size(17).font(SEMIBOLD),
                Space::new().width(Length::Fill),
                crate::view::icon_button(
                    Icon::Close,
                    "Cancel (Esc)",
                    Some(Message::LinkAnswer(false)),
                    false
                ),
            ]
            .align_y(Alignment::Center),
            muted(line, 13.0),
            container(address)
                .width(Length::Fill)
                .padding([8, 12])
                .style(theme::pill),
        ]
        .spacing(8);
        if navigate::host_of(uri).is_some_and(|h| !h.is_ascii()) {
            body = body.push(muted(
                "This address uses letters from other alphabets. Check it carefully.",
                12.0,
            ));
        }
        let card = body
            .push(Space::new().height(8))
            .push(
                row![
                    button(text(copy).size(13))
                        .padding([6, 14])
                        .style(theme::secondary)
                        .on_press(Message::CopyLink),
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(13))
                        .padding([6, 14])
                        .style(theme::secondary)
                        .on_press(Message::LinkAnswer(false)),
                    button(text(open).size(13).font(SEMIBOLD))
                        .padding([6, 18])
                        .style(theme::primary)
                        .on_press(Message::LinkAnswer(true)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(24)
            .width(480);
        opaque(
            container(container(card).style(theme::card))
                .center(Length::Fill)
                .style(theme::backdrop),
        )
    }

    /// The View section of the `⋯` menu: layouts, then Dark Pages.
    pub(crate) fn view_menu_items(
        &self,
        item: impl Fn(&'static str, bool, Message) -> Element<'static, Message>,
    ) -> Vec<Element<'static, Message>> {
        let mut items: Vec<Element<'static, Message>> = [
            ("Continuous", Layout::Continuous),
            ("Single Page", Layout::SinglePage),
            ("Two Pages", Layout::TwoPages),
            ("Two Pages with Cover", Layout::TwoPagesCover),
        ]
        .into_iter()
        .map(|(label, layout)| item(label, self.layout == layout, Message::SetLayout(layout)))
        .collect();
        items.push(item("Dark Pages", self.invert, Message::ToggleInvert));
        items
    }

    /// The password dialog for an encrypted PDF.
    pub(crate) fn password_dialog<'a>(
        &'a self,
        prompt: &'a crate::app::PasswordPrompt,
    ) -> Element<'a, Message> {
        let name = prompt.origin.file_name();
        let checking = prompt.checking;
        let field = text_input("Password", &prompt.password)
            .id(crate::app::password_input_id())
            .secure(!prompt.reveal)
            .size(13)
            .padding(iced::Padding {
                top: 7.0,
                bottom: 7.0,
                left: 10.0,
                right: 36.0,
            })
            .style(if prompt.wrong {
                theme::input_error
            } else {
                theme::input
            })
            .on_input_maybe((!checking).then_some(Message::PasswordInput))
            .on_submit_maybe(
                (!checking && !prompt.password.is_empty()).then_some(Message::PasswordSubmit),
            );
        let eye = crate::view::icon_button(
            if prompt.reveal {
                Icon::EyeOff
            } else {
                Icon::Eye
            },
            if prompt.reveal {
                "Hide password"
            } else {
                "Show password"
            },
            Some(Message::PasswordReveal),
            false,
        );
        let field = stack![
            field,
            container(eye)
                .width(Length::Fill)
                .align_x(Alignment::End)
                .padding([1, 2]),
        ];
        let mut card = column![
            row![
                text("Password required").size(17).font(SEMIBOLD),
                Space::new().width(Length::Fill),
                crate::view::icon_button(
                    Icon::Close,
                    "Cancel (Esc)",
                    Some(Message::PasswordCancel),
                    false
                ),
            ]
            .align_y(Alignment::Center),
            muted(format!("Enter the password for “{name}”."), 13.0).wrapping(text::Wrapping::None),
            field,
        ]
        .spacing(8);
        if prompt.wrong {
            card = card.push(
                row![
                    svg(Icon::Alert.handle())
                        .width(14)
                        .height(14)
                        .style(theme::icon_colored(|t| t.danger)),
                    text("Wrong password. Try again.")
                        .size(12)
                        .style(|t: &Theme| text::Style {
                            color: Some(theme::tokens(t).danger),
                        }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
        let open_label = if checking { "Opening…" } else { "Open" };
        let card = card
            .push(Space::new().height(8))
            .push(
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(13))
                        .padding([6, 14])
                        .style(theme::secondary)
                        .on_press_maybe((!checking).then_some(Message::PasswordCancel)),
                    button(text(open_label).size(13).font(SEMIBOLD))
                        .padding([6, 18])
                        .style(theme::primary)
                        .on_press_maybe(
                            (!checking && !prompt.password.is_empty())
                                .then_some(Message::PasswordSubmit),
                        ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(24)
            .width(480);
        opaque(
            container(container(card).style(theme::card))
                .center(Length::Fill)
                .style(theme::backdrop),
        )
    }
}

/// A recent file: name over folder, the full path as tooltip, and a date when given.
fn recent_row(path: &Path, date: Option<String>) -> Element<'static, Message> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let folder = folder_label(path, home().as_deref(), 44);
    let labels = column![
        text(name).size(13).wrapping(text::Wrapping::None),
        muted(folder, 12.0).wrapping(text::Wrapping::None),
    ]
    .spacing(1);
    let mut content = row![].spacing(12).align_y(Alignment::Center);
    if date.is_some() {
        content = content.push(
            svg(Icon::FileText.handle())
                .width(16)
                .height(16)
                .style(theme::icon_colored(|t| t.text_muted)),
        );
    }
    content = content.push(container(labels).width(Length::Fill).clip(true));
    if let Some(date) = date {
        content = content.push(muted(date, 12.0));
    }
    let row = button(content)
        .width(Length::Fill)
        .padding([6, 8])
        .style(theme::menu_item(false))
        .on_press(Message::OpenRecent(path.to_path_buf()));
    tooltip(
        row,
        container(text(path.display().to_string()).size(12))
            .padding([4, 8])
            .style(theme::tooltip),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

fn divider<'a>() -> Element<'a, Message> {
    container(Space::new().height(1))
        .width(Length::Fill)
        .style(theme::divider)
        .into()
}

fn home() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        None
    } else {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

/// A file's folder for display: home as `~`, shortened in the middle to `max` characters.
pub(crate) fn folder_label(path: &Path, home: Option<&Path>, max: usize) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    let shown = match home.and_then(|h| parent.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => parent.display().to_string(),
    };
    navigate::middle_ellipsis(&shown, max)
}

/// When a file was opened, in the Comments formats ("Just now" … "21 Nov 2025").
fn when(opened: SystemTime, now: SystemTime) -> String {
    let secs = |t: SystemTime| {
        t.duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    };
    crate::markup::relative_date(&pdf_date(secs(opened)), secs(now))
}

/// Seconds since the epoch as a PDF date in UTC, `D:YYYYMMDDHHmmSSZ`.
fn pdf_date(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_show_home_as_a_tilde_and_shorten_in_the_middle() {
        let home = Path::new("/Users/ana");
        assert_eq!(
            folder_label(Path::new("/Users/ana/Documents/a.pdf"), Some(home), 40),
            format!("~{}Documents", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(
            folder_label(Path::new("/Users/ana/a.pdf"), Some(home), 40),
            "~"
        );
        assert_eq!(
            folder_label(Path::new("/srv/x/a.pdf"), Some(home), 40),
            "/srv/x"
        );
        let long = folder_label(
            Path::new("/Volumes/Archive/Projects/2026/Quarterly/Reports/Final/a.pdf"),
            None,
            20,
        );
        assert_eq!(long.chars().count(), 20);
        assert!(long.contains('…'));
    }

    #[test]
    fn dates_convert_to_pdf_form() {
        assert_eq!(pdf_date(0), "D:19700101000000Z");
        assert_eq!(pdf_date(1_795_305_600), "D:20261122000000Z");
        let now = UNIX_EPOCH + std::time::Duration::from_secs(1_795_305_600);
        assert_eq!(when(now, now), "Just now");
        let earlier = now - std::time::Duration::from_secs(86_400 * 30);
        assert_eq!(when(earlier, now), "23 Oct 2026");
    }
}
