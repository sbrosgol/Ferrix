//! Page rendering on the engine thread, and the tiled images the UI displays.

use ferrix_engine::{EngineError, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PixelRect, Rotation, Size};
use iced::widget::image;

/// Logical pixels per PDF point at 100% zoom (96 dpi screens, 72 points per inch).
pub(crate) const LOGICAL_PX_PER_POINT: f32 = 96.0 / 72.0;
/// Caps the longest edge of a rendered page (~200 MB of pixels for Letter). The M1 canvas
/// renders only visible tiles and removes this limit.
const MAX_RENDER_EDGE: f32 = 8192.0;
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
) -> Result<Rendered, EngineError> {
    let size = page_size(engine, page)?;
    let wanted = logical_scale * scale_factor;
    let longest = size.width.max(size.height) * wanted;
    let scale = wanted * (MAX_RENDER_EDGE / longest).min(1.0);
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
    Ok(Rendered {
        width,
        rows: slice_tiles(&pixels.pixels, width, height),
        logical: iced::Size::new(size.width * logical_scale, size.height * logical_scale),
        zoom,
    })
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
