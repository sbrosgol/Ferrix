//! Crash-safe saving (ADR-0003 §5) and detection of files changed behind our back.

#![forbid(unsafe_code)]

pub mod journal;

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU32, Ordering},
    time::SystemTime,
};

/// Size and modification time of a file, to notice when someone else changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl FileStamp {
    pub fn of(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// Replaces `path` with `bytes` so that a crash or power loss leaves either the old or the new
/// file, never a mix: write a temporary file beside it, flush it to disk, then rename over.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = temp_path(path)?;
    let result = write_and_sync(&temp, bytes).and_then(|()| {
        // Keep the original's permissions when replacing it.
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&temp, meta.permissions())?;
        }
        fs::rename(&temp, path)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    sync_parent_dir(path);
    Ok(())
}

/// A unique hidden path in the same directory, so the final rename never crosses filesystems.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".ferrix-{}-{n}.tmp", process::id()));
    Ok(path.with_file_name(temp_name))
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The per-user directory for Ferrix's own data (recovery journals, later settings and state).
///
/// `FERRIX_DATA_DIR` overrides it, e.g. for tests.
pub fn app_data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FERRIX_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support/Ferrix"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Ferrix"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/state")))
            .map(|d| d.join("ferrix"))
    }
}

/// The per-user directory for preferences people may read, edit, back up or sync.
///
/// `FERRIX_DATA_DIR` overrides it too, so one variable isolates everything Ferrix keeps.
pub fn app_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FERRIX_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support/Ferrix"))
    } else if cfg!(windows) {
        // Roaming, so preferences follow the user between machines; state stays local.
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Ferrix"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
            .map(|d| d.join("ferrix"))
    }
}

/// Makes a newly created or renamed file's directory entry durable. Not possible (or needed)
/// on every platform.
pub(crate) fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Ok(dir) = fs::File::open(dir)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferrix-io-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn replaces_contents_and_leaves_no_temporary_files() {
        let dir = scratch_dir("replace");
        let path = dir.join("doc.pdf");
        fs::write(&path, b"old").expect("seed");

        write_atomic(&path, b"new contents").expect("atomic write");

        assert_eq!(fs::read(&path).expect("read"), b"new contents");
        let names: Vec<_> = fs::read_dir(&dir)
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["doc.pdf"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn creates_new_files_too() {
        let dir = scratch_dir("create");
        let path = dir.join("copy.pdf");
        write_atomic(&path, b"%PDF-").expect("atomic write");
        assert_eq!(fs::read(&path).expect("read"), b"%PDF-");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failure_leaves_the_original_untouched() {
        let dir = scratch_dir("fail");
        let missing_dir = dir.join("no-such-dir").join("doc.pdf");
        assert!(write_atomic(&missing_dir, b"x").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stamp_changes_when_the_file_changes() {
        let dir = scratch_dir("stamp");
        let path = dir.join("doc.pdf");
        fs::write(&path, b"one").expect("seed");
        let before = FileStamp::of(&path).expect("stamp");
        fs::write(&path, b"three").expect("modify");
        assert_ne!(FileStamp::of(&path).expect("stamp"), before);
        let _ = fs::remove_dir_all(dir);
    }
}
