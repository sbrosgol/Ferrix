//! Edits to a [`Document`] (ADR-0008 §3). Applying an edit returns the edit that undoes it.

use serde::{Deserialize, Serialize};

use crate::model::{
    CharFormat, Document, FontChoice, PageSetup, PageText, Paragraph, ParagraphFormat,
    ParagraphStyle, Position, TextRange,
};

/// Paragraphs that replace a range of text. The first is joined to the text before the range,
/// the last to the text after it. Each resulting paragraph takes the style and formatting of its
/// fragment paragraph. Never empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fragment(Vec<Paragraph>);

impl Fragment {
    pub fn new(paragraphs: Vec<Paragraph>) -> Option<Self> {
        (!paragraphs.is_empty()).then_some(Self(paragraphs))
    }

    /// A single paragraph.
    pub fn single(paragraph: Paragraph) -> Self {
        Self(vec![paragraph])
    }

    pub fn paragraphs(&self) -> &[Paragraph] {
        &self.0
    }

    pub fn text(&self) -> String {
        self.0
            .iter()
            .map(Paragraph::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The same text with every run's format replaced.
    pub(crate) fn map_formats(&self, f: impl Fn(&CharFormat) -> CharFormat) -> Self {
        Self(
            self.0
                .iter()
                .map(|p| {
                    let mut out = p.empty_like();
                    for run in &p.runs {
                        out.push(&run.text, f(&run.format));
                    }
                    out
                })
                .collect(),
        )
    }

    /// Where the fragment ends once inserted at `start`.
    fn end_from(&self, start: Position) -> Position {
        let last = self.0.len() - 1;
        let base = if last == 0 { start.offset } else { 0 };
        Position::new(start.block + last, base + self.0[last].len())
    }
}

/// Document-wide settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub title: Option<String>,
    pub page: PageSetup,
    pub font: FontChoice,
    #[serde(default)]
    pub header: Option<PageText>,
    #[serde(default)]
    pub footer: Option<PageText>,
    #[serde(default)]
    pub archival: bool,
}

/// A change to a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    /// Several edits applied in order, as one (Replace All).
    Batch(Vec<Edit>),
    /// Replaces the text in `range` with `with`. Covers typing, deleting, splitting and joining
    /// paragraphs, pasting, and formatting (replacing text with a reformatted copy of itself).
    Replace {
        range: TextRange,
        with: Fragment,
    },
    Settings(Settings),
}

/// The result of applying an edit.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Undoes the edit.
    pub inverse: Edit,
    /// The text the edit put in place (empty for a deletion; the start of the document for a
    /// settings change).
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("position {0:?} is outside the document")]
    OutOfRange(Position),
    #[error("position {0:?} is inside a character")]
    NotOnBoundary(Position),
    #[error("range ends before it starts")]
    Reversed,
    #[error(transparent)]
    Invalid(#[from] crate::model::ModelError),
}

impl Document {
    /// Applies an edit and returns its inverse. On error the document is unchanged.
    pub fn apply(&mut self, edit: &Edit) -> Result<Applied, EditError> {
        match edit {
            Edit::Replace { range, with } => self.replace(*range, with),
            Edit::Batch(edits) => {
                let mut inverses = Vec::with_capacity(edits.len());
                let mut range = TextRange::caret(Position::START);
                for edit in edits {
                    match self.apply(edit) {
                        Ok(applied) => {
                            inverses.push(applied.inverse);
                            range = applied.range;
                        }
                        Err(e) => {
                            // Leave the document as it was.
                            for inverse in inverses.iter().rev() {
                                let _ = self.apply(inverse);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Applied {
                    inverse: Edit::Batch(inverses),
                    range,
                })
            }
            Edit::Settings(settings) => {
                let mut next = self.clone();
                next.title.clone_from(&settings.title);
                next.page = settings.page;
                next.font.clone_from(&settings.font);
                next.header.clone_from(&settings.header);
                next.footer.clone_from(&settings.footer);
                next.archival = settings.archival;
                next.validate()?;
                let inverse = Edit::Settings(self.settings());
                *self = next;
                Ok(Applied {
                    inverse,
                    range: TextRange::caret(Position::START),
                })
            }
        }
    }

    pub fn settings(&self) -> Settings {
        Settings {
            title: self.title.clone(),
            page: self.page,
            font: self.font.clone(),
            header: self.header.clone(),
            footer: self.footer.clone(),
            archival: self.archival,
        }
    }

    /// Checks that a position is in the document and on a character boundary.
    pub fn check(&self, at: Position) -> Result<(), EditError> {
        let paragraph = self
            .paragraphs
            .get(at.block)
            .ok_or(EditError::OutOfRange(at))?;
        if at.offset > paragraph.len() {
            return Err(EditError::OutOfRange(at));
        }
        if !paragraph.is_boundary(at.offset) {
            return Err(EditError::NotOnBoundary(at));
        }
        Ok(())
    }

    fn check_range(&self, range: TextRange) -> Result<(), EditError> {
        self.check(range.start)?;
        self.check(range.end)?;
        if range.end < range.start {
            return Err(EditError::Reversed);
        }
        Ok(())
    }

    /// A copy of the text in `range`, each paragraph keeping its style and formatting.
    pub fn extract(&self, range: TextRange) -> Result<Fragment, EditError> {
        self.check_range(range)?;
        let (s, e) = (range.start, range.end);
        let paragraphs = if s.block == e.block {
            vec![self.paragraphs[s.block].slice(s.offset..e.offset)]
        } else {
            let first = &self.paragraphs[s.block];
            let mut out = vec![first.slice(s.offset..first.len())];
            out.extend(self.paragraphs[s.block + 1..e.block].iter().cloned());
            out.push(self.paragraphs[e.block].slice(0..e.offset));
            out
        };
        Ok(Fragment(paragraphs))
    }

    fn replace(&mut self, range: TextRange, with: &Fragment) -> Result<Applied, EditError> {
        let removed = self.extract(range)?;
        let (s, e) = (range.start, range.end);
        let first = &self.paragraphs[s.block];
        let last = &self.paragraphs[e.block];
        let prefix = first.slice(0..s.offset);
        let suffix = last.slice(e.offset..last.len());

        let count = with.0.len();
        let mut replacement = Vec::with_capacity(count);
        for (k, part) in with.0.iter().enumerate() {
            let mut paragraph = part.empty_like();
            if k == 0 {
                paragraph.append(prefix.clone());
            }
            paragraph.append(part.clone());
            if k == count - 1 {
                paragraph.append(suffix.clone());
            }
            replacement.push(paragraph);
        }
        self.paragraphs.splice(s.block..=e.block, replacement);

        let end = with.end_from(s);
        Ok(Applied {
            inverse: Edit::Replace {
                range: TextRange { start: s, end },
                with: removed,
            },
            range: TextRange { start: s, end },
        })
    }

    // Edits for common editor actions. Each returns an edit to apply; none changes the document.

    /// Types `text` over `range`. Line breaks start new paragraphs.
    pub fn insert_text(&self, range: TextRange, text: &str) -> Result<Edit, EditError> {
        self.check_range(range)?;
        let paragraph = &self.paragraphs[range.start.block];
        let format = paragraph.format_at(range.start.offset);
        let normalized = text.replace("\r\n", "\n");
        let mut lines = normalized.split(['\n', '\r']).peekable();
        let mut parts = Vec::new();
        while let Some(line) = lines.next() {
            let style = if parts.is_empty() {
                paragraph.empty_like()
            } else if lines.peek().is_some() {
                paragraph.continuation()
            } else {
                self.paragraph_after(range)
            };
            parts.push(style.with_text(line, format.clone()));
        }
        Ok(Edit::Replace {
            range,
            with: Fragment(parts),
        })
    }

    /// Splits the paragraph at the caret (the Enter key), replacing any selected text.
    pub fn split_paragraph(&self, range: TextRange) -> Result<Edit, EditError> {
        self.insert_text(range, "\n")
    }

    /// Deletes `range`; the joined paragraph keeps the first paragraph's style.
    pub fn delete(&self, range: TextRange) -> Result<Edit, EditError> {
        self.check_range(range)?;
        let keep = self.paragraphs[range.start.block].empty_like();
        Ok(Edit::Replace {
            range,
            with: Fragment::single(keep),
        })
    }

    /// Changes the character format of the text in `range`.
    /// The callback also gets the style of the paragraph the text is in.
    pub fn format_text(
        &self,
        range: TextRange,
        change: impl Fn(&mut CharFormat, ParagraphStyle),
    ) -> Result<Edit, EditError> {
        let mut fragment = self.extract(range)?;
        for paragraph in &mut fragment.0 {
            let style = paragraph.style;
            let runs = std::mem::take(&mut paragraph.runs);
            for mut run in runs {
                change(&mut run.format, style);
                paragraph.push(&run.text, run.format);
            }
        }
        Ok(Edit::Replace {
            range,
            with: fragment,
        })
    }

    /// Changes the style and paragraph format of every paragraph that `range` touches.
    pub fn format_paragraphs(
        &self,
        range: TextRange,
        change: impl Fn(&mut ParagraphStyle, &mut ParagraphFormat),
    ) -> Result<Edit, EditError> {
        let mut fragment = self.extract(range)?;
        for paragraph in &mut fragment.0 {
            change(&mut paragraph.style, &mut paragraph.format);
        }
        Ok(Edit::Replace {
            range,
            with: fragment,
        })
    }

    /// The paragraph that Enter at the end of `range` starts: a heading is followed by Normal
    /// text, anything else continues as it was.
    fn paragraph_after(&self, range: TextRange) -> Paragraph {
        let paragraph = &self.paragraphs[range.end.block];
        let at_end = range.end.offset == paragraph.len();
        if at_end && paragraph.style.is_heading() {
            Paragraph::new(paragraph.style.next())
        } else {
            self.paragraphs[range.start.block].continuation()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::Align;

    fn doc(paragraphs: &[&str]) -> Document {
        let mut doc = Document::new();
        doc.paragraphs = paragraphs
            .iter()
            .map(|t| Paragraph::new(ParagraphStyle::Normal).with_text(t, CharFormat::default()))
            .collect();
        doc
    }

    fn texts(doc: &Document) -> Vec<String> {
        doc.paragraphs.iter().map(Paragraph::text).collect()
    }

    fn at(block: usize, offset: usize) -> Position {
        Position::new(block, offset)
    }

    fn caret(block: usize, offset: usize) -> TextRange {
        TextRange::caret(at(block, offset))
    }

    #[test]
    fn typing_inserts_text_with_the_preceding_format() {
        let mut d = doc(&["Hello"]);
        let bold = CharFormat {
            bold: Some(true),
            ..CharFormat::default()
        };
        d.paragraphs[0] = Paragraph::new(ParagraphStyle::Normal).with_text("Hello", bold.clone());
        let edit = d.insert_text(caret(0, 5), ", world").unwrap();
        let applied = d.apply(&edit).unwrap();
        assert_eq!(texts(&d), ["Hello, world"]);
        assert_eq!(d.paragraphs[0].runs.len(), 1);
        assert_eq!(d.paragraphs[0].runs[0].format, bold);
        assert_eq!(applied.range, TextRange::between(at(0, 5), at(0, 12)));
    }

    #[test]
    fn enter_splits_and_backspace_joins() {
        let mut d = doc(&["HelloWorld"]);
        let split = d.split_paragraph(caret(0, 5)).unwrap();
        let applied = d.apply(&split).unwrap();
        assert_eq!(texts(&d), ["Hello", "World"]);
        assert_eq!(applied.range.end, at(1, 0));

        let join = d.delete(TextRange::between(at(0, 5), at(1, 0))).unwrap();
        d.apply(&join).unwrap();
        assert_eq!(texts(&d), ["HelloWorld"]);
    }

    #[test]
    fn enter_after_a_heading_starts_normal_text() {
        let mut d = doc(&["Intro"]);
        d.paragraphs[0].style = ParagraphStyle::Heading1;
        d.apply(&d.split_paragraph(caret(0, 5)).unwrap()).unwrap();
        assert_eq!(d.paragraphs[0].style, ParagraphStyle::Heading1);
        assert_eq!(d.paragraphs[1].style, ParagraphStyle::Normal);

        // Splitting inside a heading keeps both halves headings.
        let mut d = doc(&["Intro"]);
        d.paragraphs[0].style = ParagraphStyle::Heading1;
        d.apply(&d.split_paragraph(caret(0, 2)).unwrap()).unwrap();
        assert_eq!(d.paragraphs[1].style, ParagraphStyle::Heading1);
    }

    #[test]
    fn pasting_lines_creates_paragraphs() {
        let mut d = doc(&["AB"]);
        d.apply(&d.insert_text(caret(0, 1), "1\r\n2\n3").unwrap())
            .unwrap();
        assert_eq!(texts(&d), ["A1", "2", "3B"]);
    }

    #[test]
    fn deleting_across_paragraphs_keeps_the_first_style() {
        let mut d = doc(&["One", "Two", "Three"]);
        d.paragraphs[2].style = ParagraphStyle::Heading2;
        let edit = d.delete(TextRange::between(at(0, 1), at(2, 2))).unwrap();
        let applied = d.apply(&edit).unwrap();
        assert_eq!(texts(&d), ["Oree"]);
        assert_eq!(d.paragraphs[0].style, ParagraphStyle::Normal);
        d.apply(&applied.inverse).unwrap();
        assert_eq!(texts(&d), ["One", "Two", "Three"]);
        assert_eq!(d.paragraphs[2].style, ParagraphStyle::Heading2);
    }

    #[test]
    fn formatting_splits_runs_and_undo_merges_them() {
        let mut d = doc(&["Hello world"]);
        let original = d.clone();
        let edit = d
            .format_text(TextRange::between(at(0, 6), at(0, 11)), |f, _| {
                f.italic = Some(true);
            })
            .unwrap();
        let applied = d.apply(&edit).unwrap();
        assert_eq!(d.paragraphs[0].runs.len(), 2);
        assert_eq!(d.paragraphs[0].runs[1].text, "world");
        d.apply(&applied.inverse).unwrap();
        assert_eq!(d, original);
    }

    #[test]
    fn paragraph_format_applies_with_a_bare_caret() {
        let mut d = doc(&["One", "Two"]);
        let edit = d
            .format_paragraphs(caret(1, 1), |style, format| {
                *style = ParagraphStyle::Title;
                format.align = Some(Align::Center);
            })
            .unwrap();
        d.apply(&edit).unwrap();
        assert_eq!(texts(&d), ["One", "Two"]);
        assert_eq!(d.paragraphs[0].style, ParagraphStyle::Normal);
        assert_eq!(d.paragraphs[1].style, ParagraphStyle::Title);
        assert_eq!(d.paragraphs[1].format.align, Some(Align::Center));
    }

    #[test]
    fn invalid_positions_are_rejected_without_changes() {
        let mut d = doc(&["héllo"]);
        let before = d.clone();
        assert_eq!(
            d.insert_text(caret(0, 2), "x"),
            Err(EditError::NotOnBoundary(at(0, 2)))
        );
        assert_eq!(
            d.delete(TextRange::between(at(0, 0), at(3, 0))),
            Err(EditError::OutOfRange(at(3, 0)))
        );
        let reversed = Edit::Replace {
            range: TextRange {
                start: at(0, 3),
                end: at(0, 1),
            },
            with: Fragment::single(Paragraph::new(ParagraphStyle::Normal)),
        };
        assert_eq!(d.apply(&reversed), Err(EditError::Reversed));
        assert_eq!(d, before);
    }

    #[test]
    fn settings_round_trip() {
        let mut d = doc(&["x"]);
        let original = d.clone();
        let mut settings = d.settings();
        settings.page = settings.page.landscape();
        settings.title = Some("Report".into());
        let applied = d.apply(&Edit::Settings(settings)).unwrap();
        assert_eq!(d.title.as_deref(), Some("Report"));
        d.apply(&applied.inverse).unwrap();
        assert_eq!(d, original);

        let mut bad = d.settings();
        bad.page.margins.left = 1000.0;
        assert!(d.apply(&Edit::Settings(bad)).is_err());
        assert_eq!(d, original);
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
        const WORDS: [&str; 6] = ["a", "ß", "日本", " ", "\n", "xyz"];

        for _ in 0..300 {
            let mut d = doc(&["first", "sécond", "third"]);
            let original = d.clone();
            let mut undo = Vec::new();
            let mut done = Vec::new();
            for _ in 0..15 {
                let mut pick = |d: &Document| loop {
                    let block = next(d.paragraphs.len());
                    let offset = next(d.paragraphs[block].len() + 1);
                    let p = at(block, offset);
                    if d.check(p).is_ok() {
                        break p;
                    }
                };
                let range = TextRange::between(pick(&d), pick(&d));
                let edit = match next(5) {
                    0 => d.insert_text(range, WORDS[next(WORDS.len())]),
                    1 => d.delete(range),
                    2 => d.format_text(range, |f, _| f.bold = Some(!f.bold.unwrap_or(false))),
                    3 => d.format_paragraphs(range, |s, _| *s = ParagraphStyle::Heading2),
                    _ => d.split_paragraph(range),
                }
                .unwrap();
                let applied = d.apply(&edit).unwrap();
                undo.push(applied.inverse);
                done.push((edit, d.clone()));
            }
            let fin = d.clone();
            for inverse in undo.iter().rev() {
                d.apply(inverse).unwrap();
            }
            assert_eq!(d, original);
            for (edit, after) in &done {
                d.apply(edit).unwrap();
                assert_eq!(&d, after);
            }
            assert_eq!(d, fin);
        }
    }
}
