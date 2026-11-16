//! Continuous page layout: every page of the document stacked top to bottom at one zoom.

use std::ops::Range;

use ferrix_types::Size;

use crate::render::LOGICAL_PX_PER_POINT;

/// Space around the pages, in logical pixels.
pub(crate) const PAGE_MARGIN: f32 = 28.0;
/// Space between consecutive pages, in logical pixels.
pub(crate) const PAGE_GAP: f32 = 16.0;

/// Where each page sits in the scrolled content, in logical pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Strip {
    /// Zoom factor (1.0 = 100%) shared by all pages.
    pub(crate) zoom: f32,
    /// Displayed size of each page.
    pub(crate) sizes: Vec<iced::Size>,
    /// Top of each page.
    pub(crate) tops: Vec<f32>,
    /// Width and height of the whole content, margins included.
    pub(crate) content: iced::Size,
}

impl Strip {
    /// Lays out pages of `sizes` points at `zoom`.
    pub(crate) fn new(sizes: &[Size], zoom: f32) -> Self {
        let scale = zoom * LOGICAL_PX_PER_POINT;
        let sizes: Vec<iced::Size> = sizes
            .iter()
            .map(|s| iced::Size::new(s.width * scale, s.height * scale))
            .collect();
        let mut tops = Vec::with_capacity(sizes.len());
        let mut y = PAGE_MARGIN;
        for size in &sizes {
            tops.push(y);
            y += size.height + PAGE_GAP;
        }
        let width = sizes.iter().map(|s| s.width).fold(0.0, f32::max);
        let height = if sizes.is_empty() {
            2.0 * PAGE_MARGIN
        } else {
            y - PAGE_GAP + PAGE_MARGIN
        };
        Self {
            zoom,
            sizes,
            tops,
            content: iced::Size::new(width + 2.0 * PAGE_MARGIN, height),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tops.len()
    }

    /// The scroll position that shows page `index` at the top, within the scrollable range
    /// of a viewport `height` tall.
    pub(crate) fn offset_of(&self, index: usize, height: f32) -> f32 {
        let Some(&top) = self.tops.get(index).filter(|_| index > 0) else {
            return 0.0;
        };
        let max = (self.content.height - height).max(0.0);
        // Leave a sliver of the gap above, so the page edge is visible.
        (top - PAGE_GAP / 2.0).clamp(0.0, max)
    }

    /// Pages that overlap the viewport from `top` to `top + height`.
    pub(crate) fn visible(&self, top: f32, height: f32) -> Range<usize> {
        // The first page starting below `top`, or the one before it if it reaches past `top`.
        let after = self.tops.partition_point(|&t| t < top);
        let start = match after.checked_sub(1) {
            Some(i) if self.tops[i] + self.sizes[i].height > top => i,
            _ => after,
        };
        let end = start + self.tops[start..].partition_point(|&t| t < top + height);
        start..end
    }

    /// The page taking up most of the viewport; the earlier one on a tie.
    pub(crate) fn current(&self, top: f32, height: f32) -> Option<usize> {
        let bottom = top + height;
        let shown = |i: usize| {
            let (a, b) = (self.tops[i], self.tops[i] + self.sizes[i].height);
            (b.min(bottom) - a.max(top)).max(0.0)
        };
        self.visible(top, height)
            .fold(None, |best: Option<(usize, f32)>, i| {
                let area = shown(i);
                match best {
                    Some((_, a)) if a >= area => best,
                    _ => Some((i, area)),
                }
            })
            .map(|(i, _)| i)
    }

    /// How far down page `index` the viewport top is, as a fraction of the page height.
    pub(crate) fn fraction(&self, index: usize, top: f32) -> f32 {
        match (self.tops.get(index), self.sizes.get(index)) {
            (Some(&t), Some(s)) if s.height > 0.0 => ((top - t) / s.height).clamp(-1.0, 1.0),
            _ => 0.0,
        }
    }

    /// The viewport top that is `fraction` of the way down page `index`.
    pub(crate) fn at_fraction(&self, index: usize, fraction: f32, height: f32) -> f32 {
        match (self.tops.get(index), self.sizes.get(index)) {
            (Some(&t), Some(s)) => {
                let max = (self.content.height - height).max(0.0);
                (t + fraction * s.height).clamp(0.0, max)
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pages 72 points square at 75% zoom: 72 logical pixels, so tops are 28, 116, 204…
    fn strip(pages: usize) -> Strip {
        Strip::new(&vec![Size::new(72.0, 72.0); pages], 0.75)
    }

    #[test]
    fn pages_stack_with_gaps_and_margins() {
        let s = strip(3);
        assert_eq!(s.tops, [28.0, 116.0, 204.0]);
        assert_eq!(s.content, iced::Size::new(128.0, 304.0));
    }

    #[test]
    fn visible_pages_overlap_the_viewport() {
        let s = strip(5);
        assert_eq!(s.visible(0.0, 50.0), 0..1);
        // 100 is in the gap after page 0; 120 is inside page 1.
        assert_eq!(s.visible(101.0, 10.0), 1..1);
        assert_eq!(s.visible(90.0, 40.0), 0..2);
        assert_eq!(s.visible(110.0, 200.0), 1..4);
        assert_eq!(s.visible(1000.0, 100.0), 5..5);
    }

    #[test]
    fn the_current_page_fills_most_of_the_viewport() {
        let s = strip(5);
        assert_eq!(s.current(0.0, 100.0), Some(0));
        assert_eq!(s.current(100.0, 100.0), Some(1));
        assert_eq!(s.current(170.0, 100.0), Some(2));
        assert_eq!(s.current(101.0, 10.0), None);
    }

    #[test]
    fn jumping_to_a_page_stays_in_range() {
        let s = strip(5);
        assert_eq!(s.offset_of(0, 100.0), 0.0);
        assert_eq!(s.offset_of(2, 100.0), 196.0);
        // The last page can only scroll as far as the end of the content.
        assert_eq!(s.offset_of(4, 200.0), s.content.height - 200.0);
    }

    #[test]
    fn a_position_survives_a_zoom_change() {
        let small = strip(5);
        let top = small.tops[2] + 18.0;
        let fraction = small.fraction(2, top);
        let big = Strip::new(&[Size::new(72.0, 72.0); 5], 1.5);
        assert_eq!(big.at_fraction(2, fraction, 100.0), big.tops[2] + 36.0);
    }
}
