//! The whole edited state of a PDF: its pages and their annotations, changed by [`DocEdit`]s.

use serde::{Deserialize, Serialize};

use crate::{AnnotEdit, AnnotSet, Edit, EditError, PageList};

/// Something [`crate::History`] can undo: applying an edit returns its exact inverse.
pub trait Undoable {
    type Edit: Clone + std::fmt::Debug;

    /// Applies `edit`, returning its inverse. On error the state is left unchanged.
    fn apply_edit(&mut self, edit: &Self::Edit) -> Result<Self::Edit, EditError>;

    /// One edit doing `earlier` then `later`, if they combine into a single undo step (such as
    /// successive moves while dragging). The inverse of `earlier` must still undo the result.
    fn merge(_earlier: &Self::Edit, _later: &Self::Edit) -> Option<Self::Edit> {
        None
    }
}

impl Undoable for PageList {
    type Edit = Edit;

    fn apply_edit(&mut self, edit: &Edit) -> Result<Edit, EditError> {
        self.apply(edit)
    }
}

/// A change to a [`DocState`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DocEdit {
    Pages(Edit),
    Annot(AnnotEdit),
    /// Several edits applied in order, all or nothing (e.g. deleting a selection).
    Batch(Vec<DocEdit>),
}

impl From<Edit> for DocEdit {
    fn from(edit: Edit) -> Self {
        Self::Pages(edit)
    }
}

impl From<AnnotEdit> for DocEdit {
    fn from(edit: AnnotEdit) -> Self {
        Self::Annot(edit)
    }
}

/// The pages of the document being edited and their annotations (ADR-0003, ADR-0009).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocState {
    pub pages: PageList,
    pub annots: AnnotSet,
}

impl DocState {
    pub fn new(pages: PageList, annots: AnnotSet) -> Self {
        Self { pages, annots }
    }

    /// Applies `edit`, returning its inverse. On error the state is left unchanged.
    pub fn apply(&mut self, edit: &DocEdit) -> Result<DocEdit, EditError> {
        match edit {
            DocEdit::Pages(edit) => self.pages.apply(edit).map(DocEdit::Pages),
            DocEdit::Annot(edit) => {
                // Annotations of removed pages are frozen: their page is gone from the document,
                // and an inverse `Add` onto a missing page could not be applied.
                let page = match edit {
                    AnnotEdit::Add { page, .. } => Some(*page),
                    AnnotEdit::Remove { id } | AnnotEdit::Set { id, .. } => {
                        self.annots.get(*id).map(|(page, _)| page)
                    }
                };
                if let Some(page) = page
                    && self.pages.position(page).is_none()
                {
                    return Err(EditError::UnknownPage(page));
                }
                self.annots.apply(edit).map(DocEdit::Annot)
            }
            DocEdit::Batch(edits) => {
                if edits.is_empty() {
                    return Err(EditError::Empty);
                }
                let mut inverses = Vec::with_capacity(edits.len());
                for edit in edits {
                    match self.apply(edit) {
                        Ok(inverse) => inverses.push(inverse),
                        Err(error) => {
                            for inverse in inverses.iter().rev() {
                                // Each inverse was produced against exactly this state, so it
                                // applies; ignoring a failure here would only hide a model bug.
                                let undone = self.apply(inverse);
                                debug_assert!(undone.is_ok(), "rollback failed: {undone:?}");
                            }
                            return Err(error);
                        }
                    }
                }
                inverses.reverse();
                Ok(DocEdit::Batch(inverses))
            }
        }
    }
}

impl Undoable for DocState {
    type Edit = DocEdit;

    fn apply_edit(&mut self, edit: &DocEdit) -> Result<DocEdit, EditError> {
        self.apply(edit)
    }

    /// Successive `Set`s of one annotation, or an `Add` followed by `Set`s of the same
    /// annotation, collapse into the last state.
    fn merge(earlier: &DocEdit, later: &DocEdit) -> Option<DocEdit> {
        let DocEdit::Annot(AnnotEdit::Set { id, annot }) = later else {
            return None;
        };
        match earlier {
            DocEdit::Annot(AnnotEdit::Set { id: first, .. }) if first == id => Some(later.clone()),
            DocEdit::Annot(AnnotEdit::Add {
                page,
                index,
                id: first,
                ..
            }) if first == id => Some(DocEdit::Annot(AnnotEdit::Add {
                page: *page,
                index: *index,
                id: *id,
                annot: annot.clone(),
            })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrix_types::{PageIndex, PdfBox, PdfPoint, Rotation};

    use super::*;
    use crate::{
        AnnotFlags, AnnotId, AnnotKind, AnnotOrigin, Annotation, LineEnd, NoteIcon, ObjectRef,
        PageKey, SourceId,
    };

    struct Rng(u64);

    impl Rng {
        fn next(&mut self, bound: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % bound.max(1) as u64) as usize
        }

        fn coord(&mut self) -> f32 {
            self.next(600) as f32 + 0.5
        }
    }

    fn random_annot(rng: &mut Rng) -> Annotation {
        let (a, b) = (
            PdfPoint::new(rng.coord(), rng.coord()),
            PdfPoint::new(rng.coord(), rng.coord()),
        );
        let kind = match rng.next(5) {
            0 => AnnotKind::Note {
                icon: NoteIcon::Comment,
                open: false,
            },
            1 => AnnotKind::Ink {
                strokes: vec![vec![a, b], vec![b]],
            },
            2 => AnnotKind::Square,
            3 => AnnotKind::Circle,
            _ => AnnotKind::Line {
                from: a,
                to: b,
                ends: [LineEnd::None, LineEnd::OpenArrow],
            },
        };
        let mut annot = Annotation::new(kind, PdfBox::from_corners(a, b));
        annot.props.opacity = rng.next(11) as f32 / 10.0;
        annot.props.contents = (rng.next(2) == 0).then(|| format!("note {}", rng.next(100)));
        annot
    }

    /// A document with three pages, each holding one annotation "from the file".
    fn loaded_state() -> DocState {
        let pages = PageList::identity(3);
        let mut annots = AnnotSet::new();
        for (i, entry) in pages.entries().iter().enumerate() {
            let mut annot = Annotation::new(AnnotKind::Square, PdfBox::default());
            annot.origin = AnnotOrigin::Existing {
                source: SourceId::BASE,
                object: ObjectRef {
                    number: 10 + i as u32,
                    generation: 0,
                },
            };
            annots.insert_loaded(entry.key, [annot]);
        }
        DocState::new(pages, annots)
    }

    fn without_counters(state: &DocState) -> DocState {
        let mut state = state.clone();
        state.pages.next_key = 0;
        state.annots.next_id = 0;
        state
    }

    fn all_ids(state: &DocState) -> Vec<(PageKey, AnnotId)> {
        state
            .annots
            .pages()
            .flat_map(|(page, list)| list.iter().map(move |(id, _)| (page, *id)))
            .collect()
    }

    fn random_edit(state: &mut DocState, rng: &mut Rng) -> DocEdit {
        let len = state.pages.len();
        let page = state.pages.entries()[rng.next(len)].key;
        let ids = all_ids(state);
        match rng.next(8) {
            0 => DocEdit::Pages(Edit::Rotate {
                keys: vec![page],
                by: Rotation::R90,
            }),
            1 => DocEdit::Pages(Edit::Remove { keys: vec![page] }),
            2 => {
                let entry = state
                    .pages
                    .new_entries(SourceId(1), [PageIndex(rng.next(4) as u32)])[0];
                DocEdit::Pages(Edit::Insert {
                    pages: vec![(rng.next(len + 1), entry)],
                })
            }
            3 => state
                .pages
                .move_edit(&[page], rng.next(len))
                .map(DocEdit::Pages)
                .unwrap_or(DocEdit::Batch(Vec::new())),
            4 | 5 => {
                // Sometimes one past the end, which must fail without changing anything.
                let index = rng.next(state.annots.page(page).len() + 2);
                DocEdit::Annot(AnnotEdit::Add {
                    page,
                    index,
                    id: state.annots.new_id(),
                    annot: Box::new(random_annot(rng)),
                })
            }
            6 if !ids.is_empty() => DocEdit::Annot(AnnotEdit::Remove {
                id: ids[rng.next(ids.len())].1,
            }),
            6 => DocEdit::Batch(Vec::new()),
            _ if !ids.is_empty() => {
                let id = ids[rng.next(ids.len())].1;
                let mut annot = state
                    .annots
                    .get(id)
                    .map(|(_, a)| a.clone())
                    .expect("present");
                let replacement = random_annot(rng);
                // Often a different type or `Other`, which `Set` must refuse.
                annot.kind = match rng.next(6) {
                    0 => AnnotKind::Other {
                        subtype: "Square".into(),
                    },
                    _ => replacement.kind,
                };
                annot.props = replacement.props;
                let set = DocEdit::Annot(AnnotEdit::Set {
                    id,
                    annot: Box::new(annot),
                });
                // Sometimes wrap with a removal that fails, to exercise batch rollback.
                if rng.next(4) == 0 {
                    DocEdit::Batch(vec![
                        set,
                        DocEdit::Annot(AnnotEdit::Remove {
                            id: AnnotId(u64::MAX),
                        }),
                    ])
                } else {
                    set
                }
            }
            _ => DocEdit::Batch(Vec::new()),
        }
    }

    /// Random mixed sequences: undoing everything restores the original exactly; redoing
    /// restores the final state; failed edits change nothing.
    #[test]
    fn random_doc_edits_round_trip_through_their_inverses() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for _ in 0..300 {
            let mut state = loaded_state();
            let original = state.clone();
            let mut applied = Vec::new();
            for _ in 0..16 {
                let edit = random_edit(&mut state, &mut rng);
                let before = state.clone();
                match state.apply(&edit) {
                    Ok(inverse) => applied.push((edit, inverse)),
                    Err(_) => assert_eq!(
                        state, before,
                        "a failed edit must leave the state unchanged: {edit:?}"
                    ),
                }
            }
            let finished = state.clone();
            for (_, inverse) in applied.iter().rev() {
                state.apply(inverse).expect("inverse applies");
            }
            // Ids and page keys are allocated outside edits, so only the counters may differ.
            assert_eq!(without_counters(&state), without_counters(&original));
            for (edit, _) in &applied {
                state.apply(edit).expect("redo applies");
            }
            assert_eq!(state, finished);
        }
    }

    #[test]
    fn annotations_follow_their_page_through_removal_and_undo() {
        let mut state = loaded_state();
        let page = state.pages.entries()[1].key;
        let before = state.annots.page(page).to_vec();
        let undo = state
            .apply(&DocEdit::Pages(Edit::Remove { keys: vec![page] }))
            .expect("removes");
        assert_eq!(
            state.annots.page(page),
            before.as_slice(),
            "kept while removed"
        );
        // A removed page can't receive new annotations.
        let add = DocEdit::Annot(AnnotEdit::Add {
            page,
            index: 0,
            id: state.annots.new_id(),
            annot: Box::new(Annotation::new(AnnotKind::Square, PdfBox::default())),
        });
        assert_eq!(state.apply(&add), Err(EditError::UnknownPage(page)));
        state.apply(&undo).expect("restores");
        assert_eq!(state.pages.position(page), Some(1));
        assert_eq!(state.annots.page(page), before.as_slice());
    }

    #[test]
    fn foreign_annotations_of_unknown_type_cannot_be_rewritten() {
        let mut state = loaded_state();
        let page = state.pages.entries()[0].key;
        let mut other = Annotation::new(
            AnnotKind::Other {
                subtype: "FreeText".into(),
            },
            PdfBox::default(),
        );
        let id = state.annots.new_id();
        assert_eq!(
            state.apply(&DocEdit::Annot(AnnotEdit::Add {
                page,
                index: 0,
                id,
                annot: Box::new(other.clone()),
            })),
            Err(EditError::InvalidAnnotation("unsupported type")),
            "Ferrix can't create a type it doesn't model"
        );
        other.origin = AnnotOrigin::Existing {
            source: SourceId::BASE,
            object: ObjectRef {
                number: 99,
                generation: 0,
            },
        };
        let id = state.annots.insert_loaded(page, [other.clone()])[0];
        other.props.opacity = 0.5;
        assert_eq!(
            state.apply(&DocEdit::Annot(AnnotEdit::Set {
                id,
                annot: Box::new(other),
            })),
            Err(EditError::InvalidAnnotation("this type cannot be edited"))
        );
        // Deleting is fine.
        let undo = state
            .apply(&DocEdit::Annot(AnnotEdit::Remove { id }))
            .expect("removes");
        state.apply(&undo).expect("restores");
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut state = loaded_state();
        let page = state.pages.entries()[0].key;
        let mut annot = Annotation::new(AnnotKind::Ink { strokes: vec![] }, PdfBox::default());
        let add = |annot: &Annotation, state: &mut DocState| {
            let id = state.annots.new_id();
            state.apply(&DocEdit::Annot(AnnotEdit::Add {
                page,
                index: 0,
                id,
                annot: Box::new(annot.clone()),
            }))
        };
        assert_eq!(
            add(&annot, &mut state),
            Err(EditError::InvalidAnnotation("ink strokes"))
        );
        annot.kind = AnnotKind::Square;
        annot.props.opacity = 1.5;
        assert_eq!(
            add(&annot, &mut state),
            Err(EditError::InvalidAnnotation("opacity"))
        );
        annot.props.opacity = 1.0;
        annot.props.rect.left = f32::NAN;
        assert_eq!(
            add(&annot, &mut state),
            Err(EditError::InvalidAnnotation("rectangle"))
        );
    }

    #[test]
    fn audit_regressions() {
        let mut state = loaded_state();
        let page = state.pages.entries()[0].key;
        let (square_id, square) = state.annots.page(page)[0].clone();

        // A Set can't change the type, in particular not to the read-only `Other`.
        let mut other = square.clone();
        other.kind = AnnotKind::Other {
            subtype: "Square".into(),
        };
        let mut circle = square.clone();
        circle.kind = AnnotKind::Circle;
        for annot in [other, circle] {
            assert!(
                state
                    .apply(&DocEdit::Annot(AnnotEdit::Set {
                        id: square_id,
                        annot: Box::new(annot),
                    }))
                    .is_err()
            );
        }

        // A failed Add on an empty page leaves no trace.
        let empty = state.pages.new_entries(SourceId(1), [PageIndex(0)])[0];
        state
            .apply(&DocEdit::Pages(Edit::Insert {
                pages: vec![(0, empty)],
            }))
            .expect("inserts");
        let before = state.clone();
        let id = state.annots.new_id();
        let before_with_id = state.clone();
        let add = |index| {
            DocEdit::Annot(AnnotEdit::Add {
                page: empty.key,
                index,
                id,
                annot: Box::new(Annotation::new(AnnotKind::Square, PdfBox::default())),
            })
        };
        assert_eq!(state.apply(&add(1)), Err(EditError::InvalidPosition(1)));
        assert_eq!(state, before_with_id);
        assert_ne!(state, before, "only the reserved id differs");

        // Ids must be reserved, so no edit moves the counter; huge ids are refused.
        for id in [AnnotId(state.annots.next_id), AnnotId(u64::MAX)] {
            let edit = DocEdit::Annot(AnnotEdit::Add {
                page,
                index: 0,
                id,
                annot: Box::new(Annotation::new(AnnotKind::Square, PdfBox::default())),
            });
            assert_eq!(
                state.apply(&edit),
                Err(EditError::InvalidAnnotation("id was not reserved"))
            );
        }
        let undo = state.apply(&add(0)).expect("adds");
        state.apply(&undo).expect("removes");
        assert_eq!(state, before_with_id, "Add then its inverse is exact");

        // Locked annotations from the file can't be changed or deleted, and locks can't change.
        let mut locked = square.clone();
        locked.props.flags = AnnotFlags(AnnotFlags::PRINT | AnnotFlags::LOCKED);
        let locked_id = state.annots.insert_loaded(page, [locked.clone()])[0];
        assert!(
            !state
                .annots
                .get(locked_id)
                .expect("present")
                .1
                .is_deletable()
        );
        assert_eq!(
            state.apply(&DocEdit::Annot(AnnotEdit::Remove { id: locked_id })),
            Err(EditError::InvalidAnnotation("locked"))
        );
        let mut unlocked = square.clone();
        unlocked.props.opacity = 0.5;
        unlocked.props.flags = AnnotFlags(AnnotFlags::PRINT | AnnotFlags::LOCKED);
        let mut set_lock = DocEdit::Annot(AnnotEdit::Set {
            id: square_id,
            annot: Box::new(unlocked),
        });
        assert_eq!(
            state.apply(&set_lock),
            Err(EditError::InvalidAnnotation("lock flags cannot change"))
        );
        if let DocEdit::Annot(AnnotEdit::Set { annot, .. }) = &mut set_lock {
            annot.props.flags = square.props.flags;
        }
        state
            .apply(&set_lock)
            .expect("unlocked annotations can change");
    }

    #[test]
    fn loaded_annotations_are_sanitized_so_undo_always_works() {
        let mut state = loaded_state();
        let page = state.pages.entries()[0].key;
        let mut odd = Annotation::new(
            AnnotKind::Other {
                subtype: String::new(),
            },
            PdfBox {
                left: 0.0,
                bottom: 0.0,
                right: 3.0e38,
                top: 10.0,
            },
        );
        odd.origin = AnnotOrigin::Existing {
            source: SourceId::BASE,
            object: ObjectRef {
                number: 77,
                generation: 0,
            },
        };
        odd.props.dash = vec![0.0, 0.0];
        odd.props.opacity = f32::NAN;
        let id = state.annots.insert_loaded(page, [odd])[0];
        let undo = state
            .apply(&DocEdit::Annot(AnnotEdit::Remove { id }))
            .expect("removes");
        state.apply(&undo).expect("undo of the removal applies");
        let (_, kept) = state.annots.get(id).expect("restored");
        assert_eq!(kept.kind.subtype(), "Unknown");
        assert!(kept.props.dash.is_empty() && kept.props.opacity == 1.0);
    }

    #[test]
    fn rejects_values_other_readers_choke_on() {
        let mut state = loaded_state();
        let page = state.pages.entries()[0].key;
        let mut try_add = |annot: Annotation| {
            let id = state.annots.new_id();
            state.apply(&DocEdit::Annot(AnnotEdit::Add {
                page,
                index: 0,
                id,
                annot: Box::new(annot),
            }))
        };
        let mut a = Annotation::new(AnnotKind::Square, PdfBox::default());
        a.props.dash = vec![0.0, 0.0];
        assert!(try_add(a).is_err(), "all-zero dash");
        let a = Annotation::new(
            AnnotKind::Square,
            PdfBox {
                left: -3.0e38,
                bottom: 0.0,
                right: 3.0e38,
                top: 1.0,
            },
        );
        assert!(try_add(a).is_err(), "huge coordinates");
        let a = Annotation::new(
            AnnotKind::Note {
                icon: NoteIcon::Other("bad name".into()),
                open: false,
            },
            PdfBox::default(),
        );
        assert!(try_add(a).is_err(), "icon name with a space");
        assert_eq!(NoteIcon::from_name("bad/name"), NoteIcon::Note);
        assert_eq!(NoteIcon::from_name("Star"), NoteIcon::Other("Star".into()));
    }
}
