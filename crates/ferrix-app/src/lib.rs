//! Ferrix desktop application (iced, ADR-0002).

mod app;
mod clipboard;
mod document;
mod icon;
#[cfg(windows)]
mod print;
mod recovery;
mod render;
pub mod theme;
mod view;
mod worker;
pub mod writer;
mod writer_canvas;

use std::path::PathBuf;

pub use app::{App, Message, Recovery};
pub use document::Origin;
use ferrix_engine_pdfium::PdfiumEngine;
use iced::{Theme, window};
pub use recovery::Recoverable;
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
    // Closing asks about unsaved changes first; the app exits itself when it may.
    .exit_on_close_request(false)
    .antialiasing(true)
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: iced::Size::new(1180.0, 860.0),
        min_size: Some(iced::Size::new(720.0, 480.0)),
        position: window::Position::Centered,
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            // Merge the title bar into our toolbar, like native document apps.
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        ..window::Settings::default()
    }
}

/// A paragraph style for scripts and tests: 0 Normal, 1 Title, 2–4 Heading 1–3.
#[doc(hidden)]
pub fn writer_style(index: usize) -> writer::StyleChoice {
    writer::STYLES[index.min(writer::STYLES.len() - 1)]
}
