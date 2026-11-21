//! Ferrix desktop application (iced, ADR-0002).

pub mod annotate;
pub mod annotate_ui;
mod app;
mod clipboard;
mod document;
mod icon;
pub mod markup;
pub mod markup_canvas;
mod pdf_find;
#[cfg(windows)]
mod print;
mod recovery;
mod render;
mod strip;
mod text_select;
pub mod theme;
mod titlebar;
mod view;
mod view_markup;
mod worker;
pub mod writer;
mod writer_canvas;

use std::path::PathBuf;

pub use app::{App, Appearance, Message, Recovery};
pub use document::Origin;
use ferrix_engine_pdfium::PdfiumEngine;
use iced::{Theme, window};
pub use recovery::Recoverable;
pub use titlebar::TitleBarAction;
use worker::EngineWorker;
pub use writer::{Action as WriteAction, KeyCommand};

/// Builds the application, optionally opening `initial` on start.
///
/// A missing PDFium library is reported inside the window rather than as a crash.
pub fn application(
    initial: Option<PathBuf>,
) -> iced::Application<impl iced::Program<State = App, Message = Message, Theme = Theme>> {
    let engine = PdfiumEngine::new()
        .map_err(|e| e.to_string())
        .and_then(|engine| EngineWorker::spawn(engine).map_err(|e| e.to_string()));

    iced::application(
        move || App::new(engine.clone(), initial.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window(window_settings())
    // Inter (SIL OFL 1.1, `assets/fonts/LICENSE-OFL`) with real weights, so headings and body
    // text share one typeface on every platform (design system: Type).
    .font(include_bytes!("../assets/fonts/Inter-Regular.ttf").as_slice())
    .font(include_bytes!("../assets/fonts/Inter-SemiBold.ttf").as_slice())
    .default_font(theme::UI_FONT)
    // Closing asks about unsaved changes first; the app exits itself when it may.
    .exit_on_close_request(false)
    .antialiasing(true)
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: iced::Size::new(1180.0, 860.0),
        min_size: Some(iced::Size::new(720.0, 480.0)),
        position: window::Position::Centered,
        // macOS takes the icon from the app bundle (`cargo xtask bundle`) and ignores this.
        icon: if cfg!(target_os = "macos") {
            None
        } else {
            window_icon()
        },
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            // Merge the title bar into our toolbar, like native document apps.
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            // Wayland's app id and X11's WM_CLASS: desktops match the window to
            // `dist/linux/io.github.sbrosgol.ferrix.desktop` by it, for its name and icon.
            application_id: "io.github.sbrosgol.ferrix".to_owned(),
            ..window::settings::PlatformSpecific::default()
        },
        ..window::Settings::default()
    }
}

/// Edge of the rendered window icon; the window manager or taskbar scales it down.
const ICON_EDGE: u32 = 256;

/// The app icon (the same SVG the macOS bundle uses) for the title bar and taskbar.
fn window_icon() -> Option<window::Icon> {
    let (rgba, edge) = render_icon(ICON_EDGE)?;
    window::icon::from_rgba(rgba, edge, edge)
        .inspect_err(|e| tracing::warn!("window icon: {e}"))
        .ok()
}

/// Renders the app icon to straight-alpha RGBA pixels, `edge` pixels square.
fn render_icon(edge: u32) -> Option<(Vec<u8>, u32)> {
    use resvg::{tiny_skia, usvg};

    let svg = include_bytes!("../assets/app-icon.svg");
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default())
        .inspect_err(|e| tracing::warn!("window icon: {e}"))
        .ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(edge, edge)?;
    let zoom = edge as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(zoom, zoom),
        &mut pixmap.as_mut(),
    );
    // tiny-skia keeps premultiplied alpha; window icons take it straight.
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Some((rgba, edge))
}

/// A paragraph style for scripts and tests: 0 Normal, 1 Title, 2–4 Heading 1–3.
#[doc(hidden)]
pub fn writer_style(index: usize) -> writer::StyleChoice {
    writer::STYLES[index.min(writer::STYLES.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_renders_with_a_transparent_margin_and_a_white_page() {
        let (rgba, edge) = render_icon(64).expect("icon renders");
        assert_eq!(rgba.len(), (edge * edge * 4) as usize);
        let pixel = |x: u32, y: u32| {
            let i = ((y * edge + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        assert_eq!(pixel(0, 0)[3], 0, "corner outside the tile is transparent");
        assert_eq!(
            pixel(32, 48),
            [255, 255, 255, 255],
            "the page is opaque white"
        );
        assert!(window_icon().is_some());
    }
}
