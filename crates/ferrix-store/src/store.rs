//! History: recent files and where each document was left, in SQLite.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::settings::{Layout, SavedZoom};

/// Documents remembered at most; the least recently opened are forgotten first.
const KEEP: usize = 1000;

/// Schema migrations; entry `n` takes the database from `user_version` n to n + 1.
const MIGRATIONS: [&str; 1] = [r"
    CREATE TABLE documents (
        path      TEXT PRIMARY KEY NOT NULL,
        opened_ms INTEGER NOT NULL,
        page      INTEGER NOT NULL DEFAULT 0,
        within    REAL NOT NULL DEFAULT 0,
        zoom      TEXT,
        layout    TEXT
    );
    CREATE INDEX documents_by_time ON documents (opened_ms DESC);
"];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A file in the Open Recent list.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentFile {
    pub path: PathBuf,
    pub opened: SystemTime,
}

/// Where a document was left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    /// Index of the page on screen.
    pub page: u32,
    /// How far down that page the view started, as a fraction of its height.
    pub within: f32,
    pub zoom: SavedZoom,
    pub layout: Layout,
}

/// The history database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens or creates the database at `path`. A file that isn't a usable database is moved
    /// aside (to `state.db.broken`) and replaced: history is worth less than starting.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        match Self::open_at(path) {
            Ok(store) => Ok(store),
            Err(StoreError::Sql(e)) if is_corrupt(&e) => {
                tracing::warn!("{}: {e}; starting a new history", path.display());
                let mut aside = path.as_os_str().to_owned();
                aside.push(".broken");
                std::fs::rename(path, &aside)?;
                for suffix in ["-wal", "-shm"] {
                    let mut side = path.as_os_str().to_owned();
                    side.push(suffix);
                    let _ = std::fs::remove_file(side);
                }
                Self::open_at(path)
            }
            Err(e) => Err(e),
        }
    }

    /// A database in memory, for tests.
    pub fn in_memory() -> Result<Self, StoreError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn open_at(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        // Another window may be writing; wait briefly rather than fail.
        conn.busy_timeout(Duration::from_millis(250))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // WAL with NORMAL survives application crashes; a power cut may lose the last
        // page turn, which is fine for history.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let done = usize::try_from(version).unwrap_or(0);
        for (n, sql) in (0_i64..).zip(MIGRATIONS).skip(done) {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", n + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Records that `path` was opened just now, putting it first in the recent list.
    pub fn touch(&self, path: &Path) -> Result<(), StoreError> {
        let Some(key) = key(path) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO documents (path, opened_ms) VALUES (?1, ?2)
             ON CONFLICT (path) DO UPDATE SET opened_ms = excluded.opened_ms",
            params![key, now_ms()],
        )?;
        self.conn.execute(
            "DELETE FROM documents WHERE path NOT IN
             (SELECT path FROM documents ORDER BY opened_ms DESC LIMIT ?1)",
            params![KEEP as i64],
        )?;
        Ok(())
    }

    /// The most recently opened files, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<RecentFile>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, opened_ms FROM documents ORDER BY opened_ms DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let path: String = r.get(0)?;
            let ms: i64 = r.get(1)?;
            Ok(RecentFile {
                path: PathBuf::from(path),
                opened: UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Forgets `path`: off the recent list, and its position too.
    pub fn forget(&self, path: &Path) -> Result<(), StoreError> {
        if let Some(key) = key(path) {
            self.conn
                .execute("DELETE FROM documents WHERE path = ?1", params![key])?;
        }
        Ok(())
    }

    /// Forgets every document (the "Clear Menu" of Open Recent).
    pub fn clear(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM documents", [])?;
        Ok(())
    }

    /// Where `path` was left, if it was opened before and its position saved.
    pub fn view(&self, path: &Path) -> Result<Option<ViewState>, StoreError> {
        let Some(key) = key(path) else {
            return Ok(None);
        };
        let row = self
            .conn
            .query_row(
                "SELECT page, within, zoom, layout FROM documents
                 WHERE path = ?1 AND zoom IS NOT NULL",
                params![key],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(page, within, zoom, layout)| ViewState {
            page: u32::try_from(page).unwrap_or(0),
            within: (within as f32).clamp(-1.0, 1.0),
            zoom: serde_json::from_str::<SavedZoom>(&zoom)
                .unwrap_or_default()
                .sanitized(),
            layout: layout
                .and_then(|l| serde_json::from_str(&l).ok())
                .unwrap_or_default(),
        }))
    }

    /// Remembers where `path` was left. Adds it to the recent list if it isn't there.
    pub fn set_view(&self, path: &Path, view: ViewState) -> Result<(), StoreError> {
        let Some(key) = key(path) else {
            return Ok(());
        };
        let zoom = serde_json::to_string(&view.zoom.sanitized()).map_err(std::io::Error::other)?;
        let layout = serde_json::to_string(&view.layout).map_err(std::io::Error::other)?;
        let within = if view.within.is_finite() {
            f64::from(view.within.clamp(-1.0, 1.0))
        } else {
            0.0
        };
        self.conn.execute(
            "INSERT INTO documents (path, opened_ms, page, within, zoom, layout)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (path) DO UPDATE SET page = excluded.page, within = excluded.within,
                 zoom = excluded.zoom, layout = excluded.layout",
            params![key, now_ms(), i64::from(view.page), within, zoom, layout],
        )?;
        Ok(())
    }
}

/// Paths are stored as text; one that isn't valid Unicode is simply not remembered.
fn key(path: &Path) -> Option<&str> {
    path.to_str()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn is_corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::in_memory().expect("store")
    }

    #[test]
    fn recent_files_are_newest_first_without_duplicates() {
        let s = store();
        for name in ["/a.pdf", "/b.pdf", "/a.pdf", "/c.pdf"] {
            s.touch(Path::new(name)).expect("touch");
            std::thread::sleep(Duration::from_millis(2));
        }
        let paths: Vec<PathBuf> = s
            .recent(10)
            .expect("recent")
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(paths, ["/c.pdf", "/a.pdf", "/b.pdf"].map(PathBuf::from));
        assert_eq!(s.recent(1).expect("recent").len(), 1);
    }

    #[test]
    fn a_position_round_trips_and_survives_a_touch() {
        let s = store();
        let path = Path::new("/doc.pdf");
        assert_eq!(s.view(path).expect("view"), None);
        s.touch(path).expect("touch");
        assert_eq!(s.view(path).expect("view"), None, "opened, never left");
        let view = ViewState {
            page: 41,
            within: 0.25,
            zoom: SavedZoom::Fixed(1.5),
            layout: Layout::TwoPages,
        };
        s.set_view(path, view).expect("set");
        s.touch(path).expect("touch again");
        assert_eq!(s.view(path).expect("view"), Some(view));
        s.set_view(
            path,
            ViewState {
                zoom: SavedZoom::FitPage,
                within: f32::NAN,
                ..view
            },
        )
        .expect("set");
        let again = s.view(path).expect("view").expect("some");
        assert_eq!((again.zoom, again.within), (SavedZoom::FitPage, 0.0));
    }

    #[test]
    fn forgetting_and_clearing() {
        let s = store();
        s.touch(Path::new("/a.pdf")).expect("touch");
        s.touch(Path::new("/b.pdf")).expect("touch");
        s.forget(Path::new("/a.pdf")).expect("forget");
        assert_eq!(s.recent(10).expect("recent").len(), 1);
        s.clear().expect("clear");
        assert!(s.recent(10).expect("recent").is_empty());
    }

    #[test]
    fn history_is_capped() {
        let s = store();
        for i in 0..KEEP + 5 {
            s.touch(Path::new(&format!("/{i}.pdf"))).expect("touch");
        }
        let count: i64 = s
            .conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, KEEP as i64);
    }

    #[test]
    fn a_broken_database_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("ferrix-store-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("state.db");
        std::fs::write(&path, vec![0x5a; 8192]).expect("write");
        let s = Store::open(&path).expect("opens anyway");
        s.touch(Path::new("/a.pdf")).expect("usable");
        assert!(dir.join("state.db.broken").exists());
        drop(s);
        // And it opens again normally, keeping what was written.
        let s = Store::open(&path).expect("reopen");
        assert_eq!(s.recent(5).expect("recent").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
