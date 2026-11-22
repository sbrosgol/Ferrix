//! Ferrix's own local data (ADR-0005): preferences people can read and edit, and the history
//! the app keeps for them.
//!
//! - [`Settings`]: one JSON file in the config directory. Every field has a default, so a
//!   missing, older or hand-broken file never stops the app starting.
//! - [`Store`]: SQLite in the data directory for recent files and where each document was
//!   left. WAL mode, so several windows or processes can share it.
//!
//! The app never sees SQL or file formats, and tests point both at a temporary directory.

mod settings;
mod store;

pub use settings::{Appearance, Layout, SavedZoom, Settings};
pub use store::{RecentFile, Store, StoreError, ViewState};

use std::path::PathBuf;

/// Where preferences live: `settings.json` in the platform config directory.
pub fn settings_path() -> Option<PathBuf> {
    ferrix_io::app_config_dir().map(|d| d.join("settings.json"))
}

/// Where history lives: `state.db` in the platform data directory.
pub fn state_path() -> Option<PathBuf> {
    ferrix_io::app_data_dir().map(|d| d.join("state.db"))
}
