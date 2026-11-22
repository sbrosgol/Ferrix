//! What the page view renders next: whole pages (exact, or a low-resolution base for large
//! ones), then sharp tiles of the parts of large pages in view (architecture §5.3).

use std::collections::HashSet;

use ferrix_core::PageKey;
use ferrix_render::{Grid, Region, TILE};
use ferrix_types::DocId;
use iced::Task;

use crate::{
    app::{App, Message},
    document::Document,
    render::{
        BAND_COLS, BAND_ROWS, LOGICAL_PX_PER_POINT, Level, TILE_BUDGET, Tile, TileKey, needs_tiles,
        render_band, render_page,
    },
    strip::PAGE_MARGIN,
    worker::flatten,
};

/// The tiles one page of the view needs.
#[derive(Debug, Clone)]
pub(crate) struct TilePlan {
    pub(crate) level: Level,
    pub(crate) grid: Grid,
    pub(crate) cols: std::ops::Range<u32>,
    pub(crate) rows: std::ops::Range<u32>,
}

impl TilePlan {
    pub(crate) fn keys(&self, page: PageKey) -> impl Iterator<Item = TileKey> + '_ {
        self.rows.clone().flat_map(move |row| {
            self.cols.clone().map(move |col| TileKey {
                page,
                level: self.level,
                cell: ferrix_render::Cell { col, row },
            })
        })
    }
}

impl App {
    /// Width of the scrolled page view, in logical pixels.
    pub(crate) fn viewport_width(&self) -> f32 {
        self.canvas_size().width + 2.0 * PAGE_MARGIN
    }

    /// Device pixels per point at the current zoom.
    fn device_scale(&self, doc: &Document) -> f32 {
        doc.strip.zoom * LOGICAL_PX_PER_POINT * self.scale_factor
    }

    /// The tiles of page `index` that the view shows (plus half a tile around), if the page
    /// is too large to render whole.
    pub(crate) fn tile_plan(&self, doc: &Document, index: usize) -> Option<TilePlan> {
        let entry = doc.state.pages.get(index)?;
        let page = doc.page_ref(entry)?;
        let scale = self.device_scale(doc);
        let size = doc.page_size(entry);
        if !needs_tiles(size, scale) {
            return None;
        }
        let (width, height) = size.to_pixels(scale);
        let grid = Grid::new(width, height);
        let (view_w, view_h) = (self.viewport_width(), self.viewport_height());
        // Content narrower than the view is centred and shown whole across.
        let (x0, x1) = if doc.strip.content.width <= view_w {
            (f32::NEG_INFINITY, f32::INFINITY)
        } else {
            (doc.scroll_x, doc.scroll_x + view_w)
        };
        let (top, left) = (*doc.strip.tops.get(index)?, *doc.strip.lefts.get(index)?);
        let sf = self.scale_factor;
        let margin = TILE as f32 / 2.0;
        let view = Region {
            x0: (x0 - left) * sf - margin,
            y0: (doc.scroll - top) * sf - margin,
            x1: (x1 - left) * sf + margin,
            y1: (doc.scroll + view_h - top) * sf + margin,
        };
        let (cols, rows) = grid.covering(view);
        if cols.is_empty() || rows.is_empty() {
            return None;
        }
        Some(TilePlan {
            level: Level {
                source: page.doc,
                rotation: entry.rotation,
                scale_bits: scale.to_bits(),
                inverted: self.invert,
            },
            grid,
            cols,
            rows,
        })
    }

    /// Renders the next thing the view lacks; each result schedules the one after it.
    /// Visible pages get a whole image first (so nothing is blank), then their tiles, then the
    /// neighbours get whole images. Pages further away are dropped.
    pub(crate) fn render_pages(&mut self) -> Task<Message> {
        let (height, scale_factor, invert) =
            (self.viewport_height(), self.scale_factor, self.invert);
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        let Some(doc) = self.doc.as_ref().filter(|d| d.is_measured()) else {
            return Task::none();
        };
        let visible = doc.strip.visible(doc.scroll, height);
        // One page either side, so a short scroll finds it ready.
        let near = visible.start.saturating_sub(1)..(visible.end + 1).min(doc.len());
        let plans: Vec<(PageKey, TilePlan)> = visible
            .clone()
            .filter_map(|i| Some((doc.key_at(i)?, self.tile_plan(doc, i)?)))
            .collect();
        let zoom = doc.strip.zoom;
        let device_scale = self.device_scale(doc);

        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let keep: HashSet<PageKey> = near.clone().filter_map(|i| doc.key_at(i)).collect();
        doc.images.retain(|k, _| keep.contains(k));
        // Tiles on screen are the most recently used; they are never evicted.
        for (key, plan) in &plans {
            let keys: Vec<TileKey> = plan.keys(*key).collect();
            doc.tiles.touch(&keys);
        }
        if doc.render_busy {
            return Task::none();
        }

        let whole_needed = |doc: &Document, i: usize| {
            let entry = doc.state.pages.get(i)?;
            if doc.render_failed.contains(&entry.key) {
                return None;
            }
            let page = doc.page_ref(entry)?;
            let tiled = needs_tiles(doc.page_size(entry), device_scale);
            // Stale once the source shows a newer annotation preview (ADR-0009 §7). Under
            // tiles any rendering will do as the backdrop; otherwise it must be exact.
            let fresh = doc.images.get(&entry.key).is_some_and(|s| {
                s.rotation == entry.rotation
                    && s.source == page.doc
                    && (tiled || (!s.base && s.zoom == zoom && s.scale_factor == scale_factor))
            });
            (!fresh).then_some((entry.key, page))
        };
        let doc_id = doc.info.id;
        // 1. The current page and the rest of the view, whole.
        let first = std::iter::once(doc.current)
            .chain(visible.clone())
            .filter(|i| visible.contains(i))
            .find_map(|i| whole_needed(doc, i));
        // 2. Tiles of large pages in view.
        let band = || {
            plans.iter().find_map(|(key, plan)| {
                let batch = plan.grid.next_batch(
                    plan.cols.clone(),
                    plan.rows.clone(),
                    BAND_COLS,
                    BAND_ROWS,
                    |cell| {
                        !doc.tiles.contains(&TileKey {
                            page: *key,
                            level: plan.level,
                            cell,
                        })
                    },
                )?;
                Some((*key, plan.level, batch))
            })
        };
        if first.is_none()
            && let Some((key, level, batch)) = band()
        {
            let Some(page) = doc
                .state
                .pages
                .entries()
                .iter()
                .find(|e| e.key == key)
                .and_then(|e| doc.page_ref(e))
            else {
                return Task::none();
            };
            let Some(doc) = self.doc.as_mut() else {
                return Task::none();
            };
            doc.render_busy = true;
            return Task::perform(
                async move {
                    flatten(
                        engine
                            .call(move |e| render_band(e, page, level, &batch))
                            .await,
                    )
                },
                move |result| Message::TilesRendered(doc_id, key, level, result),
            );
        }
        // 3. The neighbours, whole.
        let next = first.or_else(|| near.clone().find_map(|i| whole_needed(doc, i)));
        let Some((key, page)) = next else {
            return Task::none();
        };
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        doc.render_busy = true;
        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        render_page(
                            e,
                            page,
                            zoom * LOGICAL_PX_PER_POINT,
                            scale_factor,
                            zoom,
                            invert,
                        )
                    })
                    .await;
                flatten(result)
            },
            move |result| {
                Message::Rendered(doc_id, key, page.rotation, page.doc, scale_factor, result)
            },
        )
    }

    pub(crate) fn tiles_rendered(
        &mut self,
        doc_id: DocId,
        key: PageKey,
        level: Level,
        result: Result<Vec<ferrix_render::TilePixels>, String>,
    ) -> Task<Message> {
        let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
            return Task::none();
        };
        doc.render_busy = false;
        match result {
            Ok(tiles) => {
                for t in tiles {
                    let rect = ferrix_types::PixelRect::new(
                        t.cell.col * TILE,
                        t.cell.row * TILE,
                        t.width,
                        t.height,
                    );
                    let bytes = t.rgba.len();
                    let handle = iced::widget::image::Handle::from_rgba(t.width, t.height, t.rgba);
                    doc.tiles.insert(
                        TileKey {
                            page: key,
                            level,
                            cell: t.cell,
                        },
                        Tile { handle, rect },
                        bytes,
                    );
                }
            }
            Err(e) => {
                let page = doc.state.pages.position(key).map_or(0, |i| i + 1);
                self.error = Some(format!("Couldn’t show page {page}: {e}"));
                doc.render_failed.insert(key);
            }
        }
        self.evict_tiles();
        Task::batch([self.render_pages(), self.load_texts()])
    }

    /// Keeps the tiles within budget, never dropping those on screen.
    fn evict_tiles(&mut self) {
        let height = self.viewport_height();
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let visible = doc.strip.visible(doc.scroll, height);
        let pinned: HashSet<TileKey> = visible
            .filter_map(|i| Some((doc.key_at(i)?, self.tile_plan(doc, i)?)))
            .flat_map(|(key, plan)| plan.keys(key).collect::<Vec<_>>())
            .collect();
        if let Some(doc) = self.doc.as_mut() {
            doc.tiles.evict(TILE_BUDGET, |k| pinned.contains(k));
        }
    }
}
