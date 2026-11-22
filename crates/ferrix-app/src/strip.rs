//! Continuous page layout: every page of the document at one zoom, in one column or in
//! pairs side by side, scrolled top to bottom.

use std::ops::Range;

use ferrix_store::Layout;
use ferrix_types::Size;

use crate::render::LOGICAL_PX_PER_POINT;

/// Space around the pages, in logical pixels.
pub(crate) const PAGE_MARGIN: f32 = 28.0;
/// Space between consecutive pages, across and down, in logical pixels.
pub(crate) const PAGE_GAP: f32 = 16.0;

/// Which pages share each row: one per row, or pairs (after a lone cover for books). A single
/// page layout has the one row holding `current`.
pub(crate) fn rows(count: usize, layout: Layout, current: usize) -> Vec<Range<usize>> {
    if layout == Layout::SinglePage {
        let page = current.min(count.saturating_sub(1));
        return if count == 0 {
            Vec::new()
        } else {
            std::iter::once(page..page + 1).collect()
        };
    }
    let mut rows = Vec::new();
    let mut i = 0;
    if layout == Layout::TwoPagesCover && count > 0 {
        rows.push(0..1);
        i = 1;
    }
    let per_row = match layout {
        Layout::Continuous | Layout::SinglePage => 1,
        Layout::TwoPages | Layout::TwoPagesCover => 2,
    };
    while i < count {
        let end = (i + per_row).min(count);
        rows.push(i..end);
        i = end;
    }
    rows
}

/// Where each page sits in the scrolled content, in logical pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Strip {
    /// Zoom factor (1.0 = 100%) shared by all pages.
    pub(crate) zoom: f32,
    /// Displayed size of each page.
    pub(crate) sizes: Vec<iced::Size>,
    /// Top of each page; pages in one row are centred on the row's middle.
    pub(crate) tops: Vec<f32>,
    /// The pages of each row, in order.
    pub(crate) rows: Vec<Range<usize>>,
    /// Top and height of each row.
    row_tops: Vec<f32>,
    row_heights: Vec<f32>,
    /// Width and height of the whole content, margins included.
    pub(crate) content: iced::Size,
}

impl Strip {
    /// Lays out pages of `sizes` points at `zoom`, in one column.
    #[cfg(test)]
    pub(crate) fn new(sizes: &[Size], zoom: f32) -> Self {
        Self::with_layout(sizes, zoom, Layout::Continuous, 0)
    }

    /// Lays out pages of `sizes` points at `zoom` in `layout`; a single page layout shows only
    /// page `current`, and the other pages have no place.
    pub(crate) fn with_layout(sizes: &[Size], zoom: f32, layout: Layout, current: usize) -> Self {
        let scale = zoom * LOGICAL_PX_PER_POINT;
        let sizes: Vec<iced::Size> = sizes
            .iter()
            .map(|s| iced::Size::new(s.width * scale, s.height * scale))
            .collect();
        let rows = rows(sizes.len(), layout, current);
        let mut tops = vec![0.0; sizes.len()];
        let (mut row_tops, mut row_heights) = (Vec::new(), Vec::new());
        let mut width: f32 = 0.0;
        let mut y = PAGE_MARGIN;
        for row in &rows {
            let pages = &sizes[row.clone()];
            let height = pages.iter().map(|s| s.height).fold(0.0, f32::max);
            let row_width =
                pages.iter().map(|s| s.width).sum::<f32>() + PAGE_GAP * (pages.len() - 1) as f32;
            width = width.max(row_width);
            for i in row.clone() {
                tops[i] = y + (height - sizes[i].height) / 2.0;
            }
            row_tops.push(y);
            row_heights.push(height);
            y += height + PAGE_GAP;
        }
        let height = if rows.is_empty() {
            2.0 * PAGE_MARGIN
        } else {
            y - PAGE_GAP + PAGE_MARGIN
        };
        Self {
            zoom,
            sizes,
            tops,
            rows,
            row_tops,
            row_heights,
            content: iced::Size::new(width + 2.0 * PAGE_MARGIN, height),
        }
    }

    /// The row holding page `index`.
    fn row_of(&self, index: usize) -> Option<usize> {
        let row = self.rows.partition_point(|r| r.end <= index);
        (row < self.rows.len()).then_some(row)
    }

    pub(crate) fn len(&self) -> usize {
        self.tops.len()
    }

    /// The scroll position that shows page `index` at the top, within the scrollable range
    /// of a viewport `height` tall.
    pub(crate) fn offset_of(&self, index: usize, height: f32) -> f32 {
        let Some(row) = self.row_of(index).filter(|&r| r > 0) else {
            return 0.0;
        };
        let top = self.row_tops[row];
        let max = (self.content.height - height).max(0.0);
        // Leave a sliver of the gap above, so the page edge is visible.
        (top - PAGE_GAP / 2.0).clamp(0.0, max)
    }

    /// Pages in the rows that overlap the viewport from `top` to `top + height`.
    pub(crate) fn visible(&self, top: f32, height: f32) -> Range<usize> {
        // The first row starting below `top`, or the one before it if it reaches past `top`.
        let after = self.row_tops.partition_point(|&t| t < top);
        let start = match after.checked_sub(1) {
            Some(r) if self.row_tops[r] + self.row_heights[r] > top => r,
            _ => after,
        };
        let end = start + self.row_tops[start..].partition_point(|&t| t < top + height);
        let first = self.rows.get(start).map_or(self.sizes.len(), |r| r.start);
        let last = end
            .checked_sub(1)
            .and_then(|r| self.rows.get(r))
            .filter(|_| end > start)
            .map_or(first, |r| r.end);
        first..last
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

    #[test]
    fn rows_pair_pages_after_an_optional_cover() {
        assert_eq!(rows(3, Layout::Continuous, 0), [0..1, 1..2, 2..3]);
        assert_eq!(rows(5, Layout::TwoPages, 0), [0..2, 2..4, 4..5]);
        assert_eq!(rows(4, Layout::TwoPagesCover, 0), [0..1, 1..3, 3..4]);
        assert!(rows(0, Layout::TwoPagesCover, 0).is_empty());
        assert_eq!(rows(4, Layout::SinglePage, 2), vec![(2..3)]);
        assert_eq!(rows(4, Layout::SinglePage, 9), vec![(3..4)]);
        assert!(rows(0, Layout::SinglePage, 0).is_empty());
    }

    #[test]
    fn two_pages_share_a_row_and_its_top() {
        // 72 logical px pages; a half-height page is centred in its row.
        let sizes = [
            Size::new(72.0, 72.0),
            Size::new(72.0, 36.0),
            Size::new(72.0, 72.0),
        ];
        let s = Strip::with_layout(&sizes, 0.75, Layout::TwoPages, 0);
        assert_eq!(s.tops, [28.0, 28.0 + 18.0, 116.0]);
        assert_eq!(s.content.width, 72.0 * 2.0 + PAGE_GAP + 2.0 * PAGE_MARGIN);
        assert_eq!(s.content.height, 28.0 + 72.0 + 16.0 + 72.0 + 28.0);
        assert_eq!(s.visible(0.0, 50.0), 0..2);
        assert_eq!(s.visible(110.0, 50.0), 2..3);
        assert_eq!(s.visible(90.0, 40.0), 0..3);
        assert_eq!(s.visible(1000.0, 10.0), 3..3);
        // Jumping to page 1 shows its row, which starts at the top.
        assert_eq!(s.offset_of(1, 50.0), 0.0);
        assert_eq!(s.offset_of(2, 50.0), 108.0);
        assert_eq!(
            s.current(0.0, 100.0),
            Some(0),
            "a tie goes to the left page"
        );
    }

    #[test]
    fn a_single_page_layout_shows_only_the_current_page() {
        let s = Strip::with_layout(&[Size::new(72.0, 72.0); 5], 0.75, Layout::SinglePage, 3);
        assert_eq!(s.rows, vec![(3..4)]);
        assert_eq!(s.tops[3], PAGE_MARGIN);
        assert_eq!(s.content.height, 72.0 + 2.0 * PAGE_MARGIN);
        assert_eq!(s.visible(0.0, 500.0), 3..4);
        assert_eq!(s.current(0.0, 500.0), Some(3));
        assert_eq!(s.offset_of(3, 50.0), 0.0);
        assert_eq!(s.offset_of(1, 50.0), 0.0, "other pages have no place");
    }
}
