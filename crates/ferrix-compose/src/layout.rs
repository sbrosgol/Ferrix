//! Line breaking and pagination (ADR-0008 §4–5).
//!
//! cosmic-text shapes each paragraph and breaks it into lines; this module stacks the lines onto
//! pages. The result is what both the editor and the PDF exporter draw. Coordinates are points
//! from the page's top-left corner, y pointing down.

use std::ops::Range;

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Style, Weight, Wrap, fontdb};

use crate::fonts::Fonts;
use crate::model::{Align, CharFormat, Color, Document, PageSetup, Paragraph};

/// Normal line height as a multiple of the font size ("single" spacing).
const LINE_HEIGHT: f32 = 1.15;

/// Fewest lines of a paragraph left alone at the bottom or top of a page.
const MIN_LINES: usize = 2;

/// A laid-out document.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub page: PageSetup,
    /// Never empty.
    pub pages: Vec<PageLayout>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageLayout {
    pub lines: Vec<Line>,
}

/// One line of a paragraph, placed on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The paragraph.
    pub block: usize,
    /// The paragraph's text on this line (byte offsets).
    pub range: Range<usize>,
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    /// Left edge and width of the box the line is aligned in.
    pub left: f32,
    pub width: f32,
    pub align: Align,
    pub rtl: bool,
    /// In visual order.
    pub glyphs: Vec<PlacedGlyph>,
    pub decorations: Vec<Decoration>,
}

/// A glyph at its final position.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedGlyph {
    pub font: fontdb::ID,
    pub glyph_id: u16,
    /// Font size in points.
    pub size: f32,
    /// Pen position: left edge and baseline.
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    /// The paragraph text this glyph shows (byte offsets).
    pub range: Range<usize>,
    pub color: Color,
    /// Right-to-left text: the glyph's logical start is its right edge.
    pub rtl: bool,
    pub weight: fontdb::Weight,
}

/// An underline or strikethrough bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoration {
    pub x: f32,
    pub top: f32,
    pub width: f32,
    pub thickness: f32,
    pub color: Color,
}

/// Character formatting with the paragraph style applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedFormat {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub color: Color,
}

pub(crate) fn resolve(
    doc: &Document,
    paragraph: &Paragraph,
    format: &CharFormat,
) -> ResolvedFormat {
    let style = paragraph.style.defaults();
    ResolvedFormat {
        family: format
            .family
            .clone()
            .unwrap_or_else(|| doc.font.family.clone()),
        size: format.size.unwrap_or(doc.font.size * style.scale),
        bold: format.bold.unwrap_or(style.bold),
        italic: format.italic.unwrap_or(false),
        underline: format.underline,
        strikethrough: format.strikethrough,
        color: format.color.unwrap_or(Color::BLACK),
    }
}

/// Paragraphs already broken into lines, keyed by everything that affects their layout.
#[derive(Default)]
pub struct LayoutCache {
    entries: std::collections::HashMap<u64, ParagraphLines>,
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("paragraphs", &self.entries.len())
            .finish()
    }
}

fn cache_key(doc: &Document, paragraph: &Paragraph) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(paragraph)
        .unwrap_or_default()
        .hash(&mut hasher);
    serde_json::to_vec(&doc.font)
        .unwrap_or_default()
        .hash(&mut hasher);
    for value in [
        doc.page.width,
        doc.page.margins.left,
        doc.page.margins.right,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// A paragraph broken into lines, before it is placed on a page.
#[derive(Clone)]
struct ParagraphLines {
    space_before: f32,
    space_after: f32,
    keep_with_next: bool,
    lines: Vec<Line>,
}

impl Layout {
    /// Lays out the whole document.
    pub fn new(doc: &Document, fonts: &mut Fonts) -> Self {
        Self::cached(doc, fonts, &mut LayoutCache::default())
    }

    /// Lays out the document, reusing paragraphs that have not changed since the last call with
    /// the same cache.
    pub fn cached(doc: &Document, fonts: &mut Fonts, cache: &mut LayoutCache) -> Self {
        let mut used = std::collections::HashMap::with_capacity(doc.paragraphs.len());
        let paragraphs: Vec<ParagraphLines> = doc
            .paragraphs
            .iter()
            .enumerate()
            .map(|(block, p)| {
                let key = cache_key(doc, p);
                let mut lines = match cache
                    .entries
                    .remove(&key)
                    .or_else(|| used.get(&key).cloned())
                {
                    Some(lines) => lines,
                    None => break_lines(doc, block, p, fonts),
                };
                for line in &mut lines.lines {
                    line.block = block;
                }
                used.insert(key, lines.clone());
                lines
            })
            .collect();
        cache.entries = used;
        paginate(doc.page, paragraphs)
    }

    /// The page and line showing a position.
    pub fn locate(&self, block: usize, offset: usize) -> Option<(usize, &Line)> {
        let mut found = None;
        for (page, layout) in self.pages.iter().enumerate() {
            for line in layout.lines.iter().filter(|l| l.block == block) {
                if line.range.start <= offset {
                    found = Some((page, line));
                }
            }
        }
        found
    }
}

fn break_lines(
    doc: &Document,
    block: usize,
    paragraph: &Paragraph,
    fonts: &mut Fonts,
) -> ParagraphLines {
    let style = paragraph.style.defaults();
    let format = paragraph.format;
    let format_align = format.align.unwrap_or_default();
    let spacing = format.line_spacing.unwrap_or(1.0).clamp(0.5, 5.0);
    let indent = format
        .indent
        .unwrap_or(0.0)
        .clamp(0.0, doc.page.content_width() / 2.0);
    let left = doc.page.margins.left + indent;
    let width = doc.page.content_width() - indent;

    // Spans for cosmic-text, with the run index as metadata. Line breaks are not allowed inside a
    // paragraph; any that slipped into the model are laid out as spaces (same byte length).
    let resolved: Vec<ResolvedFormat> = if paragraph.runs.is_empty() {
        vec![resolve(doc, paragraph, &paragraph.format_at(0))]
    } else {
        paragraph
            .runs
            .iter()
            .map(|r| resolve(doc, paragraph, &r.format))
            .collect()
    };
    let texts: Vec<String> = if paragraph.runs.is_empty() {
        vec![String::new()]
    } else {
        paragraph
            .runs
            .iter()
            .map(|r| r.text.replace(['\n', '\r'], " "))
            .collect()
    };
    let attrs: Vec<Attrs<'_>> = resolved
        .iter()
        .enumerate()
        .map(|(i, f)| {
            Attrs::new()
                .family(Family::Name(&f.family))
                .weight(if f.bold { Weight::BOLD } else { Weight::NORMAL })
                .style(if f.italic {
                    Style::Italic
                } else {
                    Style::Normal
                })
                .metrics(Metrics::new(f.size, f.size * LINE_HEIGHT * spacing))
                .metadata(i)
        })
        .collect();

    let first = &resolved[0];
    let system = fonts.system();
    let mut buffer = Buffer::new(
        system,
        Metrics::new(first.size, first.size * LINE_HEIGHT * spacing),
    );
    buffer.set_wrap(system, Wrap::WordOrGlyph);
    buffer.set_size(system, Some(width), None);
    let align = match format.align.unwrap_or_default() {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
        Align::Justify => cosmic_text::Align::Justified,
    };
    buffer.set_rich_text(
        system,
        texts.iter().map(String::as_str).zip(attrs.iter().cloned()),
        &attrs[0],
        Shaping::Advanced,
        Some(align),
    );
    buffer.shape_until_scroll(system, false);

    let mut lines: Vec<Line> = Vec::new();
    for run in buffer.layout_runs() {
        let mut glyphs = Vec::with_capacity(run.glyphs.len());
        let mut decorations: Vec<Decoration> = Vec::new();
        for g in run.glyphs {
            let format = &resolved[g.metadata.min(resolved.len() - 1)];
            let x = left + g.x + g.x_offset;
            let y = run.line_y - g.y_offset;
            glyphs.push(PlacedGlyph {
                font: g.font_id,
                glyph_id: g.glyph_id,
                size: g.font_size,
                x,
                y,
                advance: g.w,
                range: g.start..g.end,
                color: format.color,
                rtl: g.level.is_rtl(),
                weight: g.font_weight,
            });
            if format.underline || format.strikethrough {
                let metrics = system
                    .get_font(g.font_id, g.font_weight)
                    .map(|font| *font.metrics());
                let em = metrics.map_or(1000.0, |m| f32::from(m.units_per_em.max(1)));
                let scale = g.font_size / em;
                let bars = [
                    (
                        format.underline,
                        metrics.and_then(|m| m.underline),
                        -0.1,
                        0.05,
                    ),
                    (
                        format.strikethrough,
                        metrics.and_then(|m| m.strikeout),
                        0.3,
                        0.05,
                    ),
                ];
                for (on, metric, offset, thickness) in bars {
                    if !on {
                        continue;
                    }
                    let (offset, thickness) = metric
                        .map_or((offset * g.font_size, thickness * g.font_size), |d| {
                            (d.offset * scale, d.thickness * scale)
                        });
                    let bar = Decoration {
                        x: left + g.x,
                        top: run.line_y - offset,
                        width: g.w,
                        thickness: thickness.max(0.25),
                        color: format.color,
                    };
                    // Join with the previous bar when it continues it.
                    match decorations.iter_mut().rev().find(|d| {
                        d.top == bar.top && d.thickness == bar.thickness && d.color == bar.color
                    }) {
                        Some(prev) if (prev.x + prev.width - bar.x).abs() < 0.01 => {
                            prev.width += bar.width;
                        }
                        _ => decorations.push(bar),
                    }
                }
            }
        }
        let start = run.glyphs.iter().map(|g| g.start).min();
        lines.push(Line {
            block,
            range: start.unwrap_or(0)..0,
            top: run.line_top,
            height: run.line_height,
            baseline: run.line_y,
            left,
            width,
            align: format_align,
            rtl: run.rtl,
            glyphs,
            decorations,
        });
    }
    if lines.is_empty() {
        let height = first.size * LINE_HEIGHT * spacing;
        lines.push(Line {
            block,
            range: 0..0,
            top: 0.0,
            height,
            baseline: first.size * 0.8 + (height - first.size * LINE_HEIGHT) / 2.0,
            left,
            width,
            align: format_align,
            rtl: false,
            glyphs: Vec::new(),
            decorations: Vec::new(),
        });
    }
    // Each line runs up to where the next starts; the last to the end of the paragraph.
    let len = paragraph.len();
    let starts: Vec<usize> = lines.iter().map(|l| l.range.start).collect();
    for (i, line) in lines.iter_mut().enumerate() {
        let start = if i == 0 { 0 } else { starts[i] };
        line.range = start..starts.get(i + 1).copied().unwrap_or(len);
    }

    ParagraphLines {
        space_before: format.space_before.unwrap_or(style.space_before),
        space_after: format.space_after.unwrap_or(style.space_after),
        keep_with_next: paragraph.style.is_heading(),
        lines,
    }
}

/// Moves a paragraph's lines (laid out from y = 0) to `y` on a page.
fn place(line: &Line, y: f32) -> Line {
    let dy = y - line.top;
    let mut line = line.clone();
    line.top += dy;
    line.baseline += dy;
    for glyph in &mut line.glyphs {
        glyph.y += dy;
    }
    for bar in &mut line.decorations {
        bar.top += dy;
    }
    line
}

fn paginate(page: PageSetup, paragraphs: Vec<ParagraphLines>) -> Layout {
    let height = page.content_height();
    let top = page.margins.top;
    let mut pages = vec![PageLayout::default()];
    // Content used on the current page.
    let mut y = 0.0_f32;

    for (index, paragraph) in paragraphs.iter().enumerate() {
        let lines = &paragraph.lines;
        let before = if y > 0.0 { paragraph.space_before } else { 0.0 };

        // Keep a heading with the start of the next paragraph.
        if paragraph.keep_with_next && y > 0.0 {
            let own: f32 = lines.iter().map(|l| l.height).sum();
            let next = paragraphs.get(index + 1).map_or(0.0, |n| {
                n.space_before
                    + n.lines
                        .iter()
                        .take(MIN_LINES)
                        .map(|l| l.height)
                        .sum::<f32>()
            });
            if own <= height && y + before + own + paragraph.space_after + next > height {
                pages.push(PageLayout::default());
                y = 0.0;
            }
        }

        let mut rest: &[Line] = lines;
        let mut before = if y > 0.0 { paragraph.space_before } else { 0.0 };
        while !rest.is_empty() {
            // How many of the remaining lines fit here.
            let mut used = y + before;
            let mut fit = 0;
            for line in rest {
                if used + line.height > height + 0.01 {
                    break;
                }
                used += line.height;
                fit += 1;
            }
            if fit < rest.len() {
                let total = rest.len();
                // No widow: leave at least MIN_LINES for the next page.
                if total - fit < MIN_LINES && total >= MIN_LINES * 2 {
                    fit = total - MIN_LINES;
                }
                // No orphan: start the paragraph on the next page instead.
                if fit < MIN_LINES && total >= MIN_LINES && rest.len() == lines.len() {
                    fit = 0;
                }
                // A line taller than the page still has to go somewhere.
                if fit == 0 && y == 0.0 {
                    fit = 1;
                }
            }
            let mut cursor = top + y + before;
            let current = pages.last_mut().map(|p| &mut p.lines);
            if let Some(out) = current {
                for line in &rest[..fit] {
                    out.push(place(line, cursor));
                    cursor += line.height;
                }
            }
            y = cursor - top;
            rest = &rest[fit..];
            if !rest.is_empty() {
                pages.push(PageLayout::default());
                y = 0.0;
                before = 0.0;
            }
        }
        y += paragraph.space_after;
        if y >= height && index + 1 < paragraphs.len() {
            pages.push(PageLayout::default());
            y = 0.0;
        }
    }

    Layout { page, pages }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::ParagraphStyle;

    fn normal(text: &str) -> Paragraph {
        Paragraph::new(ParagraphStyle::Normal).with_text(text, CharFormat::default())
    }

    fn doc(paragraphs: Vec<Paragraph>) -> Document {
        let mut d = Document::new();
        d.paragraphs = paragraphs;
        d
    }

    #[test]
    fn short_text_is_one_line_inside_the_margins() {
        let d = doc(vec![normal("Hello, world")]);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        assert_eq!(layout.pages.len(), 1);
        let lines = &layout.pages[0].lines;
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line.range, 0..12);
        assert_eq!(line.glyphs.len(), 12);
        assert!((line.top - 72.0).abs() < 0.01);
        assert!((line.glyphs[0].x - 72.0).abs() < 0.5);
        assert!(line.baseline > line.top && line.baseline < line.top + line.height);
        // Glyphs advance left to right.
        assert!(line.glyphs.windows(2).all(|w| w[0].x < w[1].x));
    }

    #[test]
    fn long_paragraphs_wrap_within_the_text_width() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let d = doc(vec![normal(&text)]);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        let lines = &layout.pages[0].lines;
        assert!(lines.len() > 3);
        let right = d.page.width - d.page.margins.right;
        for line in lines {
            for g in &line.glyphs {
                // Trailing spaces may hang past the margin, like in every word processor.
                if !text[g.range.clone()].trim().is_empty() {
                    assert!(
                        g.x + g.advance <= right + 0.5,
                        "glyph at {} past {right}",
                        g.x
                    );
                }
            }
        }
        // Line ranges tile the paragraph.
        assert_eq!(lines[0].range.start, 0);
        assert_eq!(lines.last().unwrap().range.end, text.len());
        assert!(lines.windows(2).all(|w| w[0].range.end == w[1].range.start));
    }

    #[test]
    fn empty_paragraphs_take_up_a_line() {
        let d = doc(vec![
            normal("a"),
            Paragraph::new(ParagraphStyle::Normal),
            normal("b"),
        ]);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        let lines = &layout.pages[0].lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].range, 0..0);
        assert!(lines[1].height > 10.0);
        assert!(lines[2].top > lines[1].top + lines[1].height - 0.01);
    }

    #[test]
    fn text_flows_onto_new_pages() {
        let paragraphs = (0..120)
            .map(|i| normal(&format!("Paragraph {i}")))
            .collect();
        let d = doc(paragraphs);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        assert!(layout.pages.len() >= 3);
        let bottom = d.page.height - d.page.margins.bottom;
        let mut blocks = Vec::new();
        for page in &layout.pages {
            assert!(!page.lines.is_empty());
            for line in &page.lines {
                assert!(line.top >= d.page.margins.top - 0.01);
                assert!(line.top + line.height <= bottom + 0.01);
                blocks.push(line.block);
            }
        }
        assert_eq!(blocks, (0..120).collect::<Vec<_>>());
    }

    #[test]
    fn headings_stay_with_the_next_paragraph() {
        let fonts = &mut Fonts::bundled();
        let mut moved = 0;
        for fillers in 30..70 {
            let mut paragraphs: Vec<Paragraph> = (0..fillers).map(|_| normal("filler")).collect();
            let mut heading = normal("Heading");
            heading.style = ParagraphStyle::Heading1;
            paragraphs.push(heading);
            paragraphs.push(normal(&"Body text that follows the heading. ".repeat(8)));
            let d = doc(paragraphs);
            let layout = Layout::new(&d, fonts);
            let (heading_page, heading_line) = layout.locate(fillers, 0).unwrap();
            let (body_page, _) = layout.locate(fillers + 1, 0).unwrap();
            assert_eq!(heading_page, body_page, "{fillers} fillers");
            // Count the cases where the heading had room on the previous page but was moved.
            let previous = layout.pages[heading_page.saturating_sub(1)]
                .lines
                .last()
                .unwrap();
            let room = d.page.height - d.page.margins.bottom - previous.top - previous.height;
            if heading_page > 0 && heading_line.top < 73.0 && room > heading_line.height + 18.0 {
                moved += 1;
            }
        }
        assert!(moved > 0, "the rule was never exercised");
    }

    #[test]
    fn no_single_line_orphans_or_widows() {
        let long = "word ".repeat(400);
        let fonts = &mut Fonts::bundled();
        for fillers in 0..60 {
            let mut paragraphs: Vec<Paragraph> = (0..fillers).map(|_| normal("filler")).collect();
            paragraphs.push(normal(&long));
            let d = doc(paragraphs);
            let layout = Layout::new(&d, fonts);
            let block = d.paragraphs.len() - 1;
            for page in &layout.pages {
                let count = page.lines.iter().filter(|l| l.block == block).count();
                assert!(
                    count == 0 || count >= MIN_LINES,
                    "{fillers} fillers: {count} line(s)"
                );
            }
        }
    }

    #[test]
    fn formatting_changes_glyphs_and_adds_decorations() {
        let mut p = normal("plain ");
        p.push(
            "bold",
            CharFormat {
                bold: Some(true),
                underline: true,
                color: Some(Color(200, 0, 0)),
                ..CharFormat::default()
            },
        );
        let d = doc(vec![p]);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        let line = &layout.pages[0].lines[0];
        let plain = &line.glyphs[0];
        let bold = line.glyphs.last().unwrap();
        assert_ne!(plain.font, bold.font);
        assert_eq!(bold.color, Color(200, 0, 0));
        assert_eq!(line.decorations.len(), 1);
        let bar = &line.decorations[0];
        assert!(bar.top > line.baseline, "underline sits below the baseline");
        let start = line.glyphs[6].x;
        assert!((bar.x - start).abs() < 0.5);
    }

    #[test]
    fn styles_and_alignment_apply() {
        let mut title = normal("Title");
        title.style = ParagraphStyle::Title;
        title.format.align = Some(Align::Center);
        let d = doc(vec![title, normal("Body")]);
        let layout = Layout::new(&d, &mut Fonts::bundled());
        let lines = &layout.pages[0].lines;
        assert!(lines[0].glyphs[0].size > lines[1].glyphs[0].size * 2.0);
        let first = lines[0].glyphs.first().unwrap();
        let last = lines[0].glyphs.last().unwrap();
        let centre = (first.x + last.x + last.advance) / 2.0;
        assert!(
            (centre - d.page.width / 2.0).abs() < 1.0,
            "centre at {centre}"
        );
        assert_eq!(layout.locate(1, 2).map(|(p, l)| (p, l.block)), Some((0, 1)));
    }
}
