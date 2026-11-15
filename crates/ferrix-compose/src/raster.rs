//! Draws laid-out pages into pixels for the editor (ADR-0008 §5). The PDF export draws the same
//! glyphs at the same positions, so what the editor shows is what the PDF contains.

use cosmic_text::{CacheKey, CacheKeyFlags, SwashCache, SwashContent};

use crate::fonts::Fonts;
use crate::layout::{Layout, PageLayout, PlacedGlyph, PlacedImage};
use crate::model::Color;

/// An RGBA page image, premultiplied by nothing: white paper with opaque content.
#[derive(Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Rows top to bottom, 4 bytes per pixel.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Rasterizes glyphs, keeping the ones it has drawn for reuse.
pub struct Rasterizer {
    cache: SwashCache,
    /// Decoded pictures by (hash, width, height) in pixels.
    images: std::collections::HashMap<(u64, u32, u32), image::RgbaImage>,
}

impl std::fmt::Debug for Rasterizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rasterizer").finish_non_exhaustive()
    }
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Pages larger than this many pixels are not drawn (guards against absurd zoom).
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

impl Rasterizer {
    pub fn new() -> Self {
        Self {
            cache: SwashCache::new(),
            images: std::collections::HashMap::new(),
        }
    }

    /// Draws page `index` of `layout` with `scale` pixels per point.
    pub fn page(
        &mut self,
        layout: &Layout,
        index: usize,
        scale: f32,
        fonts: &mut Fonts,
    ) -> Option<Raster> {
        let page = layout.pages.get(index)?;
        let width = (layout.page.width * scale).round().max(1.0) as u32;
        let height = (layout.page.height * scale).round().max(1.0) as u32;
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return None;
        }
        let mut raster = Raster {
            width,
            height,
            pixels: vec![255; width as usize * height as usize * 4],
        };
        self.draw(&mut raster, page, scale, fonts);
        Some(raster)
    }

    fn draw(&mut self, raster: &mut Raster, page: &PageLayout, scale: f32, fonts: &mut Fonts) {
        for line in page.lines.iter().chain(&page.furniture) {
            if let Some(image) = &line.image {
                self.draw_image(raster, image, scale);
            }
            self.draw_glyphs(raster, &line.glyphs, scale, fonts);
            if let Some((_, marker)) = &line.marker {
                self.draw_glyphs(raster, marker, scale, fonts);
            }
            for bar in &line.decorations {
                fill_rect(
                    raster,
                    bar.x * scale,
                    bar.top * scale,
                    bar.width * scale,
                    (bar.thickness * scale).max(1.0),
                    bar.color,
                );
            }
        }
    }

    fn draw_glyphs(
        &mut self,
        raster: &mut Raster,
        glyphs: &[PlacedGlyph],
        scale: f32,
        fonts: &mut Fonts,
    ) {
        for glyph in glyphs {
            let (key, x, y) = CacheKey::new(
                glyph.font,
                glyph.glyph_id,
                glyph.size * scale,
                (glyph.x * scale, glyph.y * scale),
                glyph.weight,
                CacheKeyFlags::empty(),
            );
            let Some(image) = self.cache.get_image(fonts.system(), key) else {
                continue;
            };
            let left = x + image.placement.left;
            let top = y - image.placement.top;
            let (w, h) = (image.placement.width as i32, image.placement.height as i32);
            for row in 0..h {
                for col in 0..w {
                    let i = (row * w + col) as usize;
                    let (alpha, color) = match image.content {
                        SwashContent::Mask => (image.data[i], glyph.color),
                        SwashContent::Color => {
                            let p = &image.data[i * 4..i * 4 + 4];
                            (p[3], Color(p[0], p[1], p[2]))
                        }
                        SwashContent::SubpixelMask => {
                            let p = &image.data[i * 4..i * 4 + 4];
                            let a =
                                ((u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3) as u8;
                            (a, glyph.color)
                        }
                    };
                    blend(raster, left + col, top + row, color, alpha);
                }
            }
        }
    }

    /// Draws a picture, decoded and scaled once per size and kept for the next page drawn.
    fn draw_image(&mut self, raster: &mut Raster, image: &PlacedImage, scale: f32) {
        let width = (image.width * scale).round().max(1.0) as u32;
        let height = (image.height * scale).round().max(1.0) as u32;
        let key = (image.data.hash, width, height);
        if !self.images.contains_key(&key) {
            let Ok(decoded) = image::load_from_memory(&image.data.bytes) else {
                return;
            };
            let scaled = image::imageops::resize(
                &decoded.to_rgba8(),
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            // Keep the cache small: pictures at other sizes are dropped.
            self.images
                .retain(|(hash, _, _), _| *hash != image.data.hash);
            self.images.insert(key, scaled);
        }
        let Some(pixels) = self.images.get(&key) else {
            return;
        };
        let left = (image.x * scale).round() as i32;
        let top = (image.top * scale).round() as i32;
        for (col, row, p) in pixels.enumerate_pixels() {
            blend(
                raster,
                left + col as i32,
                top + row as i32,
                Color(p[0], p[1], p[2]),
                p[3],
            );
        }
    }
}

fn blend(raster: &mut Raster, x: i32, y: i32, color: Color, alpha: u8) {
    if alpha == 0 || x < 0 || y < 0 || x >= raster.width as i32 || y >= raster.height as i32 {
        return;
    }
    let i = (y as usize * raster.width as usize + x as usize) * 4;
    let a = u16::from(alpha);
    for (c, src) in [color.0, color.1, color.2].into_iter().enumerate() {
        let dst = u16::from(raster.pixels[i + c]);
        raster.pixels[i + c] = ((u16::from(src) * a + dst * (255 - a) + 127) / 255) as u8;
    }
    raster.pixels[i + 3] = 255;
}

/// Fills a rectangle, with partial coverage at its top and bottom edges.
fn fill_rect(raster: &mut Raster, x: f32, y: f32, width: f32, height: f32, color: Color) {
    let x0 = x.round() as i32;
    let x1 = (x + width).round() as i32;
    let y_start = y.floor() as i32;
    let y_end = (y + height).ceil() as i32;
    for row in y_start..y_end {
        let top = (row as f32).max(y);
        let bottom = ((row + 1) as f32).min(y + height);
        let coverage = ((bottom - top).clamp(0.0, 1.0) * 255.0).round() as u8;
        for col in x0..x1 {
            blend(raster, col, row, color, coverage);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::{CharFormat, Document, Paragraph, ParagraphStyle};

    fn ink(raster: &Raster) -> usize {
        raster.pixels.chunks(4).filter(|p| p[0] < 128).count()
    }

    #[test]
    fn draws_text_where_the_layout_puts_it() {
        let mut doc = Document::new();
        doc.paragraphs =
            vec![Paragraph::new(ParagraphStyle::Normal).with_text("Hello", CharFormat::default())];
        let mut fonts = Fonts::bundled();
        let layout = Layout::new(&doc, &mut fonts);
        let raster = Rasterizer::new().page(&layout, 0, 2.0, &mut fonts).unwrap();
        assert_eq!(raster.width, (595.28_f32 * 2.0).round() as u32);
        assert!(ink(&raster) > 50);

        // All ink sits inside the first line's box.
        let line = &layout.pages[0].lines[0];
        let (top, bottom) = (line.top * 2.0, (line.top + line.height) * 2.0);
        for (i, px) in raster.pixels.chunks(4).enumerate() {
            if px[0] < 128 {
                let y = (i / raster.width as usize) as f32;
                assert!(y >= top - 1.0 && y <= bottom + 1.0, "ink at y={y}");
            }
        }
    }

    #[test]
    fn empty_pages_are_white_and_huge_pages_are_refused() {
        let doc = Document::new();
        let mut fonts = Fonts::bundled();
        let layout = Layout::new(&doc, &mut fonts);
        let raster = Rasterizer::new().page(&layout, 0, 1.0, &mut fonts).unwrap();
        assert_eq!(ink(&raster), 0);
        assert!(
            Rasterizer::new()
                .page(&layout, 0, 100.0, &mut fonts)
                .is_none()
        );
        assert!(
            Rasterizer::new()
                .page(&layout, 5, 1.0, &mut fonts)
                .is_none()
        );
    }
}
