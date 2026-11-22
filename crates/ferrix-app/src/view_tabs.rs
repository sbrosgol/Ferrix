//! The tab bar under the toolbar, and the quit summary (design system › Viewer (M1) › Tabs).

use iced::{
    Alignment, Element, Length, Theme,
    widget::{
        Space, button, column, container, mouse_area, opaque, row, scrollable, svg, text, tooltip,
    },
};

use crate::{
    app::{App, Message},
    icon::Icon,
    tabs::TabLabel,
    theme,
    view::{SEMIBOLD, icon_button, muted, platform_keys},
};

/// Height of the tab bar, without its hairline.
pub(crate) const TAB_BAR_HEIGHT: f32 = 36.0;
const TAB_HEIGHT: f32 = 28.0;
const TAB_MIN: f32 = 120.0;
const TAB_MAX: f32 = 220.0;
const TAB_GAP: f32 = 2.0;
/// The bar's side padding and the New Tab button with its gap.
const BAR_CHROME: f32 = 2.0 * 8.0 + 4.0 + 30.0;

/// Width of each tab: an equal share of the bar, between the limits.
pub(crate) fn tab_width(bar: f32, count: usize) -> f32 {
    if count == 0 {
        return TAB_MAX;
    }
    let share = (bar - BAR_CHROME - TAB_GAP * (count - 1) as f32) / count as f32;
    share.clamp(TAB_MIN, TAB_MAX)
}

/// `title` cut at the end with “…” to fit `width` pixels of 13 px Inter (about 7 px a
/// character); the container clips too, so this only decides where the “…” goes.
pub(crate) fn fit_title(title: &str, width: f32) -> String {
    let max = ((width / 7.0).floor() as usize).max(4);
    if title.chars().count() <= max {
        return title.to_owned();
    }
    let mut cut: String = title.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// A path for the tooltip, with home as `~`.
fn tooltip_path(label: &TabLabel) -> String {
    match &label.path {
        Some(path) => match std::env::var("HOME") {
            Ok(home) if !cfg!(windows) && path.starts_with(&home) => {
                format!("~{}", &path[home.len()..])
            }
            _ => path.clone(),
        },
        None if label.writing || label.title != "New Tab" => "Not saved yet".into(),
        None => "New Tab".into(),
    }
}

impl App {
    /// The tab bar; `None` with a single tab, so a one-document window looks as before.
    pub(crate) fn tab_bar(&self) -> Option<Element<'_, Message>> {
        if self.tabs.len() < 2 {
            return None;
        }
        let (labels, active) = self.tab_labels();
        let width = tab_width(self.window_size.width, labels.len());
        let mut tabs = row![].spacing(TAB_GAP).align_y(Alignment::Center);
        for (i, label) in labels.iter().enumerate() {
            // A separator only between two inactive tabs.
            if i > 0 && i != active && i - 1 != active {
                tabs = tabs.push(container(Space::new().width(1).height(16)).style(theme::divider));
            } else if i > 0 {
                tabs = tabs.push(Space::new().width(1));
            }
            tabs = tabs.push(self.tab(i, label, i == active, width));
        }
        let scrolls =
            labels.len() as f32 * (TAB_MIN + TAB_GAP + 1.0) + BAR_CHROME > self.window_size.width;
        let strip: Element<'_, Message> = if scrolls {
            scrollable(tabs)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(0).scroller_width(0),
                ))
                .id(tab_scroll_id())
                .width(Length::Fill)
                .into()
        } else {
            container(tabs).width(Length::Fill).into()
        };
        let bar = row![
            strip,
            icon_button(Icon::Plus, "New Tab (⌘T)", Some(Message::NewTab), false),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        Some(
            container(bar)
                .height(TAB_BAR_HEIGHT)
                .padding([4, 8])
                .center_y(TAB_BAR_HEIGHT)
                .style(theme::chrome)
                .into(),
        )
    }

    fn tab<'a>(
        &'a self,
        index: usize,
        label: &TabLabel,
        active: bool,
        width: f32,
    ) -> Element<'a, Message> {
        let icon: Element<'a, Message> = match (label.writing, label.title.as_str()) {
            (false, "New Tab") if label.path.is_none() => Space::new().width(0).into(),
            (writing, _) => svg(if writing {
                Icon::FileWriting
            } else {
                Icon::FileText
            }
            .handle())
            .width(14)
            .height(14)
            .style(theme::icon_colored(|t| t.text_muted))
            .into(),
        };
        let hovered = self.hovered_tab == Some(index);
        // The end slot: the close button (active tab, or on hover), else the edited dot.
        let end: Element<'a, Message> = if active || hovered {
            let close = button(
                container(
                    svg(Icon::Close.handle())
                        .width(12)
                        .height(12)
                        .style(theme::icon_colored(|t| t.text_muted)),
                )
                .center(24),
            )
            .padding(0)
            .style(theme::ghost)
            .on_press(Message::CloseTab(Some(index)));
            tooltip(
                close,
                container(text(platform_keys("Close Tab (⌘W)")).size(12))
                    .padding([4, 8])
                    .style(theme::tooltip),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .into()
        } else if label.dirty {
            container(
                container(Space::new().width(6).height(6)).style(|theme: &Theme| {
                    container::Style::default()
                        .background(theme::tokens(theme).text_muted)
                        .border(iced::Border::default().rounded(3))
                }),
            )
            .center(24)
            .into()
        } else {
            Space::new().width(24).into()
        };
        let title = text(fit_title(
            &label.title,
            width - 8.0 - 4.0 - 14.0 - 6.0 - 4.0 - 24.0,
        ))
        .size(13)
        .wrapping(text::Wrapping::None);
        let title = if active {
            title
        } else {
            title.style(|t: &Theme| text::Style {
                color: Some(theme::tokens(t).text_muted),
            })
        };
        let content = row![icon, container(title).width(Length::Fill).clip(true), end]
            .spacing(6)
            .align_y(Alignment::Center);
        let body = container(content)
            .width(width)
            .height(TAB_HEIGHT)
            .padding(iced::Padding {
                top: 0.0,
                right: 4.0,
                bottom: 0.0,
                left: 8.0,
            })
            .center_y(TAB_HEIGHT)
            .style(move |theme: &Theme| theme::tab(theme, active, hovered));
        let area = mouse_area(body)
            .on_press(Message::SelectTab(index))
            .on_middle_press(Message::CloseTab(Some(index)))
            .on_enter(Message::TabHover(Some(index)))
            .on_exit(Message::TabHover(None));
        tooltip(
            area,
            container(text(tooltip_path(label)).size(12))
                .padding([4, 8])
                .style(theme::tooltip),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .delay(std::time::Duration::from_millis(700))
        .into()
    }

    /// "Review unsaved changes before quitting?", with several edited tabs.
    pub(crate) fn quit_summary_dialog<'a>(&self, names: &'a [String]) -> Element<'a, Message> {
        let count = names.len();
        let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
        let mut list = shown.join(" · ");
        if count > 5 {
            list.push_str(&format!(" and {} more", count - 5));
        }
        let card = column![
            row![
                text("Review unsaved changes before quitting?")
                    .size(17)
                    .font(SEMIBOLD),
                Space::new().width(Length::Fill),
                icon_button(Icon::Close, "Cancel (Esc)", Some(Message::Escape), false),
            ]
            .align_y(Alignment::Center),
            muted(
                format!(
                    "{count} documents have unsaved changes. If you don’t review them, the \
                     changes will be lost."
                ),
                13.0
            ),
            text(list).size(13),
            Space::new().height(8),
            row![
                button(text("Discard Changes").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press(Message::QuitDiscard),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(13))
                    .padding([6, 14])
                    .style(theme::secondary)
                    .on_press(Message::Escape),
                button(text("Review Changes…").size(13))
                    .padding([6, 18])
                    .style(theme::primary)
                    .on_press(Message::QuitReview),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
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

pub(crate) fn tab_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("tabs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_share_the_bar_within_limits() {
        assert_eq!(tab_width(1180.0, 2), TAB_MAX);
        let eight = tab_width(1180.0, 8);
        assert!(eight > TAB_MIN && eight < TAB_MAX, "{eight}");
        assert_eq!(tab_width(760.0, 20), TAB_MIN);
    }

    #[test]
    fn long_titles_end_in_an_ellipsis() {
        assert_eq!(fit_title("Q3 report.pdf", 200.0), "Q3 report.pdf");
        let cut = fit_title("A very long annual report title.pdf", 70.0);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
    }
}
