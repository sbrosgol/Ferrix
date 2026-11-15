//! Formatted copy and paste for written documents (ADR-0008 phase 4).
//!
//! Copying puts HTML and plain text on the system clipboard, so formatting carries over to other
//! apps. Pasting prefers Ferrix's own copy of what it last put there (lossless: lists, pictures,
//! fonts), then HTML from other apps, then plain text.

use ferrix_compose::{Fragment, fragment_from_html, fragment_to_html};

/// What a paste brings in.
#[derive(Debug)]
pub(crate) enum Pasted {
    Formatted(Fragment),
    Text(String),
}

#[derive(Default)]
pub(crate) struct RichClipboard {
    board: Option<arboard::Clipboard>,
    /// What Ferrix last copied: its plain text (to recognise it) and the formatted original.
    last: Option<(String, Fragment)>,
}

impl std::fmt::Debug for RichClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RichClipboard")
            .field("available", &self.board.is_some())
            .field("holds_copy", &self.last.is_some())
            .finish()
    }
}

impl RichClipboard {
    fn board(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.board.is_none() {
            match arboard::Clipboard::new() {
                Ok(board) => self.board = Some(board),
                Err(e) => tracing::warn!("system clipboard unavailable: {e}"),
            }
        }
        self.board.as_mut()
    }

    /// Copies formatted text. Returns `false` if the system clipboard could not be written, in
    /// which case the caller should fall back to plain text.
    pub(crate) fn copy(&mut self, fragment: Fragment) -> bool {
        let plain = fragment.text();
        let html = fragment_to_html(&fragment);
        let written = self
            .board()
            .is_some_and(|board| board.set().html(html, Some(plain.clone())).is_ok());
        self.last = Some((plain, fragment));
        written
    }

    /// What to paste, or `None` if the system clipboard can't be read.
    pub(crate) fn paste(&mut self) -> Option<Pasted> {
        let last = self.last.clone();
        let board = self.board()?;
        let text = board.get_text().ok();
        if let (Some(text), Some((plain, fragment))) = (&text, last)
            && same_text(text, &plain)
        {
            return Some(Pasted::Formatted(fragment));
        }
        if let Some(fragment) = board.get().html().ok().and_then(|h| fragment_from_html(&h)) {
            return Some(Pasted::Formatted(fragment));
        }
        text.map(Pasted::Text)
    }

    /// Plain text only (Paste and Match Style).
    pub(crate) fn paste_plain(&mut self) -> Option<String> {
        self.board()?.get_text().ok()
    }
}

/// Clipboards may change line endings; compare text without them.
fn same_text(a: &str, b: &str) -> bool {
    let normal = |s: &str| s.replace("\r\n", "\n").replace('\r', "\n");
    normal(a).trim_end() == normal(b).trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_do_not_matter() {
        assert!(same_text("a\r\nb\n", "a\nb"));
        assert!(!same_text("a b", "a\nb"));
    }
}
