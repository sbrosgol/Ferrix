//! Finding text on a page (architecture §5.5): matches are character ranges of the page, so
//! they draw and select like any selection.
//!
//! Page text and query are folded the same way: compatibility forms (ligatures, full-width
//! letters) become plain letters, runs of spaces and line breaks become one space, and a word
//! hyphenated at a line end is joined. Without "match case", case and accents are ignored too.

use std::ops::Range;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::PageText;

/// A query, folded once for every page searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    folded: Vec<char>,
    match_case: bool,
}

impl Query {
    /// `None` for a query of only spaces.
    pub fn new(text: &str, match_case: bool) -> Option<Self> {
        let mut folded = Vec::new();
        for c in text.chars() {
            push_folded(&mut folded, c, match_case, |out, c| {
                if !(c == ' ' && out.last() == Some(&' ')) {
                    out.push(c);
                }
            });
        }
        while folded.last() == Some(&' ') {
            folded.pop();
        }
        let start = folded.iter().take_while(|c| **c == ' ').count();
        folded.drain(..start);
        (!folded.is_empty()).then_some(Self { folded, match_case })
    }
}

/// Appends `c` folded, calling `push` for each resulting character; spaces collapse.
fn push_folded<T>(out: &mut Vec<T>, c: char, match_case: bool, push: impl Fn(&mut Vec<T>, char)) {
    for d in std::iter::once(c).nfkd() {
        if !match_case && is_combining_mark(d) {
            continue;
        }
        if d.is_whitespace() {
            push(out, ' ');
        } else if match_case {
            push(out, d);
        } else {
            for l in d.to_lowercase() {
                push(out, l);
            }
        }
    }
}

impl PageText {
    /// Every match of `query`, in page order, without overlaps.
    pub fn find(&self, query: &Query) -> Vec<Range<usize>> {
        // The folded page: each character with the glyph it came from.
        let mut folded: Vec<(char, usize)> = Vec::with_capacity(self.glyphs.len());
        let glyphs = &self.glyphs;
        let mut i = 0;
        while i < glyphs.len() {
            let c = glyphs[i].ch;
            match c {
                '\r' | '\u{ad}' => {}
                // PDFium's mark for a hyphen at a line end: the word continues on the next line.
                '\u{2}' => {
                    while glyphs
                        .get(i + 1)
                        .is_some_and(|g| matches!(g.ch, '\r' | '\n'))
                    {
                        i += 1;
                    }
                }
                '-' if glyphs
                    .get(i + 1)
                    .is_some_and(|g| matches!(g.ch, '\r' | '\n'))
                    && i > 0
                    && glyphs[i - 1].ch.is_alphabetic() =>
                {
                    // An ordinary hyphen at a line end matches with and without the break.
                    folded.push(('-', i));
                    while glyphs
                        .get(i + 1)
                        .is_some_and(|g| matches!(g.ch, '\r' | '\n'))
                    {
                        i += 1;
                    }
                }
                _ => push_folded(&mut folded, c, query.match_case, |out, f| {
                    if f == ' ' && out.last().is_some_and(|(l, _)| *l == ' ') {
                        return;
                    }
                    out.push((f, i));
                }),
            }
            i += 1;
        }
        let q = &query.folded;
        let mut out = Vec::new();
        let mut at = 0;
        while at + q.len() <= folded.len() {
            if folded[at..at + q.len()].iter().map(|(c, _)| c).eq(q.iter()) {
                let start = folded[at].1;
                let end = folded[at + q.len() - 1].1 + 1;
                out.push(start..end);
                at += q.len();
            } else {
                at += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use ferrix_types::Rect;

    use super::*;
    use crate::Glyph;

    fn text(s: &str) -> PageText {
        let glyphs = s
            .chars()
            .enumerate()
            .map(|(i, ch)| Glyph {
                ch,
                bounds: Rect {
                    x0: i as f32 * 10.0,
                    y0: 0.0,
                    x1: i as f32 * 10.0 + 9.0,
                    y1: 12.0,
                },
                generated: matches!(ch, '\r' | '\n'),
            })
            .collect();
        PageText::from_glyphs(glyphs)
    }

    fn find(page: &str, query: &str, case: bool) -> Vec<String> {
        let t = text(page);
        t.find(&Query::new(query, case).expect("query"))
            .into_iter()
            .map(|r| t.text(r))
            .collect()
    }

    #[test]
    fn ignores_case_and_accents_unless_asked() {
        assert_eq!(
            find("Café, CAFE and cafe", "cafe", false),
            ["Café", "CAFE", "cafe"]
        );
        assert_eq!(find("Café, CAFE and cafe", "cafe", true), ["cafe"]);
        assert_eq!(find("Café", "Café", true), ["Café"]);
    }

    #[test]
    fn spaces_line_breaks_and_hyphens_fold() {
        assert_eq!(
            find("energy\r\nstar", "energy star", false),
            ["energy\nstar"]
        );
        assert_eq!(find("two   spaces", "two spaces", false).len(), 1);
        assert_eq!(find("two spaces", " two   spaces ", false).len(), 1);
        assert_eq!(find("effi\u{2}\r\nciency", "efficiency", false).len(), 1);
        assert_eq!(find("ﬁle", "file", false), ["ﬁle"]);
        assert!(Query::new("   ", false).is_none());
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find("aaaa", "aa", false).len(), 2);
    }
}
