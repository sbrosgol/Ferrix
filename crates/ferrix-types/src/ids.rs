use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies an open document within one engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocId(pub u64);

/// Zero-based page index. Displayed page numbers are `index + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageIndex(pub u32);

impl PageIndex {
    /// Builds an index from a one-based page number, as typed by users.
    pub fn from_number(number: u32) -> Option<Self> {
        number.checked_sub(1).map(Self)
    }

    pub fn number(self) -> u32 {
        self.0 + 1
    }
}

impl fmt::Display for PageIndex {
    /// Formats as the user-facing one-based page number.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.number())
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "doc#{}", self.0)
    }
}
