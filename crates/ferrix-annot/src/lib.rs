//! Annotations in PDF files (ADR-0009): reading them into the [`ferrix_core`] model, drawing
//! their appearance, and writing changes into a [`ferrix_pdf::Revision`].
//!
//! The model and its edits live in `ferrix-core`; this crate is the bridge to PDF objects.
//! Every annotation written here carries its own appearance stream, built from the same
//! [`Appearance`] display list the editor draws on screen.

#![forbid(unsafe_code)]

mod appearance;
pub mod free_text;
mod read;
pub mod stamp;
mod tag;
mod write;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub use appearance::{Appearance, Item, PathEl, Stroke, appearance, note_rect};
pub use read::{read_annotations, read_page};
pub use write::{
    Permission, WriteReport, annotation_permission, verify, write_annotations,
    write_page_annotations,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnnotError {
    #[error(transparent)]
    Pdf(#[from] ferrix_pdf::PdfError),
    #[error("annotations cannot be added to this document: {0}")]
    NotAllowed(String),
    #[error("the written file doesn't hold the expected annotations: {0}")]
    Verify(String),
}

impl From<lopdf::Error> for AnnotError {
    fn from(error: lopdf::Error) -> Self {
        Self::Pdf(error.into())
    }
}

/// A date in PDF format (`D:YYYYMMDDHHmmSSZ`, ISO 32000-2 §7.9.4), always in UTC.
pub fn pdf_date(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A name for `/NM` that is unique in practice: time, process and a counter. Not a secret and
/// not derived from anything about the user.
pub fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ferrix-{nanos:x}-{:x}-{count:x}", std::process::id())
}

/// Whether `doc` is a tagged PDF whose structure tree new annotations can't be added to (it
/// doesn't look as ISO 32000-2 describes); they are then saved untagged.
pub fn structure_tree_blocked(doc: &lopdf::Document) -> bool {
    matches!(tag::Tagger::load(doc), Some(Err(())))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn formats_pdf_dates_in_utc() {
        assert_eq!(pdf_date(UNIX_EPOCH), "D:19700101000000Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
        // 2026-11-18 11:06:40 UTC
        assert_eq!(pdf_date(t), "D:20261118110640Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(pdf_date(leap), "D:20000229000000Z");
    }

    #[test]
    fn unique_names_differ() {
        assert_ne!(unique_name(), unique_name());
    }
}
