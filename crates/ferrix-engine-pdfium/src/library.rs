//! Locating and binding the PDFium dynamic library (one binding per process).

use std::{
    env,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use pdfium_render::prelude::Pdfium;

/// Environment variable naming a directory that contains the PDFium library.
pub const PDFIUM_DIR_ENV: &str = "FERRIX_PDFIUM_DIR";

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();
static BIND_LOCK: Mutex<()> = Mutex::new(());

/// The PDFium library could not be found or loaded.
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct LibraryError {
    pub tried: Vec<PathBuf>,
    pub reason: String,
}

/// Binds PDFium from `dir`, or from the default search path when `dir` is `None`.
///
/// Later calls return the already-bound library regardless of `dir`.
pub(crate) fn bind(dir: Option<&Path>) -> Result<&'static Pdfium, LibraryError> {
    let _guard = BIND_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }

    let candidates = match dir {
        Some(dir) => library_candidates(dir),
        None => search_dirs()
            .iter()
            .flat_map(|d| library_candidates(d))
            .collect(),
    };
    let Some(path) = candidates.iter().find(|p| p.is_file()) else {
        return Err(LibraryError {
            tried: candidates,
            reason: format!(
                "PDFium library not found; run `cargo xtask fetch-pdfium` or set {PDFIUM_DIR_ENV}"
            ),
        });
    };

    tracing::debug!(path = %path.display(), "binding PDFium");
    let bindings = Pdfium::bind_to_library(path).map_err(|e| LibraryError {
        tried: vec![path.clone()],
        reason: e.to_string(),
    })?;
    Ok(PDFIUM.get_or_init(|| Pdfium::new(bindings)))
}

/// Directories searched for PDFium, most specific first.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env::var_os(PDFIUM_DIR_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        // Installed layouts: next to the binary, or a macOS bundle's Frameworks directory.
        dirs.push(exe_dir.join("../Frameworks"));
        dirs.push(exe_dir.clone());
    }
    if cfg!(debug_assertions) {
        // Development checkout: `cargo xtask fetch-pdfium` unpacks here.
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium"));
    }
    dirs
}

/// The PDFium archives keep the library in `lib/` (macOS, Linux) or `bin/` (Windows).
fn library_candidates(dir: &Path) -> Vec<PathBuf> {
    ["", "lib", "bin"]
        .iter()
        .map(|sub| Pdfium::pdfium_platform_library_name_at_path(&dir.join(sub)))
        .collect()
}
