use serde::{Deserialize, Serialize};

use crate::{PdfBox, PdfPoint, Point, Rect, Size};

/// Clockwise page rotation in quarter turns, matching the PDF `/Rotate` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    /// Normalizes any multiple of 90 (including negatives); `None` otherwise.
    pub fn from_degrees(degrees: i32) -> Option<Self> {
        match degrees.rem_euclid(360) {
            0 => Some(Self::R0),
            90 => Some(Self::R90),
            180 => Some(Self::R180),
            270 => Some(Self::R270),
            _ => None,
        }
    }

    pub fn degrees(self) -> i32 {
        match self {
            Self::R0 => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }

    /// Composes two rotations.
    pub fn then(self, other: Self) -> Self {
        Self::from_degrees(self.degrees() + other.degrees()).unwrap_or_default()
    }

    /// True for 90° and 270°, which swap width and height.
    pub fn is_sideways(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

/// Maps PDF user space to page space for one page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageTransform {
    crop: PdfBox,
    rotation: Rotation,
}

impl PageTransform {
    pub fn new(crop: PdfBox, rotation: Rotation) -> Self {
        Self { crop, rotation }
    }

    /// The same page shown with `by` more clockwise rotation (e.g. a page rotated in Ferrix).
    pub fn rotated(self, by: Rotation) -> Self {
        Self {
            crop: self.crop,
            rotation: self.rotation.then(by),
        }
    }

    /// Displayed page size in points.
    pub fn page_size(&self) -> Size {
        let (w, h) = (self.crop.width(), self.crop.height());
        if self.rotation.is_sideways() {
            Size::new(h, w)
        } else {
            Size::new(w, h)
        }
    }

    pub fn to_page(&self, x: f32, y: f32) -> Point {
        let PdfBox {
            left,
            bottom,
            right,
            top,
        } = self.crop;
        match self.rotation {
            Rotation::R0 => Point::new(x - left, top - y),
            Rotation::R90 => Point::new(y - bottom, x - left),
            Rotation::R180 => Point::new(right - x, y - bottom),
            Rotation::R270 => Point::new(top - y, right - x),
        }
    }

    pub fn box_to_page(&self, b: PdfBox) -> Rect {
        Rect::from_corners(self.to_page(b.left, b.bottom), self.to_page(b.right, b.top))
    }

    /// The inverse of [`Self::to_page`]: a page-space point in PDF user space.
    pub fn to_pdf(&self, p: Point) -> PdfPoint {
        let PdfBox {
            left,
            bottom,
            right,
            top,
        } = self.crop;
        match self.rotation {
            Rotation::R0 => PdfPoint::new(p.x + left, top - p.y),
            Rotation::R90 => PdfPoint::new(p.y + left, p.x + bottom),
            Rotation::R180 => PdfPoint::new(right - p.x, p.y + bottom),
            Rotation::R270 => PdfPoint::new(right - p.y, top - p.x),
        }
    }

    pub fn rect_to_pdf(&self, r: Rect) -> PdfBox {
        PdfBox::from_corners(
            self.to_pdf(Point::new(r.x0, r.y0)),
            self.to_pdf(Point::new(r.x1, r.y1)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: PdfBox = PdfBox {
        left: 0.0,
        bottom: 0.0,
        right: 612.0,
        top: 792.0,
    };

    #[test]
    fn rotation_normalizes_degrees() {
        assert_eq!(Rotation::from_degrees(-90), Some(Rotation::R270));
        assert_eq!(Rotation::from_degrees(450), Some(Rotation::R90));
        assert_eq!(Rotation::from_degrees(45), None);
        assert_eq!(Rotation::R270.then(Rotation::R180), Rotation::R90);
    }

    #[test]
    fn unrotated_page_flips_y() {
        let t = PageTransform::new(LETTER, Rotation::R0);
        assert_eq!(t.to_page(0.0, 792.0), Point::new(0.0, 0.0));
        assert_eq!(t.to_page(612.0, 0.0), Point::new(612.0, 792.0));
    }

    #[test]
    fn crop_box_offset_is_removed() {
        let crop = PdfBox {
            left: 50.0,
            bottom: 100.0,
            right: 550.0,
            top: 700.0,
        };
        let t = PageTransform::new(crop, Rotation::R0);
        assert_eq!(t.to_page(50.0, 700.0), Point::new(0.0, 0.0));
        assert_eq!(t.page_size(), Size::new(500.0, 600.0));
    }

    #[test]
    fn every_rotation_maps_crop_box_onto_page_rect() {
        for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let t = PageTransform::new(LETTER, rotation);
            let size = t.page_size();
            let r = t.box_to_page(LETTER);
            assert_eq!(
                r,
                Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: size.width,
                    y1: size.height
                },
                "{rotation:?}"
            );
        }
    }

    #[test]
    fn to_pdf_inverts_to_page_for_every_rotation_and_offset_crop() {
        let crop = PdfBox {
            left: 30.0,
            bottom: -40.0,
            right: 530.0,
            top: 660.0,
        };
        for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let t = PageTransform::new(crop, rotation);
            for (x, y) in [(30.0, -40.0), (530.0, 660.0), (100.0, 250.5), (512.25, 0.0)] {
                let page = t.to_page(x, y);
                assert_eq!(
                    t.to_pdf(page),
                    PdfPoint::new(x, y),
                    "{rotation:?} ({x}, {y})"
                );
            }
            let b = PdfBox {
                left: 50.0,
                bottom: 10.0,
                right: 90.0,
                top: 70.0,
            };
            assert_eq!(t.rect_to_pdf(t.box_to_page(b)), b, "{rotation:?}");
        }
    }

    #[test]
    fn rotated_90_puts_pdf_origin_top_left() {
        // Rotating clockwise moves the bottom-left corner of the sheet to the top-left.
        let t = PageTransform::new(LETTER, Rotation::R90);
        assert_eq!(t.page_size(), Size::new(792.0, 612.0));
        assert_eq!(t.to_page(0.0, 0.0), Point::new(0.0, 0.0));
        assert_eq!(t.to_page(612.0, 0.0), Point::new(0.0, 612.0));
    }
}
