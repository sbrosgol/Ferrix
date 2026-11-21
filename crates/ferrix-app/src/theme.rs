//! Ferrix visual design: color tokens for light and dark, and widget styles built on them.
//!
//! Widgets take their colors from [`tokens`], never from literals, so both themes stay consistent.

use iced::{
    Background, Border, Color, Shadow, Theme, Vector,
    border::Radius,
    theme::Palette,
    widget::{button, container, overlay::menu, pick_list, scrollable, svg, text_input},
};

/// Semantic colors for one appearance.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    /// Toolbar, sidebar and status bar.
    pub chrome: Color,
    /// The area behind pages.
    pub canvas: Color,
    /// Raised surfaces: inputs, menus, cards.
    pub surface: Color,
    /// Grouped controls (pills) set into the chrome.
    pub surface_sunken: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub text_muted: Color,
    /// Disabled text and placeholders.
    pub text_faint: Color,
    pub hover: Color,
    pub pressed: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub accent_soft: Color,
    /// Accent used as text or icon colour; meets 4.5:1 on `surface` and `chrome`.
    pub accent_text: Color,
    pub danger: Color,
    pub page_shadow: Color,
}

pub const LIGHT: Tokens = Tokens {
    chrome: rgb(0xF7F7F8),
    canvas: rgb(0xE8E9EC),
    surface: rgb(0xFFFFFF),
    surface_sunken: rgba(0x000000, 0.045),
    border: rgb(0xDEDFE3),
    border_strong: rgb(0xC4C7CE),
    text: rgb(0x1B1D21),
    text_muted: rgb(0x60656E),
    text_faint: rgb(0xA3A7AF),
    hover: rgba(0x000000, 0.055),
    pressed: rgba(0x000000, 0.10),
    // 4.9:1 on white, so accent text and icons pass WCAG AA.
    accent: rgb(0x3563E9),
    accent_hover: rgb(0x4A74EE),
    on_accent: rgb(0xFFFFFF),
    accent_soft: rgba(0x3563E9, 0.12),
    accent_text: rgb(0x3563E9),
    danger: rgb(0xD23B3B),
    page_shadow: rgba(0x0B1424, 0.20),
};

pub const DARK: Tokens = Tokens {
    chrome: rgb(0x1F2023),
    canvas: rgb(0x141517),
    surface: rgb(0x2A2C30),
    surface_sunken: rgba(0xFFFFFF, 0.06),
    border: rgb(0x34363B),
    border_strong: rgb(0x4A4D55),
    text: rgb(0xE8E9EC),
    text_muted: rgb(0x9CA1AA),
    text_faint: rgb(0x62666E),
    hover: rgba(0xFFFFFF, 0.07),
    pressed: rgba(0xFFFFFF, 0.12),
    // Fill: white text on it is 4.6:1. Text and icons use the lighter `accent_text` (6:1).
    accent: rgb(0x3D6BF0),
    accent_hover: rgb(0x5580F4),
    on_accent: rgb(0xFFFFFF),
    accent_soft: rgba(0x5B7CFA, 0.22),
    accent_text: rgb(0x8DA6FF),
    danger: rgb(0xF06A6A),
    page_shadow: rgba(0x000000, 0.55),
};

/// The UI typeface, bundled with the app (Regular and SemiBold).
pub const UI_FONT: iced::Font = iced::Font::with_name("Inter");

/// Controls.
pub const RADIUS: f32 = 6.0;
/// Groups of controls and menus.
pub const RADIUS_GROUP: f32 = 8.0;
/// Dialogs.
pub const RADIUS_DIALOG: f32 = 12.0;

const fn rgb(hex: u32) -> Color {
    rgba(hex, 1.0)
}

const fn rgba(hex: u32, a: f32) -> Color {
    Color {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

pub fn light() -> Theme {
    theme("Ferrix Light", &LIGHT)
}

pub fn dark() -> Theme {
    theme("Ferrix Dark", &DARK)
}

fn theme(name: &'static str, t: &Tokens) -> Theme {
    Theme::custom(
        name,
        Palette {
            background: t.chrome,
            text: t.text,
            primary: t.accent,
            success: rgb(0x2E9E5B),
            warning: rgb(0xD08A16),
            danger: t.danger,
        },
    )
}

/// The tokens for whichever Ferrix theme is active.
pub fn tokens(theme: &Theme) -> &'static Tokens {
    if theme.extended_palette().is_dark {
        &DARK
    } else {
        &LIGHT
    }
}

fn border(color: Color, width: f32, radius: f32) -> Border {
    Border {
        color,
        width,
        radius: Radius::from(radius),
    }
}

// ── Containers ────────────────────────────────────────────────────────────────

pub fn chrome(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style::default()
        .background(t.chrome)
        .color(t.text)
}

pub fn canvas(theme: &Theme) -> container::Style {
    container::Style::default().background(tokens(theme).canvas)
}

pub fn divider(theme: &Theme) -> container::Style {
    container::Style::default().background(tokens(theme).border)
}

/// A rendered PDF page: always white paper, lifted off the canvas.
pub fn page(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::WHITE)),
        shadow: Shadow {
            color: tokens(theme).page_shadow,
            offset: Vector::new(0.0, 2.0),
            blur_radius: 14.0,
        },
        ..container::Style::default()
    }
}

/// A thumbnail's paper: white with a hairline edge, so white pages stand out from the sidebar.
pub fn thumbnail_paper(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::WHITE)),
        // A translucent edge reads on both the light and the dark sidebar.
        border: border(
            if theme.extended_palette().is_dark {
                Color::TRANSPARENT
            } else {
                Color::BLACK.scale_alpha(0.14)
            },
            1.0,
            0.0,
        ),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.12),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..container::Style::default()
    }
}

/// The frame around a sidebar thumbnail: a hairline normally, an accent ring when selected.
pub fn thumbnail_ring(theme: &Theme, selected: bool) -> container::Style {
    let t = tokens(theme);
    container::Style {
        border: if selected {
            border(t.accent, 2.0, 6.0)
        } else {
            border(Color::TRANSPARENT, 2.0, 6.0)
        },
        ..container::Style::default()
    }
}

/// A thumbnail's page number: an accent pill for the current page, plain otherwise.
pub fn page_badge(theme: &Theme, current: bool) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: current.then_some(Background::Color(t.accent)),
        text_color: Some(if current { t.on_accent } else { t.text_muted }),
        border: border(Color::TRANSPARENT, 0.0, 9.0),
        ..container::Style::default()
    }
}

/// Related controls grouped on one sunken, rounded surface (page navigator, zoom).
pub fn pill(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(tokens(theme).surface_sunken)),
        border: border(Color::TRANSPARENT, 0.0, RADIUS_GROUP),
        ..container::Style::default()
    }
}

/// A popover menu, e.g. the toolbar's overflow menu.
pub fn popover(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.surface)),
        text_color: Some(t.text),
        border: border(t.border, 1.0, 10.0),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.18),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 20.0,
        },
        ..container::Style::default()
    }
}

/// Section headings inside menus and the sidebar.
pub fn overline(theme: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(tokens(theme).text_muted),
    }
}

pub fn tooltip(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.text)),
        text_color: Some(t.chrome),
        border: border(Color::TRANSPARENT, 0.0, 5.0),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.2),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

/// The rounded tile behind the empty-state icon.
pub fn badge(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.accent_soft)),
        border: border(Color::TRANSPARENT, 0.0, 18.0),
        ..container::Style::default()
    }
}

/// Dims the window behind a dialog.
pub fn backdrop(theme: &Theme) -> container::Style {
    let shade = if theme.extended_palette().is_dark {
        0.5
    } else {
        0.25
    };
    container::Style::default().background(Color::BLACK.scale_alpha(shade))
}

/// A raised dialog surface.
pub fn card(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.surface)),
        text_color: Some(t.text),
        border: border(t.border, 1.0, RADIUS_DIALOG),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.25),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 28.0,
        },
        ..container::Style::default()
    }
}

pub fn kbd(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(Background::Color(t.surface)),
        border: border(t.border_strong, 1.0, 4.0),
        text_color: Some(t.text_muted),
        ..container::Style::default()
    }
}

// ── Buttons ───────────────────────────────────────────────────────────────────

/// A row in a popover menu; `checked` rows show the accent text of the current choice.
pub fn menu_item(checked: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => Some(Background::Color(t.hover)),
            _ => None,
        };
        button::Style {
            background,
            text_color: if checked { t.accent_text } else { t.text },
            border: border(Color::TRANSPARENT, 0.0, RADIUS),
            ..button::Style::default()
        }
    }
}

/// Borderless toolbar button that only shows a background on hover/press.
pub fn ghost(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    let background = match status {
        button::Status::Hovered => Some(Background::Color(t.hover)),
        button::Status::Pressed => Some(Background::Color(t.pressed)),
        button::Status::Active | button::Status::Disabled => None,
    };
    button::Style {
        background,
        text_color: if status == button::Status::Disabled {
            t.text_faint
        } else {
            t.text
        },
        border: border(Color::TRANSPARENT, 0.0, RADIUS),
        ..button::Style::default()
    }
}

/// The chosen segment of a segmented switch: a raised surface within a pill.
pub fn segment_active(theme: &Theme, _status: button::Status) -> button::Style {
    let t = tokens(theme);
    button::Style {
        background: Some(Background::Color(t.surface)),
        text_color: t.text,
        border: border(t.border, 1.0, RADIUS),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.08),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..button::Style::default()
    }
}

/// A ghost button that is currently switched on (e.g. sidebar shown, fit-width active).
pub fn ghost_active(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    button::Style {
        background: Some(Background::Color(if status == button::Status::Hovered {
            t.accent_soft.scale_alpha(1.4)
        } else {
            t.accent_soft
        })),
        text_color: t.accent_text,
        ..ghost(theme, status)
    }
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    let background = match status {
        button::Status::Hovered => t.accent_hover,
        button::Status::Disabled => t.accent.scale_alpha(0.4),
        button::Status::Active | button::Status::Pressed => t.accent,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: t.on_accent,
        border: border(Color::TRANSPARENT, 0.0, RADIUS),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.12),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..button::Style::default()
    }
}

/// A bordered neutral button, for the non-default choices in dialogs.
pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    let background = match status {
        button::Status::Hovered => t.hover,
        button::Status::Pressed => t.pressed,
        button::Status::Active | button::Status::Disabled => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: t.text,
        border: border(t.border_strong, 1.0, RADIUS),
        ..button::Style::default()
    }
}

/// Wraps a thumbnail; the selection ring is drawn by the inner container.
pub fn plain(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style::default()
}

// ── Icons ─────────────────────────────────────────────────────────────────────

/// Icons follow the text color of the button they sit in, dimmed when disabled.
pub fn icon_for(
    status: button::Status,
    active: bool,
) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, _| {
        let t = tokens(theme);
        let color = match (status, active) {
            (button::Status::Disabled, _) => t.text_faint,
            (_, true) => t.accent_text,
            _ => t.text,
        };
        svg::Style { color: Some(color) }
    }
}

pub fn icon_colored(color: fn(&Tokens) -> Color) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, _| svg::Style {
        color: Some(color(tokens(theme))),
    }
}

// ── Inputs ────────────────────────────────────────────────────────────────────

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let t = tokens(theme);
    let border_color = match status {
        text_input::Status::Focused { .. } => t.accent,
        text_input::Status::Hovered => t.border_strong,
        _ => t.border,
    };
    text_input::Style {
        background: Background::Color(t.surface),
        border: border(border_color, 1.0, RADIUS),
        icon: t.text_muted,
        placeholder: t.text_faint,
        value: t.text,
        selection: t.accent_soft.scale_alpha(1.8),
    }
}

pub fn select(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let t = tokens(theme);
    let border_color = match status {
        pick_list::Status::Opened { .. } => t.accent,
        pick_list::Status::Hovered => t.border_strong,
        pick_list::Status::Active => t.border,
    };
    pick_list::Style {
        text_color: t.text,
        placeholder_color: t.text_faint,
        handle_color: t.text_muted,
        background: Background::Color(t.surface),
        border: border(border_color, 1.0, RADIUS),
    }
}

/// A select without its own box, for use inside a [`pill`].
pub fn select_flat(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let t = tokens(theme);
    let background = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened { .. } => t.hover,
        pick_list::Status::Active => Color::TRANSPARENT,
    };
    pick_list::Style {
        text_color: t.text,
        placeholder_color: t.text_faint,
        handle_color: t.text_muted,
        background: Background::Color(background),
        border: border(Color::TRANSPARENT, 0.0, RADIUS),
    }
}

pub fn select_menu(theme: &Theme) -> menu::Style {
    let t = tokens(theme);
    menu::Style {
        background: Background::Color(t.surface),
        border: border(t.border, 1.0, RADIUS_GROUP),
        text_color: t.text,
        // A soft highlight reads as "current" without shouting like a solid accent block.
        selected_text_color: t.text,
        selected_background: Background::Color(t.accent_soft),
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.18),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
    }
}

// ── Scrollbars ────────────────────────────────────────────────────────────────

/// Thin, rounded scrollbars that stay out of the way until hovered.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let t = tokens(theme);
    let mut style = scrollable::default(theme, status);
    let (v_hot, h_hot) = match status {
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered: v,
            is_horizontal_scrollbar_hovered: h,
            ..
        } => (v, h),
        scrollable::Status::Dragged {
            is_vertical_scrollbar_dragged: v,
            is_horizontal_scrollbar_dragged: h,
            ..
        } => (v, h),
        scrollable::Status::Active { .. } => (false, false),
    };
    for (rail, hot) in [
        (&mut style.vertical_rail, v_hot),
        (&mut style.horizontal_rail, h_hot),
    ] {
        rail.background = None;
        rail.border = border(Color::TRANSPARENT, 0.0, 4.0);
        rail.scroller.background = Background::Color(if hot {
            t.text_muted.scale_alpha(0.7)
        } else {
            t.text_muted.scale_alpha(0.35)
        });
        rail.scroller.border = border(Color::TRANSPARENT, 0.0, 4.0);
    }
    style
}
