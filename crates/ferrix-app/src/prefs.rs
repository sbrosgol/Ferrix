//! Preferences and history (ADR-0005) as the app uses them: loaded at startup, written in the
//! background so the UI never waits on the disk.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ferrix_store::{RecentFile, Settings, Store, ViewState};

use crate::worker::blocking;

/// Files shown in Open Recent.
pub(crate) const RECENT_SHOWN: usize = 10;

/// The history database, shared with background writes.
#[derive(Clone)]
pub struct History(Arc<Mutex<Store>>);

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("History")
    }
}

impl History {
    /// Opens the database and reads the recent list, off the UI thread.
    pub(crate) async fn open() -> Result<(Self, Vec<RecentFile>), String> {
        blocking(|| {
            let path = ferrix_store::state_path().ok_or("no data directory")?;
            let store = Store::open(&path).map_err(|e| e.to_string())?;
            let recent = existing(store.recent(RECENT_SHOWN * 2).map_err(|e| e.to_string())?);
            Ok((Self(Arc::new(Mutex::new(store))), recent))
        })
        .await
    }

    /// Runs `f` on the store; a poisoned lock (a panic mid-write) only loses history.
    fn with<T>(&self, f: impl FnOnce(&Store) -> Result<T, ferrix_store::StoreError>) -> Option<T> {
        let store = self.0.lock().ok()?;
        f(&store)
            .inspect_err(|e| tracing::warn!("history: {e}"))
            .ok()
    }

    /// Where `path` was left. Read on the UI thread: one indexed row, needed before the first
    /// page is laid out.
    pub(crate) fn view(&self, path: &Path) -> Option<ViewState> {
        self.with(|s| s.view(path)).flatten()
    }

    /// Records an opened file and returns the new recent list.
    pub(crate) async fn touch(self, path: PathBuf) -> Vec<RecentFile> {
        self.update(move |s| s.touch(&path)).await
    }

    pub(crate) async fn forget(self, path: PathBuf) -> Vec<RecentFile> {
        self.update(move |s| s.forget(&path)).await
    }

    pub(crate) async fn clear(self) -> Vec<RecentFile> {
        self.update(Store::clear).await
    }

    pub(crate) async fn set_view(self, path: PathBuf, view: ViewState) {
        let _ = blocking(move || {
            self.with(|s| s.set_view(&path, view));
            Ok(())
        })
        .await;
    }

    /// Saves a position now, for when the app is about to exit and can't wait.
    pub(crate) fn set_view_now(&self, path: &Path, view: ViewState) {
        self.with(|s| s.set_view(path, view));
    }

    async fn update(
        self,
        change: impl FnOnce(&Store) -> Result<(), ferrix_store::StoreError> + Send + 'static,
    ) -> Vec<RecentFile> {
        blocking(move || {
            Ok(self
                .with(|s| {
                    change(s)?;
                    s.recent(RECENT_SHOWN * 2)
                })
                .map(existing)
                .unwrap_or_default())
        })
        .await
        .unwrap_or_default()
    }
}

/// The recent files that still exist, at most [`RECENT_SHOWN`]. Moved or deleted files stay
/// in the database (they may come back, say on a remounted drive) but aren't offered.
fn existing(recent: Vec<RecentFile>) -> Vec<RecentFile> {
    recent
        .into_iter()
        .filter(|r| r.path.is_file())
        .take(RECENT_SHOWN)
        .collect()
}

/// Reads the preferences. Small, local and needed for the first frame, so read directly.
pub(crate) fn load_settings() -> Settings {
    ferrix_store::settings_path().map_or_else(Settings::default, |p| Settings::load(&p))
}

/// Writes the preferences in the background; a failure is logged, not shown.
pub(crate) async fn save_settings(settings: Settings) {
    let _ = blocking(move || {
        if let Some(path) = ferrix_store::settings_path()
            && let Err(e) = settings.save(&path)
        {
            tracing::warn!("settings: {e}");
        }
        Ok(())
    })
    .await;
}
