//! Ferrix desktop application.

mod app;
mod worker;

use std::path::PathBuf;

use app::App;
use ferrix_engine_pdfium::PdfiumEngine;
use worker::EngineWorker;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("FERRIX_LOG"))
        .init();

    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    // A missing PDFium library is reported inside the window rather than as a crash.
    let engine = PdfiumEngine::new()
        .map_err(|e| e.to_string())
        .and_then(|engine| EngineWorker::spawn(engine).map_err(|e| e.to_string()));

    iced::application(
        move || App::new(engine.clone(), initial.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .subscription(App::subscription)
    .window_size((1024.0, 900.0))
    .centered()
    .run()
}
