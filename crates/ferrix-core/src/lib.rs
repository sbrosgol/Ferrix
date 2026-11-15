//! The Ferrix document model, independent of any UI or PDF engine (architecture §5.4).
//!
//! Editing works on an in-memory [`PageList`]: every change is an [`Edit`] that returns its own
//! inverse, so [`History`] gets undo/redo for free and nothing touches the file until it is saved.

#![forbid(unsafe_code)]

mod history;
mod pages;

pub use history::History;
pub use pages::{Edit, EditError, PageEntry, PageKey, PageList, SourceId};
