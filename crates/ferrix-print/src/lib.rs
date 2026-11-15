//! Native printing (ADR-0006): the platform's own print dialog, one implementation per OS.
//!
//! - macOS: AppKit print sheet via PDFKit (vector).
//! - Linux/BSD: xdg-desktop-portal Print, spooled to CUPS (vector).
//! - Windows: `PrintDlgEx`, pages rasterized by the caller's [`PageSource`] (our PDFium engine).

use std::path::PathBuf;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::print_with_sheet;
#[cfg(all(unix, not(target_os = "macos")))]
pub use portal::print_with_portal;
#[cfg(windows)]
pub use windows::print_with_dialog;

/// A document to print. The file must be a complete PDF on disk.
#[derive(Debug, Clone)]
pub struct PrintJob {
    pub title: String,
    pub path: PathBuf,
    pub page_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintOutcome {
    /// The job was handed to the print system.
    Submitted,
    /// A non-blocking print sheet is open; the OS finishes (or cancels) the job.
    SheetOpened,
    Cancelled,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PrintError {
    #[error("printing is not available: {0}")]
    Unavailable(String),
    #[error("the print system reported an error: {0}")]
    Platform(String),
    #[error("could not prepare page {page}: {reason}")]
    Render { page: u32, reason: String },
}

/// An RGBA page image, tightly packed and row-major.
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Supplies page images for raster printing (used on Windows).
pub trait PageSource {
    /// Page size in points, as displayed (crop box, intrinsic rotation applied).
    fn page_size(&mut self, page: u32) -> Result<(f32, f32), String>;

    /// Renders a zero-based page at `scale` pixels per point, rotated 90° clockwise if `rotate`.
    fn render(&mut self, page: u32, scale: f32, rotate: bool) -> Result<Bitmap, String>;
}

/// A window to attach print dialogs to, captured on the UI thread (raw handles are not `Send`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    None,
    Win32(isize),
    X11(u64),
}

impl Parent {
    pub fn of<W: HasWindowHandle + ?Sized>(window: &W) -> Self {
        let Ok(handle) = window.window_handle() else {
            return Self::None;
        };
        // `c_ulong` X11 ids are 32-bit on some targets, so the conversions are not no-ops there.
        #[allow(clippy::useless_conversion)]
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Self::Win32(h.hwnd.get()),
            RawWindowHandle::Xlib(h) => Self::X11(u64::from(h.window)),
            RawWindowHandle::Xcb(h) => Self::X11(u64::from(h.window.get())),
            _ => Self::None,
        }
    }
}

/// Orientation and size for placing a page on paper: rotate the page if its orientation
/// differs from the paper's, and shrink (never enlarge) it to fit the printable area.
///
/// Returns `(rotate, fit)` where `fit` ≤ 1 scales the page's natural size at the device DPI.
pub fn fit_to_paper(page: (f32, f32), printable: (f32, f32)) -> (bool, f32) {
    let rotate = (page.0 > page.1) != (printable.0 > printable.1);
    let (w, h) = if rotate { (page.1, page.0) } else { page };
    let fit = (printable.0 / w).min(printable.1 / h).min(1.0);
    (rotate, fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: (f32, f32) = (612.0, 792.0);

    #[test]
    fn portrait_page_on_portrait_paper_is_unchanged() {
        assert_eq!(fit_to_paper(LETTER, (612.0, 792.0)), (false, 1.0));
    }

    #[test]
    fn landscape_page_rotates_onto_portrait_paper() {
        let (rotate, fit) = fit_to_paper((792.0, 612.0), (612.0, 792.0));
        assert!(rotate);
        assert_eq!(fit, 1.0);
    }

    #[test]
    fn oversized_pages_shrink_but_small_pages_never_grow() {
        // A3 onto Letter's printable area shrinks to the limiting dimension (height).
        let (_, fit) = fit_to_paper((842.0, 1191.0), (576.0, 756.0));
        assert!((fit - 756.0 / 1191.0).abs() < 1e-6);
        let (_, fit) = fit_to_paper((200.0, 300.0), (576.0, 756.0));
        assert_eq!(fit, 1.0);
    }
}
