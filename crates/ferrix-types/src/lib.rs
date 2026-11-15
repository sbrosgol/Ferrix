//! Shared vocabulary types for Ferrix: identifiers, geometry and page coordinate spaces.
//!
//! # Coordinate spaces
//!
//! - **PDF user space** ([`PdfBox`]): points, origin bottom-left, y up, as written in the file.
//! - **Page space** ([`Rect`], [`Point`]): points, origin at the top-left of the page *as displayed*
//!   (crop box, intrinsic `/Rotate` applied), y down. Everything above the engine boundary uses this.
//! - **Device space** ([`PixelRect`]): integer pixels of a page rendered at a given scale.
//!
//! [`PageTransform`] converts PDF user space into page space.

#![forbid(unsafe_code)]

mod geometry;
mod ids;
mod transform;

pub use geometry::{PdfBox, PixelRect, Point, Rect, Size};
pub use ids::{DocId, PageIndex};
pub use transform::{PageTransform, Rotation};
