//! Preferences: a small JSON file people can read, edit, back up and sync.

use std::{io, path::Path};

use serde::{Deserialize, Serialize};

/// The format version written by this build. Older files load through `#[serde(default)]`;
/// a newer file loads too, and keys this build doesn't know are kept when it saves.
const VERSION: u32 = 1;

/// Light, dark, or whatever the operating system uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// How pages are arranged in the view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// One column of pages.
    #[default]
    Continuous,
    /// One page at a time.
    SinglePage,
    /// Pairs of pages side by side: 1–2, 3–4…
    TwoPages,
    /// Pairs after a first page on its own, like an open book: 1, 2–3, 4–5…
    TwoPagesCover,
}

/// A zoom as remembered: a fit mode, or a fixed factor (1.0 = 100 %).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SavedZoom {
    #[default]
    Auto,
    FitWidth,
    FitPage,
    Fixed(f32),
}

impl SavedZoom {
    /// A fixed zoom outside sane bounds (a hand-edited file) falls back to automatic.
    pub fn sanitized(self) -> Self {
        match self {
            Self::Fixed(z) if !(z.is_finite() && (0.05..=16.0).contains(&z)) => Self::Auto,
            other => other,
        }
    }
}

/// The user's preferences. Every field has a default, so a partial file is fine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub appearance: Appearance,
    /// Name put on new annotations and replies.
    pub author: Option<String>,
    /// The PDF sidebar is shown.
    pub sidebar: bool,
    /// Pages are shown with inverted colours, for reading in the dark.
    pub invert_pages: bool,
    /// Page layout for documents opened for the first time.
    pub layout: Layout,
    /// Zoom for documents opened for the first time.
    pub zoom: SavedZoom,
    /// Reopen documents where they were left.
    pub restore_position: bool,
    /// Keys written by a newer Ferrix, kept as they were.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            appearance: Appearance::default(),
            author: None,
            sidebar: true,
            invert_pages: false,
            layout: Layout::default(),
            zoom: SavedZoom::default(),
            restore_position: true,
            unknown: serde_json::Map::new(),
        }
    }
}

impl Settings {
    /// Reads `path`. A missing file gives the defaults; an unreadable one is logged and gives
    /// the defaults too, and is left alone until the next [`Settings::save`] replaces it.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_json(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{}: {e}; using default settings", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!("{}: {e}; using default settings", path.display());
                Self::default()
            }
        }
    }

    /// Parses settings, ignoring fields with values of the wrong type rather than failing.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let serde_json::Value::Object(fields) = value else {
            return Ok(Self::default());
        };
        // Field by field, so one bad value (say "sidebar": "yes") costs only that field.
        let mut settings = Self::default();
        let mut kept = serde_json::Map::new();
        for (key, value) in fields {
            let mut one = serde_json::Map::new();
            one.insert(key.clone(), value.clone());
            match serde_json::from_value::<Self>(serde_json::Value::Object(one)) {
                Ok(parsed) => settings.merge_field(&key, parsed),
                Err(e) => tracing::warn!("settings: ignoring “{key}”: {e}"),
            }
            if !KNOWN.contains(&key.as_str()) {
                kept.insert(key, value);
            }
        }
        settings.unknown = kept;
        settings.zoom = settings.zoom.sanitized();
        if let Some(author) = &settings.author
            && author.trim().is_empty()
        {
            settings.author = None;
        }
        Ok(settings)
    }

    fn merge_field(&mut self, key: &str, from: Self) {
        match key {
            "version" => self.version = from.version,
            "appearance" => self.appearance = from.appearance,
            "author" => self.author = from.author,
            "sidebar" => self.sidebar = from.sidebar,
            "invert_pages" => self.invert_pages = from.invert_pages,
            "layout" => self.layout = from.layout,
            "zoom" => self.zoom = from.zoom,
            "restore_position" => self.restore_position = from.restore_position,
            _ => {}
        }
    }

    /// Writes the settings atomically, creating the directory if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut copy = self.clone();
        copy.version = copy.version.max(VERSION);
        let mut json = serde_json::to_vec_pretty(&copy).map_err(io::Error::other)?;
        json.push(b'\n');
        ferrix_io::write_atomic(path, &json)
    }
}

const KNOWN: [&str; 8] = [
    "version",
    "appearance",
    "author",
    "sidebar",
    "invert_pages",
    "layout",
    "zoom",
    "restore_position",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ferrix-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = scratch("missing");
        assert_eq!(
            Settings::load(&dir.join("settings.json")),
            Settings::default()
        );
    }

    #[test]
    fn settings_round_trip() {
        let path = scratch("round").join("settings.json");
        let settings = Settings {
            appearance: Appearance::Dark,
            author: Some("Ana Ruiz".into()),
            sidebar: false,
            invert_pages: true,
            layout: Layout::TwoPagesCover,
            zoom: SavedZoom::Fixed(1.5),
            ..Settings::default()
        };
        settings.save(&path).expect("saved");
        assert_eq!(Settings::load(&path), settings);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"two-pages-cover\""), "{text}");
    }

    #[test]
    fn bad_values_cost_only_their_own_field() {
        let json = br#"{"appearance": "dark", "sidebar": "yes", "zoom": {"fixed": 900},
            "author": "  ", "layout": "spiral"}"#;
        let s = Settings::from_json(json).expect("parsed");
        assert_eq!(s.appearance, Appearance::Dark);
        assert!(s.sidebar, "bad value keeps the default");
        assert_eq!(s.zoom, SavedZoom::Auto, "absurd zoom falls back");
        assert_eq!(s.author, None, "blank author is no author");
        assert_eq!(s.layout, Layout::Continuous);
    }

    #[test]
    fn keys_from_a_newer_version_survive_a_save() {
        let path = scratch("newer").join("settings.json");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, br#"{"version": 7, "future_thing": [1, 2]}"#).expect("write");
        let mut s = Settings::load(&path);
        assert_eq!(s.version, 7);
        s.sidebar = false;
        s.save(&path).expect("saved");
        let again = Settings::load(&path);
        assert_eq!(
            again.unknown.get("future_thing"),
            Some(&serde_json::json!([1, 2]))
        );
        assert_eq!(again.version, 7, "never downgrades the version");
        assert!(!again.sidebar);
    }

    #[test]
    fn garbage_gives_the_defaults() {
        assert!(Settings::from_json(b"not json").is_err());
        assert_eq!(
            Settings::from_json(b"[1]").expect("parsed"),
            Settings::default()
        );
    }
}
