//! The thumbnail column's geometry, so only the thumbnails near the visible part of the
//! sidebar are built and rendered, however long the document.

use std::ops::Range;

use ferrix_types::Size;

use crate::app::THUMB_WIDTH;

/// Space above the first thumbnail.
pub(crate) const TOP: f32 = 4.0;
/// Space below the last thumbnail.
pub(crate) const BOTTOM: f32 = 16.0;
/// Space between thumbnails.
pub(crate) const GAP: f32 = 16.0;
/// The ring around the paper, each side.
pub(crate) const RING: f32 = 3.0;
/// Between the ring and the page-number badge.
pub(crate) const LABEL_GAP: f32 = 6.0;
/// The page-number badge, fixed so the column's height is known exactly.
pub(crate) const LABEL_HEIGHT: f32 = 18.0;
/// Thumbnails kept beyond the visible ones, either side: a short scroll finds them ready.
const MARGIN: usize = 12;

/// Logical size of a page's thumbnail: `THUMB_WIDTH` wide, and not much taller, whatever the
/// orientation. The renderer uses the same box.
pub(crate) fn paper(page: Size) -> iced::Size {
    let scale = (THUMB_WIDTH / page.width).min(THUMB_WIDTH * 1.4 / page.height);
    iced::Size::new(page.width * scale, page.height * scale)
}

/// Where each thumbnail sits in the sidebar's scrolled column.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Column {
    /// Size of each thumbnail's paper.
    pub(crate) papers: Vec<iced::Size>,
    /// Top of each thumbnail's slot (ring, paper and badge).
    pub(crate) tops: Vec<f32>,
    pub(crate) height: f32,
}

impl Column {
    pub(crate) fn new(pages: &[Size]) -> Self {
        let papers: Vec<iced::Size> = pages.iter().map(|&p| paper(p)).collect();
        let mut tops = Vec::with_capacity(papers.len());
        // Summed in f64: thousands of f32 additions drift by pixels.
        let mut y = f64::from(TOP);
        for p in &papers {
            tops.push(y as f32);
            y += f64::from(slot(*p) + GAP);
        }
        let height = if papers.is_empty() {
            TOP + BOTTOM
        } else {
            (y - f64::from(GAP) + f64::from(BOTTOM)) as f32
        };
        Self {
            papers,
            tops,
            height,
        }
    }

    /// Thumbnails overlapping the view from `top`, `height` tall, plus a margin either side.
    pub(crate) fn near(&self, top: f32, height: f32) -> Range<usize> {
        let start = self
            .tops
            .partition_point(|&t| t < top)
            .saturating_sub(1 + MARGIN);
        let end = self.tops.partition_point(|&t| t < top + height) + MARGIN;
        start.min(self.len())..end.min(self.len())
    }

    /// The scroll offset that shows thumbnail `index` whole, moving as little as possible
    /// from `top`; `None` if it is already in view.
    pub(crate) fn reveal(&self, index: usize, top: f32, height: f32) -> Option<f32> {
        let (&t, &p) = (self.tops.get(index)?, self.papers.get(index)?);
        let bottom = t + slot(p);
        if t >= top && bottom <= top + height {
            return None;
        }
        let max = (self.height - height).max(0.0);
        let wanted = if t < top || slot(p) > height {
            t - GAP / 2.0
        } else {
            bottom - height + GAP / 2.0
        };
        Some(wanted.clamp(0.0, max))
    }

    /// The bottom of slot `index`, where the next spacer starts.
    pub(crate) fn bottom(&self, index: usize) -> f32 {
        match (self.tops.get(index), self.papers.get(index)) {
            (Some(&t), Some(&p)) => t + slot(p),
            _ => self.height - BOTTOM,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tops.len()
    }
}

/// Height of a thumbnail's slot: ring, paper, gap and badge.
fn slot(paper: iced::Size) -> f32 {
    paper.height + 2.0 * RING + LABEL_GAP + LABEL_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Size = Size {
        width: 612.0,
        height: 792.0,
    };

    #[test]
    fn papers_fit_the_thumbnail_box() {
        let portrait = paper(LETTER);
        assert!((portrait.width - THUMB_WIDTH).abs() < 1e-3);
        let landscape = paper(Size::new(792.0, 612.0));
        assert!((landscape.width - THUMB_WIDTH).abs() < 1e-3);
        assert!(landscape.height < portrait.height);
        // A very tall page is capped by height instead.
        let tall = paper(Size::new(100.0, 1000.0));
        assert!((tall.height - THUMB_WIDTH * 1.4).abs() < 1e-3);
    }

    #[test]
    fn slots_stack_and_only_nearby_ones_are_built() {
        let col = Column::new(&vec![LETTER; 1000]);
        let step = f64::from(col.tops[1] - col.tops[0]);
        assert_eq!(col.tops[0], TOP);
        let expected = f64::from(TOP - GAP + BOTTOM) + 1000.0 * step;
        assert!(
            (f64::from(col.height) - expected).abs() < 0.1,
            "{} vs {expected}",
            col.height
        );
        let step = step as f32;
        let near = col.near(step * 500.0, step * 3.0);
        assert!(near.contains(&500) && near.contains(&502));
        assert!(near.len() < 40, "{near:?}");
        assert_eq!(col.near(0.0, 100.0).start, 0);
        assert_eq!(col.near(col.height, 100.0).end, 1000);
    }

    #[test]
    fn revealing_moves_as_little_as_needed() {
        let col = Column::new(&vec![LETTER; 50]);
        let step = col.tops[1] - col.tops[0];
        let view = step * 4.0;
        assert_eq!(col.reveal(1, 0.0, view), None);
        // Below the view: scrolled until its bottom shows.
        let below = col.reveal(10, 0.0, view).expect("scrolls");
        assert!((below - (col.bottom(10) - view + GAP / 2.0)).abs() < 1e-3);
        // Above the view: scrolled until its top shows.
        let above = col.reveal(2, step * 20.0, view).expect("scrolls");
        assert!((above - (col.tops[2] - GAP / 2.0)).abs() < 1e-3);
    }
}
