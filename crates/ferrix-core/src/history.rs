use crate::{Edit, EditError, PageList};

#[derive(Debug, Clone)]
struct Step {
    label: String,
    forward: Edit,
    inverse: Edit,
}

/// Undo/redo over a [`PageList`], tracking whether the current state is what's on disk.
#[derive(Debug, Clone)]
pub struct History {
    done: Vec<Step>,
    undone: Vec<Step>,
    /// `done.len()` when last saved; `None` once that state can no longer be reached.
    saved_at: Option<usize>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            saved_at: Some(0),
        }
    }
}

impl History {
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
        pages: &mut PageList,
        edit: Edit,
        label: impl Into<String>,
    ) -> Result<(), EditError> {
        let inverse = pages.apply(&edit)?;
        if self.saved_at.is_some_and(|saved| saved > self.done.len()) {
            self.saved_at = None; // the saved state was in the discarded redo branch
        }
        self.undone.clear();
        self.done.push(Step {
            label: label.into(),
            forward: edit,
            inverse,
        });
        Ok(())
    }

    /// Undoes the latest edit, returning its label.
    pub fn undo(&mut self, pages: &mut PageList) -> Option<String> {
        let step = self.done.pop()?;
        match pages.apply(&step.inverse) {
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
    pub fn redo(&mut self, pages: &mut PageList) -> Option<String> {
        let step = self.undone.pop()?;
        match pages.apply(&step.forward) {
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

    /// True when the pages differ from the last saved state.
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.done.len())
    }
}

#[cfg(test)]
mod tests {
    use ferrix_types::Rotation;

    use super::*;

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
        let history = History::unsaved();
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
}
