//! Text on a PDF page as a reader works with it (architecture §5.5): which character is under
//! the pointer, words and lines, selections, the text to copy, and one box per line for
//! highlights and selection drawing.
//!
//! Built from the engine's [`TextPage`]; everything is in page space (points, origin top-left,
//! intrinsic rotation applied), so the engine and the UI stay out of it.

#![forbid(unsafe_code)]

mod search;

use std::ops::Range;

pub use search::Query;

use ferrix_engine::TextPage;
use ferrix_types::{Point, Rect};

/// One character with its box. Engine-generated spaces and line breaks have empty boxes.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub ch: char,
    pub bounds: Rect,
    pub generated: bool,
}

impl Glyph {
    fn has_box(&self) -> bool {
        !self.generated && self.bounds.width() > 0.0 && self.bounds.height() > 0.0
    }
}

/// A page's text, split into lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    glyphs: Vec<Glyph>,
    /// Character ranges of each line, in content order; together they cover every glyph.
    lines: Vec<Range<usize>>,
    /// Each line's box (the union of its glyph boxes); empty for lines without boxes.
    line_boxes: Vec<Option<Rect>>,
}

impl PageText {
    pub fn new(page: &TextPage) -> Self {
        let glyphs: Vec<Glyph> = page
            .chars
            .iter()
            .map(|c| Glyph {
                ch: c.ch,
                bounds: c.bounds,
                generated: c.generated,
            })
            .collect();
        Self::from_glyphs(glyphs)
    }

    pub fn from_glyphs(glyphs: Vec<Glyph>) -> Self {
        // A line ends after a line break, or where the next character starts a new baseline
        // (it doesn't overlap the previous one vertically and doesn't continue to its right).
        let mut lines = Vec::new();
        let mut start = 0;
        let mut last_box: Option<Rect> = None;
        for (i, g) in glyphs.iter().enumerate() {
            if g.has_box()
                && let Some(prev) = last_box
                && i > start
                && starts_new_line(prev, g.bounds)
            {
                lines.push(start..i);
                start = i;
            }
            if g.has_box() {
                last_box = Some(g.bounds);
            }
            if g.ch == '\n' {
                lines.push(start..i + 1);
                start = i + 1;
                last_box = None;
            }
        }
        if start < glyphs.len() {
            lines.push(start..glyphs.len());
        }
        let line_boxes = lines
            .iter()
            .map(|r| {
                union(
                    glyphs[r.clone()]
                        .iter()
                        .filter(|g| g.has_box())
                        .map(|g| g.bounds),
                )
            })
            .collect();
        Self {
            glyphs,
            lines,
            line_boxes,
        }
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// Whether `p` is over text (for the I-beam pointer).
    pub fn is_over_text(&self, p: Point) -> bool {
        self.line_boxes.iter().flatten().any(|b| b.contains(p))
    }

    /// The caret position (a character boundary, `0..=len`) nearest to `p`: on the line under
    /// `p` or vertically closest to it, before or after the character `p` is in.
    pub fn caret_at(&self, p: Point) -> Option<usize> {
        let (line, _) = self
            .line_boxes
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.map(|b| (i, vertical_distance(b, p.y))))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let range = self.lines[line].clone();
        let mut best = (range.start, f32::INFINITY);
        for i in range.clone() {
            let g = &self.glyphs[i];
            if !g.has_box() {
                continue;
            }
            let mid = (g.bounds.x0 + g.bounds.x1) / 2.0;
            let (before, after) = (g.bounds.x0, g.bounds.x1);
            let (pos, d) = if p.x < mid {
                (i, (p.x - before).abs())
            } else {
                (i + 1, (p.x - after).abs())
            };
            if d < best.1 {
                best = (pos, d);
            }
        }
        Some(best.0)
    }

    /// The word around character `i` (letters and digits; otherwise the single character).
    pub fn word_at(&self, i: usize) -> Range<usize> {
        let Some(g) = self.glyphs.get(i) else {
            return i..i;
        };
        if !is_word(g.ch) {
            return i..i + 1;
        }
        let mut start = i;
        while start > 0 && is_word(self.glyphs[start - 1].ch) {
            start -= 1;
        }
        let mut end = i + 1;
        while end < self.glyphs.len() && is_word(self.glyphs[end].ch) {
            end += 1;
        }
        start..end
    }

    /// The line containing character `i`, without its trailing line break.
    pub fn line_at(&self, i: usize) -> Range<usize> {
        let line = self
            .lines
            .iter()
            .find(|r| r.contains(&i))
            .cloned()
            .unwrap_or(i..i);
        let mut end = line.end;
        while end > line.start && matches!(self.glyphs[end - 1].ch, '\n' | '\r') {
            end -= 1;
        }
        line.start..end
    }

    /// The text of `range`, for copying: engine-generated spaces and line breaks included,
    /// `\r\n` written as `\n`, a soft hyphen at a line end joined.
    pub fn text(&self, range: Range<usize>) -> String {
        let range = self.clamp(range);
        let mut out = String::with_capacity(range.len());
        for g in &self.glyphs[range] {
            match g.ch {
                '\r' => {}
                '\u{2}' | '\u{ad}' => {} // PDFium's and Unicode's soft hyphen
                c => out.push(c),
            }
        }
        out
    }

    /// One box per line covered by `range` (the union of its glyph boxes there): what a
    /// selection draws and what a highlight's quads are made of.
    pub fn boxes(&self, range: Range<usize>) -> Vec<Rect> {
        let range = self.clamp(range);
        self.lines
            .iter()
            .filter_map(|line| {
                let from = line.start.max(range.start);
                let to = line.end.min(range.end);
                (from < to).then(|| {
                    union(
                        self.glyphs[from..to]
                            .iter()
                            .filter(|g| g.has_box())
                            .map(|g| g.bounds),
                    )
                })?
            })
            .collect()
    }

    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let end = range.end.min(self.glyphs.len());
        range.start.min(end)..end
    }
}

/// A selection: where it started and where the pointer is now, as caret positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub focus: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            focus: at,
        }
    }

    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.focus
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\'' || c == '’'
}

/// Whether `next` starts a new line after `prev`: it lies wholly above or below, or jumps back
/// to the left on a lower baseline.
fn starts_new_line(prev: Rect, next: Rect) -> bool {
    let overlap = prev.y1.min(next.y1) - prev.y0.max(next.y0);
    let shorter = prev.height().min(next.height()).max(0.01);
    overlap < shorter * 0.5 || (next.x1 < prev.x0 && next.y0 > prev.y0 + shorter * 0.3)
}

fn vertical_distance(b: Rect, y: f32) -> f32 {
    if y < b.y0 {
        b.y0 - y
    } else if y > b.y1 {
        y - b.y1
    } else {
        0.0
    }
}

fn union(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of text at 10 pt per character, 12 pt line height, with generated breaks.
    fn page(lines: &[&str]) -> PageText {
        let mut glyphs = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let y0 = 100.0 + row as f32 * 14.0;
            for (col, ch) in line.chars().enumerate() {
                let x0 = 50.0 + col as f32 * 10.0;
                glyphs.push(Glyph {
                    ch,
                    bounds: Rect {
                        x0,
                        y0,
                        x1: x0 + 9.0,
                        y1: y0 + 12.0,
                    },
                    generated: false,
                });
            }
            if row + 1 < lines.len() {
                glyphs.push(Glyph {
                    ch: '\r',
                    bounds: Rect::default(),
                    generated: true,
                });
                glyphs.push(Glyph {
                    ch: '\n',
                    bounds: Rect::default(),
                    generated: true,
                });
            }
        }
        PageText::from_glyphs(glyphs)
    }

    #[test]
    fn carets_land_before_or_after_the_character_under_the_pointer() {
        let t = page(&["Hello world", "Second line"]);
        // Left half of 'e' (x 60..69) → before it; right half → after it.
        assert_eq!(t.caret_at(Point::new(61.0, 105.0)), Some(1));
        assert_eq!(t.caret_at(Point::new(67.0, 105.0)), Some(2));
        // Below the last line snaps to it; far right is the line's end.
        let second = t.caret_at(Point::new(500.0, 200.0)).expect("caret");
        assert_eq!(t.text(t.line_at(second - 1)), "Second line");
    }

    #[test]
    fn words_lines_and_copied_text() {
        let t = page(&["Hello world", "Second line"]);
        assert_eq!(t.text(t.word_at(7)), "world");
        assert_eq!(t.text(t.word_at(5)), " ");
        assert_eq!(t.text(t.line_at(3)), "Hello world");
        let all = Selection {
            anchor: 24,
            focus: 6,
        }
        .range();
        assert_eq!(t.text(all), "world\nSecond line");
    }

    #[test]
    fn one_box_per_line_covering_only_the_selected_characters() {
        let t = page(&["Hello world", "Second line"]);
        let boxes = t.boxes(6..19);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].x0, 110.0); // 'w'
        assert_eq!(boxes[0].x1, 159.0); // 'd'
        assert_eq!(boxes[1].x0, 50.0);
        assert_eq!(boxes[1].x1, 109.0); // "Second" ends at col 5
        assert!(t.is_over_text(Point::new(55.0, 105.0)));
        assert!(!t.is_over_text(Point::new(55.0, 300.0)));
    }

    #[test]
    fn baseline_changes_split_lines_without_explicit_breaks() {
        let mut glyphs = Vec::new();
        for (i, (x, y)) in [(50.0, 100.0), (60.0, 100.0), (50.0, 120.0)]
            .iter()
            .enumerate()
        {
            glyphs.push(Glyph {
                ch: ['a', 'b', 'c'][i],
                bounds: Rect {
                    x0: *x,
                    y0: *y,
                    x1: x + 9.0,
                    y1: y + 12.0,
                },
                generated: false,
            });
        }
        let t = PageText::from_glyphs(glyphs);
        assert_eq!(t.boxes(0..3).len(), 2);
        assert_eq!(t.line_at(2), 2..3);
    }
}
