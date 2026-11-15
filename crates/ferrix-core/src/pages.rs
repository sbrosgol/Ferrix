use std::collections::HashSet;

use ferrix_types::{PageIndex, Rotation};
use serde::{Deserialize, Serialize};

/// Which document a page comes from. [`SourceId::BASE`] is the open document itself; others
/// are documents whose pages were inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

impl SourceId {
    pub const BASE: Self = Self(0);
}

/// A page's identity while it moves around; stable across edits, unique within a [`PageList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageKey(u64);

/// One page of the edited document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageEntry {
    pub key: PageKey,
    pub source: SourceId,
    /// The page within its source document.
    pub page: PageIndex,
    /// Rotation added on top of the source page's own rotation.
    pub rotation: Rotation,
}

/// A change to the page list. Applying one returns the edit that undoes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    Rotate {
        keys: Vec<PageKey>,
        by: Rotation,
    },
    Remove {
        keys: Vec<PageKey>,
    },
    /// Inserts pages at positions in the resulting list, in ascending position order.
    Insert {
        pages: Vec<(usize, PageEntry)>,
    },
    /// Puts every page in the given order.
    Reorder {
        order: Vec<PageKey>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownPage(PageKey),
    DuplicatePage(PageKey),
    /// A document must keep at least one page.
    RemovesAllPages,
    InvalidPosition(usize),
    NotAPermutation,
    Empty,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPage(key) => write!(f, "page {key:?} is not in the document"),
            Self::DuplicatePage(key) => write!(f, "page {key:?} is listed twice"),
            Self::RemovesAllPages => f.write_str("a document must keep at least one page"),
            Self::InvalidPosition(at) => write!(f, "position {at} is outside the document"),
            Self::NotAPermutation => f.write_str("the new order must contain every page once"),
            Self::Empty => f.write_str("the edit affects no pages"),
        }
    }
}

impl std::error::Error for EditError {}

/// The pages of the document being edited, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageList {
    entries: Vec<PageEntry>,
    next_key: u64,
}

impl PageList {
    /// The unedited document: its own pages, in order.
    pub fn identity(page_count: u32) -> Self {
        let mut list = Self {
            entries: Vec::new(),
            next_key: 0,
        };
        list.entries = list.new_entries(SourceId::BASE, (0..page_count).map(PageIndex));
        list
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[PageEntry] {
        &self.entries
    }

    pub fn get(&self, index: usize) -> Option<&PageEntry> {
        self.entries.get(index)
    }

    pub fn position(&self, key: PageKey) -> Option<usize> {
        self.entries.iter().position(|e| e.key == key)
    }

    /// True when the list is exactly the base document's pages, unrotated and in order.
    pub fn is_unchanged(&self) -> bool {
        self.entries.iter().enumerate().all(|(i, e)| {
            e.source == SourceId::BASE && e.page.0 as usize == i && e.rotation == Rotation::R0
        })
    }

    /// Allocates fresh entries (not yet in the list) for pages of `source`, e.g. to insert them.
    pub fn new_entries(
        &mut self,
        source: SourceId,
        pages: impl IntoIterator<Item = PageIndex>,
    ) -> Vec<PageEntry> {
        pages
            .into_iter()
            .map(|page| {
                let key = PageKey(self.next_key);
                self.next_key += 1;
                PageEntry {
                    key,
                    source,
                    page,
                    rotation: Rotation::R0,
                }
            })
            .collect()
    }

    /// An edit moving `keys` (kept in their current relative order) so the first lands at
    /// `to`, counted in the list without the moved pages.
    pub fn move_edit(&self, keys: &[PageKey], to: usize) -> Result<Edit, EditError> {
        let moving: HashSet<PageKey> = self.checked_keys(keys)?;
        let (mut block, mut rest): (Vec<_>, Vec<_>) = self
            .entries
            .iter()
            .map(|e| e.key)
            .partition(|k| moving.contains(k));
        let to = to.min(rest.len());
        let tail = rest.split_off(to);
        rest.append(&mut block);
        rest.extend(tail);
        Ok(Edit::Reorder { order: rest })
    }

    /// Applies `edit`, returning its inverse. On error the list is left unchanged.
    pub fn apply(&mut self, edit: &Edit) -> Result<Edit, EditError> {
        match edit {
            Edit::Rotate { keys, by } => {
                let keys_set = self.checked_keys(keys)?;
                for entry in &mut self.entries {
                    if keys_set.contains(&entry.key) {
                        entry.rotation = entry.rotation.then(*by);
                    }
                }
                let back = Rotation::from_degrees(-by.degrees()).unwrap_or_default();
                Ok(Edit::Rotate {
                    keys: keys.clone(),
                    by: back,
                })
            }
            Edit::Remove { keys } => {
                let keys_set = self.checked_keys(keys)?;
                if keys_set.len() == self.entries.len() {
                    return Err(EditError::RemovesAllPages);
                }
                let removed: Vec<(usize, PageEntry)> = self
                    .entries
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(_, e)| keys_set.contains(&e.key))
                    .collect();
                self.entries.retain(|e| !keys_set.contains(&e.key));
                Ok(Edit::Insert { pages: removed })
            }
            Edit::Insert { pages } => {
                if pages.is_empty() {
                    return Err(EditError::Empty);
                }
                let mut seen: HashSet<PageKey> = self.entries.iter().map(|e| e.key).collect();
                let mut previous = None;
                for (i, (at, entry)) in pages.iter().enumerate() {
                    if !seen.insert(entry.key) {
                        return Err(EditError::DuplicatePage(entry.key));
                    }
                    // Positions must ascend and stay within the list as it grows.
                    if previous.is_some_and(|p| *at <= p) || *at > self.entries.len() + i {
                        return Err(EditError::InvalidPosition(*at));
                    }
                    previous = Some(*at);
                }
                for (at, entry) in pages {
                    self.entries.insert(*at, *entry);
                    self.next_key = self.next_key.max(entry.key.0 + 1);
                }
                Ok(Edit::Remove {
                    keys: pages.iter().map(|(_, e)| e.key).collect(),
                })
            }
            Edit::Reorder { order } => {
                let wanted = self.checked_keys(order)?;
                if wanted.len() != self.entries.len() {
                    return Err(EditError::NotAPermutation);
                }
                let previous = self.entries.iter().map(|e| e.key).collect();
                let mut by_key: std::collections::HashMap<PageKey, PageEntry> =
                    self.entries.iter().map(|e| (e.key, *e)).collect();
                self.entries = order.iter().filter_map(|k| by_key.remove(k)).collect();
                Ok(Edit::Reorder { order: previous })
            }
        }
    }

    /// Validates that `keys` is non-empty, has no duplicates, and names only present pages.
    fn checked_keys(&self, keys: &[PageKey]) -> Result<HashSet<PageKey>, EditError> {
        if keys.is_empty() {
            return Err(EditError::Empty);
        }
        let present: HashSet<PageKey> = self.entries.iter().map(|e| e.key).collect();
        let mut set = HashSet::with_capacity(keys.len());
        for &key in keys {
            if !present.contains(&key) {
                return Err(EditError::UnknownPage(key));
            }
            if !set.insert(key) {
                return Err(EditError::DuplicatePage(key));
            }
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(list: &PageList) -> Vec<u32> {
        list.entries().iter().map(|e| e.page.0).collect()
    }

    fn key(list: &PageList, index: usize) -> PageKey {
        list.entries()[index].key
    }

    #[test]
    fn moves_a_selection_as_a_block() {
        let mut list = PageList::identity(6);
        let keys = [key(&list, 1), key(&list, 4)];
        let edit = list.move_edit(&keys, 3).expect("valid move");
        list.apply(&edit).expect("applies");
        assert_eq!(pages(&list), [0, 2, 3, 1, 4, 5]);
    }

    #[test]
    fn remove_and_its_inverse_restore_positions() {
        let mut list = PageList::identity(5);
        let original = list.clone();
        let inverse = list
            .apply(&Edit::Remove {
                keys: vec![key(&list, 4), key(&list, 0), key(&list, 2)],
            })
            .expect("applies");
        assert_eq!(pages(&list), [1, 3]);
        list.apply(&inverse).expect("undo applies");
        assert_eq!(list, original);
    }

    #[test]
    fn rotation_accumulates_and_inverts() {
        let mut list = PageList::identity(2);
        let k = key(&list, 0);
        let edit = Edit::Rotate {
            keys: vec![k],
            by: Rotation::R90,
        };
        list.apply(&edit).expect("applies");
        let inverse = list.apply(&edit).expect("applies");
        assert_eq!(list.entries()[0].rotation, Rotation::R180);
        list.apply(&inverse).expect("applies");
        assert_eq!(list.entries()[0].rotation, Rotation::R90);
        assert!(!list.is_unchanged());
    }

    #[test]
    fn invalid_edits_leave_the_list_untouched() {
        let mut list = PageList::identity(3);
        let before = list.clone();
        let all: Vec<_> = list.entries().iter().map(|e| e.key).collect();
        let k0 = key(&list, 0);

        assert_eq!(
            list.apply(&Edit::Remove { keys: all }),
            Err(EditError::RemovesAllPages)
        );
        assert_eq!(
            list.apply(&Edit::Remove { keys: vec![k0, k0] }),
            Err(EditError::DuplicatePage(k0))
        );
        assert_eq!(
            list.apply(&Edit::Reorder { order: vec![k0] }),
            Err(EditError::NotAPermutation)
        );
        let fresh = list.clone().new_entries(SourceId(1), [PageIndex(0)]);
        assert_eq!(
            list.apply(&Edit::Insert {
                pages: vec![(9, fresh[0])]
            }),
            Err(EditError::InvalidPosition(9))
        );
        assert_eq!(list, before);
    }

    /// Random edit sequences: undoing everything restores the original, redoing restores the
    /// final state.
    #[test]
    fn random_edits_round_trip_through_their_inverses() {
        let mut rng = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move |bound: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % bound.max(1) as u64) as usize
        };

        for _ in 0..200 {
            let mut list = PageList::identity(8);
            let original = list.clone();
            let mut applied = Vec::new();
            for _ in 0..12 {
                let len = list.len();
                let some_keys = |next: &mut dyn FnMut(usize) -> usize| -> Vec<PageKey> {
                    let mut keys: Vec<PageKey> =
                        (0..1 + next(3)).map(|_| key(&list, next(len))).collect();
                    keys.sort();
                    keys.dedup();
                    keys
                };
                let edit = match next(4) {
                    0 => Edit::Rotate {
                        keys: some_keys(&mut next),
                        by: Rotation::from_degrees(90 * (1 + next(3)) as i32).unwrap_or_default(),
                    },
                    1 => Edit::Remove {
                        keys: some_keys(&mut next),
                    },
                    2 => {
                        let entry = list.new_entries(SourceId(1), [PageIndex(next(5) as u32)])[0];
                        Edit::Insert {
                            pages: vec![(next(len + 1), entry)],
                        }
                    }
                    _ => list
                        .move_edit(&some_keys(&mut next), next(len))
                        .expect("valid move"),
                };
                if let Ok(inverse) = list.apply(&edit) {
                    applied.push((edit, inverse));
                }
            }
            let finished = list.clone();
            for (_, inverse) in applied.iter().rev() {
                list.apply(inverse).expect("inverse applies");
            }
            assert_eq!(list.entries(), original.entries());
            for (edit, _) in &applied {
                list.apply(edit).expect("redo applies");
            }
            assert_eq!(list.entries(), finished.entries());
        }
    }
}
