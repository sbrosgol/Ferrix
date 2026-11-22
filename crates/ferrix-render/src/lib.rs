//! The page view's rendering plan (architecture §5.3), free of UI and engine types:
//!
//! - [`Grid`]: a page at one scale cut into [`TILE`]-pixel tiles, and which tiles a viewport
//!   needs.
//! - [`Grid::next_batch`]: which missing tiles to render in the next engine call. PDFium
//!   interprets the whole page on every call, so neighbouring tiles go together, in bands.
//! - [`split`]: a rendered band cut back into tiles.
//! - [`TileCache`]: rendered tiles under a byte budget, the least recently drawn evicted first.

#![forbid(unsafe_code)]

use std::{collections::HashMap, hash::Hash, ops::Range};

use ferrix_types::PixelRect;

/// Tile edge in device pixels. 512² RGBA is 1 MiB: under iced's synchronous upload limit, so a
/// tile appears in the frame it arrives in, and few enough tiles that a screenful is ~20.
pub const TILE: u32 = 512;

/// A page at one scale, as a grid of tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    /// Page size in device pixels.
    pub width: u32,
    pub height: u32,
}

/// A tile's column and row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub col: u32,
    pub row: u32,
}

impl Grid {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn cols(&self) -> u32 {
        self.width.div_ceil(TILE)
    }

    pub fn rows(&self) -> u32 {
        self.height.div_ceil(TILE)
    }

    /// The pixels of tile `cell`; edge tiles are smaller.
    pub fn rect(&self, cell: Cell) -> PixelRect {
        let (x, y) = (cell.col * TILE, cell.row * TILE);
        PixelRect::new(
            x,
            y,
            TILE.min(self.width.saturating_sub(x)),
            TILE.min(self.height.saturating_sub(y)),
        )
    }

    /// The tiles overlapping `view` (page pixels, which may reach past the page), as column and
    /// row ranges; empty when the view misses the page.
    pub fn covering(&self, view: Region) -> (Range<u32>, Range<u32>) {
        let span = |from: f32, to: f32, cells: u32| {
            let first = (from.max(0.0) / TILE as f32).floor() as u32;
            let end = ((to.max(0.0) / TILE as f32).ceil() as u32).min(cells);
            first.min(end)..end
        };
        (
            span(view.x0, view.x1, self.cols()),
            span(view.y0, view.y1, self.rows()),
        )
    }

    /// The next band of tiles to render: the first row of `rows` with a tile that
    /// `missing` reports, and up to `max_rows` rows from it, across the columns from the first
    /// to the last missing one (at most `max_cols`). Rendering a band costs one engine call.
    pub fn next_batch(
        &self,
        cols: Range<u32>,
        rows: Range<u32>,
        max_cols: u32,
        max_rows: u32,
        missing: impl Fn(Cell) -> bool,
    ) -> Option<Batch> {
        let missing = &missing;
        let missing_in = |row: u32| cols.clone().filter(move |&col| missing(Cell { col, row }));
        let first_row = rows.clone().find(|&r| missing_in(r).next().is_some())?;
        let band_rows = first_row..(first_row + max_rows.max(1)).min(rows.end);
        let in_band = || band_rows.clone().flat_map(&missing_in);
        let first_col = in_band().min()?;
        let last_col = in_band().max()?;
        let band_cols = first_col..(last_col + 1).min(first_col + max_cols.max(1));
        let rect = |c: &Range<u32>, r: &Range<u32>| {
            let top_left = self.rect(Cell {
                col: c.start,
                row: r.start,
            });
            let bottom_right = self.rect(Cell {
                col: c.end - 1,
                row: r.end - 1,
            });
            PixelRect::new(
                top_left.x,
                top_left.y,
                bottom_right.x + bottom_right.width - top_left.x,
                bottom_right.y + bottom_right.height - top_left.y,
            )
        };
        Some(Batch {
            region: rect(&band_cols, &band_rows),
            cols: band_cols,
            rows: band_rows,
        })
    }
}

/// An area of a page in device pixels, as floats (viewports rarely align to pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Tiles to render together: their cells, and the pixels covering them exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub cols: Range<u32>,
    pub rows: Range<u32>,
    pub region: PixelRect,
}

impl Batch {
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.rows
            .clone()
            .flat_map(|row| self.cols.clone().map(move |col| Cell { col, row }))
    }
}

/// A tile's pixels, tightly packed RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilePixels {
    pub cell: Cell,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Cuts the pixels rendered for `batch` (RGBA, `batch.region` sized) into its tiles.
pub fn split(grid: &Grid, batch: &Batch, rgba: &[u8]) -> Vec<TilePixels> {
    let stride = batch.region.width as usize * 4;
    if rgba.len() < stride * batch.region.height as usize {
        return Vec::new();
    }
    batch
        .cells()
        .map(|cell| {
            let r = grid.rect(cell);
            let (x, y) = (r.x - batch.region.x, r.y - batch.region.y);
            let mut out = Vec::with_capacity(r.width as usize * r.height as usize * 4);
            for row in y..y + r.height {
                let start = row as usize * stride + x as usize * 4;
                out.extend_from_slice(&rgba[start..start + r.width as usize * 4]);
            }
            TilePixels {
                cell,
                width: r.width,
                height: r.height,
                rgba: out,
            }
        })
        .collect()
}

/// Rendered tiles under a byte budget.
#[derive(Debug)]
pub struct TileCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    bytes: usize,
    clock: u64,
}

#[derive(Debug)]
struct Entry<V> {
    value: V,
    bytes: usize,
    used: u64,
}

impl<K, V> Default for TileCache<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
        }
    }
}

impl<K: Hash + Eq + Clone, V> TileCache<K, V> {
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.clock += 1;
        let entry = Entry {
            value,
            bytes,
            used: self.clock,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
    }

    /// A tile, marked as just used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(&entry.value)
    }

    /// A tile, without marking it used (for drawing from `&self`).
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Marks tiles as used without returning them.
    pub fn touch<'a>(&mut self, keys: impl IntoIterator<Item = &'a K>)
    where
        K: 'a,
    {
        for key in keys {
            self.clock += 1;
            if let Some(entry) = self.entries.get_mut(key) {
                entry.used = self.clock;
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops every tile `keep` refuses.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut freed = 0;
        self.entries.retain(|k, e| {
            let kept = keep(k);
            if !kept {
                freed += e.bytes;
            }
            kept
        });
        self.bytes -= freed;
    }

    /// Evicts the least recently used tiles until the cache fits `budget` bytes, never those
    /// `pinned` (tiles on screen), even if that leaves it over budget.
    pub fn evict(&mut self, budget: usize, pinned: impl Fn(&K) -> bool) {
        if self.bytes <= budget {
            return;
        }
        let mut order: Vec<(u64, K)> = self
            .entries
            .iter()
            .filter(|(k, _)| !pinned(k))
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        order.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in order {
            if self.bytes <= budget {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(x0: f32, y0: f32, x1: f32, y1: f32) -> Region {
        Region { x0, y0, x1, y1 }
    }

    #[test]
    fn grids_have_smaller_edge_tiles() {
        let g = Grid::new(1100, 600);
        assert_eq!((g.cols(), g.rows()), (3, 2));
        assert_eq!(
            g.rect(Cell { col: 2, row: 1 }),
            PixelRect::new(1024, 512, 76, 88)
        );
    }

    #[test]
    fn the_view_picks_the_tiles_it_overlaps() {
        let g = Grid::new(2048, 4096);
        assert_eq!(g.covering(view(0.0, 0.0, 600.0, 600.0)), (0..2, 0..2));
        assert_eq!(
            g.covering(view(512.0, 1000.0, 1024.0, 1100.0)),
            (1..2, 1..3)
        );
        // Past the page on every side: clamped.
        assert_eq!(g.covering(view(-100.0, -100.0, 9e4, 9e4)), (0..4, 0..8));
        // Entirely below the page: nothing.
        let (cols, rows) = g.covering(view(0.0, 5000.0, 100.0, 5100.0));
        assert!(rows.is_empty(), "{cols:?} {rows:?}");
    }

    #[test]
    fn batches_are_bands_of_missing_tiles() {
        let g = Grid::new(4096, 4096);
        let have = [Cell { col: 0, row: 0 }, Cell { col: 1, row: 0 }];
        let batch = g
            .next_batch(0..4, 0..6, 8, 2, |c| !have.contains(&c))
            .expect("something is missing");
        // Row 0 still misses columns 2–3; the band takes rows 0–1 from column 0 (row 1 misses
        // everything).
        assert_eq!((batch.cols.clone(), batch.rows.clone()), (0..4, 0..2));
        assert_eq!(batch.region, PixelRect::new(0, 0, 2048, 1024));
        assert_eq!(batch.cells().count(), 8);
        // Capped columns.
        let narrow = g
            .next_batch(0..8, 0..8, 3, 1, |_| true)
            .expect("all missing");
        assert_eq!(narrow.cols, 0..3);
        assert!(g.next_batch(0..4, 0..4, 8, 2, |_| false).is_none());
    }

    #[test]
    fn a_band_splits_back_into_its_tiles() {
        let g = Grid::new(700, 600);
        let batch = g
            .next_batch(0..2, 0..2, 4, 4, |_| true)
            .expect("all missing");
        assert_eq!(batch.region, PixelRect::new(0, 0, 700, 600));
        // Each pixel holds its own x, y.
        let rgba: Vec<u8> = (0..600u32)
            .flat_map(|y| (0..700u32).flat_map(move |x| [(x % 256) as u8, (y % 256) as u8, 0, 255]))
            .collect();
        let tiles = split(&g, &batch, &rgba);
        assert_eq!(tiles.len(), 4);
        let t = tiles
            .iter()
            .find(|t| t.cell == Cell { col: 1, row: 1 })
            .expect("tile");
        assert_eq!((t.width, t.height), (188, 88));
        // Its first pixel is page pixel (512, 512).
        assert_eq!(&t.rgba[..2], &[0, 0]);
        assert_eq!(&t.rgba[4..6], &[1, 0]);
        assert!(
            split(&g, &batch, &rgba[..10]).is_empty(),
            "short input is refused"
        );
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_but_never_pinned_tiles() {
        let mut c: TileCache<u32, ()> = TileCache::default();
        for k in 0..5 {
            c.insert(k, (), 10);
        }
        assert_eq!(c.bytes(), 50);
        c.get(&0); // 0 is now the most recent
        c.evict(30, |&k| k == 1);
        assert!(c.contains(&0) && c.contains(&1), "recent and pinned stay");
        assert!(!c.contains(&2) && !c.contains(&3));
        assert_eq!(c.bytes(), 30);
        c.insert(0, (), 15);
        assert_eq!(c.bytes(), 35, "replacing counts the new size only");
        c.retain(|&k| k != 4);
        assert_eq!((c.len(), c.bytes()), (2, 25));
        // Everything pinned: over budget rather than blank.
        c.evict(0, |_| true);
        assert_eq!(c.len(), 2);
    }
}
