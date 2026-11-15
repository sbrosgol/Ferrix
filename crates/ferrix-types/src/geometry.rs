use serde::{Deserialize, Serialize};

/// A point in page space (points, origin top-left, y down).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A width/height pair in points.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Size in whole device pixels at `scale` pixels per point (at least 1×1).
    pub fn to_pixels(self, scale: f32) -> (u32, u32) {
        let px = |v: f32| ((v * scale).round() as u32).max(1);
        (px(self.width), px(self.height))
    }
}

/// An axis-aligned rectangle in page space. Invariant: `x0 <= x1`, `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// Builds a normalized rectangle from any two opposite corners.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self {
            x0: a.x.min(b.x),
            y0: a.y.min(b.y),
            x1: a.x.max(b.x),
            y1: a.y.max(b.y),
        }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
}

/// A rectangle in PDF user space (points, origin bottom-left, y up), e.g. a crop box.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PdfBox {
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl PdfBox {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.top - self.bottom
    }
}

/// A rectangle of device pixels within a rendered page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Clamps this rectangle to a `width`×`height` pixel area, or `None` if they don't overlap.
    pub fn clamp_to(self, width: u32, height: u32) -> Option<Self> {
        let x1 = (self.x.saturating_add(self.width)).min(width);
        let y1 = (self.y.saturating_add(self.height)).min(height);
        (self.x < x1 && self.y < y1).then(|| Self::new(self.x, self.y, x1 - self.x, y1 - self.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_corners_normalizes() {
        let r = Rect::from_corners(Point::new(10.0, 2.0), Point::new(1.0, 20.0));
        assert_eq!(
            r,
            Rect {
                x0: 1.0,
                y0: 2.0,
                x1: 10.0,
                y1: 20.0
            }
        );
    }

    #[test]
    fn pixel_rect_clamps_to_page() {
        let tile = PixelRect::new(768, 1024, 256, 256);
        assert_eq!(
            tile.clamp_to(900, 1100),
            Some(PixelRect::new(768, 1024, 132, 76))
        );
        assert_eq!(tile.clamp_to(500, 500), None);
    }

    #[test]
    fn size_to_pixels_rounds_and_is_never_empty() {
        assert_eq!(Size::new(612.0, 792.0).to_pixels(2.0), (1224, 1584));
        assert_eq!(Size::new(0.1, 0.1).to_pixels(1.0), (1, 1));
    }
}
