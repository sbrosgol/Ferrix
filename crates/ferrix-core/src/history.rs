use crate::{EditError, PageList, Undoable};

#[derive(Debug, Clone)]
struct Step<E> {
    label: String,
    forward: E,
    inverse: E,
    /// Edits performed with the same group merge into this step (one gesture, one undo).
    group: Option<u64>,
}

/// Undo/redo over an [`Undoable`] state, tracking whether the current state is what's on disk.
///
/// Defaults to page organization ([`PageList`]); documents with annotations use
/// [`crate::DocState`].
#[derive(Debug, Clone)]
pub struct History<T: Undoable = PageList> {
    done: Vec<Step<T::Edit>>,
    undone: Vec<Step<T::Edit>>,
    /// `done.len()` when last saved; `None` once that state can no longer be reached.
    saved_at: Option<usize>,
}

impl<T: Undoable> Default for History<T> {
    fn default() -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            saved_at: Some(0),
        }
    }
}

impl<T: Undoable> History<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A history whose current state was never saved, e.g. edits recovered after a crash.
    pub fn unsaved() -> Self {
        Self {
            saved_at: None,
            ..Self::default()
        }
    }

    /// Applies `edit` and records it under `label` (e.g. "Delete 2 pages").
    pub fn perform(
        &mut self,
        state: &mut T,
        edit: T::Edit,
        label: impl Into<String>,
    ) -> Result<(), EditError> {
        self.record(state, edit, label.into(), None)
    }

    /// Like [`Self::perform`], but merges into the previous step when that step has the same
    /// `group` and the edits combine ([`Undoable::merge`]). Use one group per gesture, such as
    /// a drag, so it undoes in one step. Never merges across a save.
    ///
    /// Group ids must be unique per gesture: after undo and redo, the next edit with the redone
    /// step's group would merge into it.
    pub fn perform_grouped(
        &mut self,
        state: &mut T,
        edit: T::Edit,
        label: impl Into<String>,
        group: u64,
    ) -> Result<(), EditError> {
        let at_save_point = self.saved_at == Some(self.done.len());
        if !at_save_point
            && self.undone.is_empty()
            && let Some(last) = self.done.last_mut()
            && last.group == Some(group)
            && let Some(merged) = T::merge(&last.forward, &edit)
        {
            state.apply_edit(&edit)?;
            last.forward = merged;
            return Ok(());
        }
        self.record(state, edit, label.into(), Some(group))
    }

    fn record(
        &mut self,
        state: &mut T,
        edit: T::Edit,
        label: String,
        group: Option<u64>,
    ) -> Result<(), EditError> {
        let inverse = state.apply_edit(&edit)?;
        if self.saved_at.is_some_and(|saved| saved > self.done.len()) {
            self.saved_at = None; // the saved state was in the discarded redo branch
        }
        self.undone.clear();
        self.done.push(Step {
            label,
            forward: edit,
            inverse,
            group,
        });
        Ok(())
    }

    /// Undoes the latest edit, returning its label.
    pub fn undo(&mut self, state: &mut T) -> Option<String> {
        let step = self.done.pop()?;
        match state.apply_edit(&step.inverse) {
            Ok(_) => {
                let label = step.label.clone();
                self.undone.push(step);
                Some(label)
            }
            Err(_) => {
                // Unreachable while all edits go through this history; keep it consistent anyway.
                self.done.push(step);
                None
            }
        }
    }

    /// Redoes the latest undone edit, returning its label.
    pub fn redo(&mut self, state: &mut T) -> Option<String> {
        let step = self.undone.pop()?;
        match state.apply_edit(&step.forward) {
            Ok(_) => {
                let label = step.label.clone();
                self.done.push(step);
                Some(label)
            }
            Err(_) => {
                self.undone.push(step);
                None
            }
        }
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.done.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.undone.last().map(|s| s.label.as_str())
    }

    /// True when the state differs from the last saved one.
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.done.len())
    }
}

#[cfg(test)]
mod tests {
    use ferrix_types::{PdfBox, PdfPoint, Rotation};

    use super::*;
    use crate::{AnnotEdit, AnnotKind, AnnotSet, Annotation, DocEdit, DocState, Edit};

    fn rotate(pages: &PageList) -> Edit {
        Edit::Rotate {
            keys: vec![pages.entries()[0].key],
            by: Rotation::R90,
        }
    }

    #[test]
    fn dirty_tracks_the_saved_state_through_undo_and_redo() {
        let mut pages = PageList::identity(3);
        let mut history = History::new();
        assert!(!history.is_dirty());

        let edit = rotate(&pages);
        history
            .perform(&mut pages, edit, "Rotate")
            .expect("applies");
        assert!(history.is_dirty());
        assert_eq!(history.undo(&mut pages).as_deref(), Some("Rotate"));
        assert!(!history.is_dirty());
        assert_eq!(history.redo(&mut pages).as_deref(), Some("Rotate"));
        assert!(history.is_dirty());
    }

    #[test]
    fn recovered_state_stays_dirty_until_saved() {
        let history: History = History::unsaved();
        assert!(history.is_dirty());
        assert_eq!(history.undo_label(), None);
    }

    #[test]
    fn a_new_edit_after_undo_drops_the_redo_branch() {
        let mut pages = PageList::identity(3);
        let mut history = History::new();
        let edit = rotate(&pages);
        history.perform(&mut pages, edit, "First").expect("applies");
        history.undo(&mut pages);
        let edit = rotate(&pages);
        history
            .perform(&mut pages, edit, "Second")
            .expect("applies");

        assert_eq!(history.redo_label(), None);
        assert_eq!(history.undo_label(), Some("Second"));
    }

    fn square_at(x: f32) -> Box<Annotation> {
        Box::new(Annotation::new(
            AnnotKind::Square,
            PdfBox::from_corners(PdfPoint::new(x, 10.0), PdfPoint::new(x + 50.0, 60.0)),
        ))
    }

    #[test]
    fn a_drag_merges_into_one_undo_step() {
        let mut state = DocState::new(PageList::identity(1), AnnotSet::new());
        let page = state.pages.entries()[0].key;
        let id = state.annots.new_id();
        let mut history = History::<DocState>::new();
        let add = DocEdit::Annot(AnnotEdit::Add {
            page,
            index: 0,
            id,
            annot: square_at(0.0),
        });
        history
            .perform(&mut state, add, "Add rectangle")
            .expect("adds");
        let placed = state.clone();

        for x in [5.0, 10.0, 15.0] {
            let set = DocEdit::Annot(AnnotEdit::Set {
                id,
                annot: square_at(x),
            });
            history
                .perform_grouped(&mut state, set, "Move", 7)
                .expect("moves");
        }
        let moved = state.clone();

        assert_eq!(history.undo(&mut state).as_deref(), Some("Move"));
        assert_eq!(state, placed, "one undo reverts the whole drag");
        assert_eq!(history.redo(&mut state).as_deref(), Some("Move"));
        assert_eq!(state, moved);
    }

    #[test]
    fn grouped_edits_do_not_merge_across_a_save_point() {
        let mut state = DocState::new(PageList::identity(1), AnnotSet::new());
        let page = state.pages.entries()[0].key;
        let id = state.annots.new_id();
        let mut history = History::<DocState>::new();
        let add = DocEdit::Annot(AnnotEdit::Add {
            page,
            index: 0,
            id,
            annot: square_at(0.0),
        });
        history
            .perform_grouped(&mut state, add, "Add", 1)
            .expect("adds");
        // Simulate a save after the first step.
        let mut saved = History::<DocState> {
            saved_at: Some(1),
            ..history
        };
        let set = DocEdit::Annot(AnnotEdit::Set {
            id,
            annot: square_at(5.0),
        });
        saved
            .perform_grouped(&mut state, set, "Move", 1)
            .expect("moves");
        assert_eq!(saved.done.len(), 2);
    }
}
