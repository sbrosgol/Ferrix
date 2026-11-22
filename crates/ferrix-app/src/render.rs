//! Page rendering on the engine thread, and the tiled images the UI displays.

use ferrix_engine::{EngineError, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PixelRect, Rotation, Size};
use iced::widget::image;

/// Logical pixels per PDF point at 100% zoom (96 dpi screens, 72 points per inch).
pub(crate) const LOGICAL_PX_PER_POINT: f32 = 96.0 / 72.0;
/// A page whose longest edge on screen is at most this many device pixels renders whole, in
/// one engine call (up to ~28 MB for Letter). Larger ones get a [`BASE_EDGE`] image of the
/// whole page plus sharp tiles of the part in view.
pub(crate) const WHOLE_EDGE: u32 = 3072;
/// Longest edge of the low-resolution whole page shown under the tiles.
const BASE_EDGE: f32 = 1536.0;
/// Rendered tiles kept, in bytes; beyond it the least recently drawn go first.
pub(crate) const TILE_BUDGET: usize = 192 << 20;
/// A tile band is at most this many tiles across and down (3072 × 1024 px, 12 MB).
pub(crate) const BAND_COLS: u32 = 6;
pub(crate) const BAND_ROWS: u32 = 2;
/// Pages are shown as a grid of tiles this size. Each stays under iced's 2 MiB synchronous
/// upload limit, so a new page appears in the same frame instead of flashing blank.
const TILE_EDGE: u32 = 512;

/// A page of an open engine document, shown with an extra rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageRef {
    pub(crate) doc: DocId,
    pub(crate) page: PageIndex,
    pub(crate) rotation: Rotation,
}

/// A rendered page as rows of tiles, each with its display size in logical pixels.
#[derive(Debug)]
pub(crate) struct PageImage {
    pub(crate) rows: Vec<Vec<(image::Handle, iced::Size)>>,
    pub(crate) logical: iced::Size,
}

/// Pixels produced on the engine thread, before they become GPU image handles.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Device-pixel width of the whole page.
    width: u32,
    rows: Vec<Vec<RawTile>>,
    logical: iced::Size,
    pub(crate) zoom: f32,
    /// Rendered with inverted lightness (the dark reading mode).
    pub(crate) inverted: bool,
    /// A low-resolution image of a page too large to render whole; tiles go over it.
    pub(crate) base: bool,
}

#[derive(Debug, Clone)]
struct RawTile {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Rendered {
    pub(crate) fn into_image(self) -> PageImage {
        let px_per_logical = self.width as f32 / self.logical.width;
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|t| {
                        let size = iced::Size::new(
                            t.width as f32 / px_per_logical,
                            t.height as f32 / px_per_logical,
                        );
                        (image::Handle::from_rgba(t.width, t.height, t.pixels), size)
                    })
                    .collect()
            })
            .collect();
        PageImage {
            rows,
            logical: self.logical,
        }
    }
}

/// Displayed size of `page` in points, including its extra rotation.
pub(crate) fn page_size(engine: &mut PdfiumEngine, page: PageRef) -> Result<Size, EngineError> {
    let size = engine.page_info(page.doc, page.page)?.size;
    Ok(if page.rotation.is_sideways() {
        Size::new(size.height, size.width)
    } else {
        size
    })
}

/// Renders a whole page at `logical_scale` logical px per point on a `scale_factor` display.
pub(crate) fn render_page(
    engine: &mut PdfiumEngine,
    page: PageRef,
    logical_scale: f32,
    scale_factor: f32,
    zoom: f32,
    invert: bool,
) -> Result<Rendered, EngineError> {
    let size = page_size(engine, page)?;
    let wanted = logical_scale * scale_factor;
    let base = needs_tiles(size, wanted);
    let scale = if base {
        BASE_EDGE / size.width.max(size.height)
    } else {
        wanted
    };
    let (width, height) = size.to_pixels(scale);
    // One engine call for the whole page (PDFium re-interprets the page per call), then slice.
    let pixels = engine.render_tile(&TileRequest {
        doc: page.doc,
        page: page.page,
        scale,
        rotation: page.rotation,
        region: PixelRect::new(0, 0, width, height),
        flags: RenderFlags::default(),
    })?;
    let (width, height) = (pixels.region.width, pixels.region.height);
    let mut pixels = pixels.pixels;
    if invert {
        invert_lightness(&mut pixels);
    }
    Ok(Rendered {
        width,
        rows: slice_tiles(&pixels, width, height),
        logical: iced::Size::new(size.width * logical_scale, size.height * logical_scale),
        zoom,
        inverted: invert,
        base,
    })
}

/// Dark Pages' paper (white becomes `#1E1E1E`) and ink (black becomes `#E6E6E6`).
const DARK_PAPER: i32 = 0x1E;
const LIGHT_INK: i32 = 0xE6;

/// Turns light into dark and back while keeping hues: white paper becomes black, black text
/// white, and a red heading stays red. Shifting all three channels by one amount keeps hue and
/// chroma; this amount mirrors the HSL lightness `(max + min) / 2` around the middle.
pub(crate) fn invert_lightness(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let (r, g, b) = (i16::from(px[0]), i16::from(px[1]), i16::from(px[2]));
        let shift = 255 - r.max(g).max(b) - r.min(g).min(b);
        // Each channel lies between min and max, so the result stays within 0..=255. Then
        // squeeze into DARK_PAPER..=LIGHT_INK: pure black paper and white text glare.
        for c in &mut px[..3] {
            let inverted = i32::from((i16::from(*c) + shift).clamp(0, 255));
            *c = (DARK_PAPER + inverted * (LIGHT_INK - DARK_PAPER) / 255) as u8;
        }
    }
}

/// Whether a page of `size` points at `device_scale` pixels per point is too large to render
/// whole.
pub(crate) fn needs_tiles(size: Size, device_scale: f32) -> bool {
    let (w, h) = size.to_pixels(device_scale);
    w.max(h) > WHOLE_EDGE
}

/// Which rendering of a page a tile belongs to: tiles of different levels never mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Level {
    /// The engine document rendered (the file, or an annotation preview).
    pub(crate) source: DocId,
    pub(crate) rotation: Rotation,
    /// Device pixels per point, as bits (exact equality is what's wanted).
    pub(crate) scale_bits: u32,
    pub(crate) inverted: bool,
}

impl Level {
    pub(crate) fn scale(self) -> f32 {
        f32::from_bits(self.scale_bits)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TileKey {
    pub(crate) page: ferrix_core::PageKey,
    pub(crate) level: Level,
    pub(crate) cell: ferrix_render::Cell,
}

/// A tile ready to draw, and where on the page (device pixels).
#[derive(Debug, Clone)]
pub(crate) struct Tile {
    pub(crate) handle: image::Handle,
    pub(crate) rect: PixelRect,
}

/// Renders a band of tiles of `page` at `level` in one engine call, cut into tiles.
pub(crate) fn render_band(
    engine: &mut PdfiumEngine,
    page: PageRef,
    level: Level,
    batch: &ferrix_render::Batch,
) -> Result<Vec<ferrix_render::TilePixels>, EngineError> {
    let size = page_size(engine, page)?;
    let (width, height) = size.to_pixels(level.scale());
    let rendered = engine.render_tile(&TileRequest {
        doc: page.doc,
        page: page.page,
        scale: level.scale(),
        rotation: page.rotation,
        region: batch.region,
        flags: RenderFlags::default(),
    })?;
    if rendered.region != batch.region {
        return Err(EngineError::Backend(format!(
            "asked for {:?}, got {:?}",
            batch.region, rendered.region
        )));
    }
    let mut pixels = rendered.pixels;
    if level.inverted {
        invert_lightness(&mut pixels);
    }
    let grid = ferrix_render::Grid::new(width, height);
    Ok(ferrix_render::split(&grid, batch, &pixels))
}

/// Tiles an RGBA image for display at `logical` size, like rendered PDF pages.
pub(crate) fn tiled_image(
    pixels: &[u8],
    width: u32,
    height: u32,
    logical: iced::Size,
) -> PageImage {
    Rendered {
        rows: slice_tiles(pixels, width, height),
        width,
        logical,
        zoom: 1.0,
        inverted: false,
        base: false,
    }
    .into_image()
}

/// Splits a tightly packed RGBA image into rows of tiles of at most [`TILE_EDGE`] pixels.
fn slice_tiles(pixels: &[u8], width: u32, height: u32) -> Vec<Vec<RawTile>> {
    let stride = width as usize * 4;
    (0..height)
        .step_by(TILE_EDGE as usize)
        .map(|y| {
            let tile_h = TILE_EDGE.min(height - y);
            (0..width)
                .step_by(TILE_EDGE as usize)
                .map(|x| {
                    let tile_w = TILE_EDGE.min(width - x);
                    let mut tile = Vec::with_capacity(tile_w as usize * tile_h as usize * 4);
                    for row in y..y + tile_h {
                        let start = row as usize * stride + x as usize * 4;
                        tile.extend_from_slice(&pixels[start..start + tile_w as usize * 4]);
                    }
                    RawTile {
                        width: tile_w,
                        height: tile_h,
                        pixels: tile,
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverting_lightness_keeps_hues() {
        let mut px = [
            255, 255, 255, 255, // white paper
            0, 0, 0, 255, // black text
            255, 0, 0, 128, // red, half transparent
            200, 220, 240, 255, // pale blue
        ];
        invert_lightness(&mut px);
        assert_eq!(
            &px[..4],
            &[0x1E, 0x1E, 0x1E, 255],
            "paper is dark grey, not black"
        );
        assert_eq!(&px[4..8], &[0xE6, 0xE6, 0xE6, 255]);
        assert_eq!(
            &px[8..12],
            &[0xE6, 0x1E, 0x1E, 128],
            "red stays red; alpha is kept"
        );
        assert!(
            px[12] < px[13] && px[13] < px[14] && px[14] < 0x80,
            "pale blue becomes dark blue"
        );
    }

    #[test]
    fn tiles_cover_the_image_exactly() {
        let (w, h) = (1100, 600);
        let pixels: Vec<u8> = (0..w * h).flat_map(u32::to_le_bytes).collect();
        let rows = slice_tiles(&pixels, w, h);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].iter().map(|t| t.width).collect::<Vec<_>>(),
            [512, 512, 76]
        );
        assert_eq!(rows[1][0].height, 88);
        // The first pixel of tile (1, 2) is image pixel (1024, 512).
        let first = u32::from_le_bytes(rows[1][2].pixels[..4].try_into().expect("4 bytes"));
        assert_eq!(first, 512 * w + 1024);
    }
}
