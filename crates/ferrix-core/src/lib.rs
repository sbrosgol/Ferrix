//! The Ferrix document model, independent of any UI or PDF engine (architecture §5.4).
//!
//! Editing works on in-memory state: a [`PageList`] for page organization and, with
//! annotations (ADR-0009), a [`DocState`] holding pages and their [`AnnotSet`]. Every change is
//! an edit that returns its own inverse, so [`History`] gets undo/redo for free and nothing
//! touches the file until it is saved.

#![forbid(unsafe_code)]

mod annots;
mod history;
mod pages;
mod state;

pub use annots::{
    AnnotEdit, AnnotFlags, AnnotId, AnnotKind, AnnotOrigin, AnnotProps, AnnotSet, Annotation,
    LineEnd, MAX_COORD, MAX_INK_POINTS, MAX_QUADS, MAX_STROKE_POINTS, MAX_STROKES, MAX_TEXT_BYTES,
    MarkupStyle, NoteIcon, ObjectRef, Rgb,
};
pub use history::History;
pub use pages::{Edit, EditError, PageEntry, PageKey, PageList, SourceId};
pub use state::{DocEdit, DocState, Undoable};
