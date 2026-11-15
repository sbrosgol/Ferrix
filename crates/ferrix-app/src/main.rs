//! Ferrix desktop application.

// Release builds on Windows are GUI programs: no console window opens with them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("FERRIX_LOG"))
        .init();

    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    ferrix_app::application(initial).run()
}
