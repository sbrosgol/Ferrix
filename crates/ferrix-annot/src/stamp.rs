//! Stamps (ADR-0009 phase 4): the standard set with Ferrix's own look, and image stamps.
//! Labels are set by `ferrix-compose` and pictures drawn by krilla; both pages become inner
//! forms of the stamp's appearance, as text boxes do.

use ferrix_core::{ImageFormat, Rgb, StampImage};

use crate::AnnotError;

/// The standard stamps Ferrix offers: name in ISO 32000-2 Table 181, label, ink.
pub const STANDARD: [(&str, &str, Rgb); 7] = [
    ("Approved", "APPROVED", GREEN),
    ("NotApproved", "NOT APPROVED", RED),
    ("Draft", "DRAFT", BLUE),
    ("Final", "FINAL", GREEN),
    ("ForComment", "FOR COMMENT", BLUE),
    ("Confidential", "CONFIDENTIAL", RED),
    ("ForPublicRelease", "FOR PUBLIC RELEASE", GREEN),
];
// Darker than the annotation palette so labels reach 5:1 or more on paper.
const GREEN: Rgb = Rgb::new(
    0x2B as f32 / 255.0,
    0x7A as f32 / 255.0,
    0x3B as f32 / 255.0,
);
const RED: Rgb = Rgb::new(
    0xC9 as f32 / 255.0,
    0x2A as f32 / 255.0,
    0x2A as f32 / 255.0,
);
const BLUE: Rgb = Rgb::new(
    0x18 as f32 / 255.0,
    0x64 as f32 / 255.0,
    0xAB as f32 / 255.0,
);

/// Label size, border width and padding of a standard stamp at its natural size, in points.
pub const LABEL_SIZE: f32 = 18.0;
pub const BORDER: f32 = 2.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;
/// Pictures larger than this on a side, or than this many pixels, are refused (memory).
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 40_000_000;

/// The label and ink of a standard stamp, by name.
pub fn standard(name: &str) -> Option<(&'static str, Rgb)> {
    STANDARD
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, label, ink)| (*label, *ink))
}

/// The natural size of a standard stamp, border included.
pub fn standard_size(name: &str) -> (f32, f32) {
    let label = standard(name).map_or(name, |(l, _)| l);
    let (w, h) = crate::free_text::measure_with(label, LABEL_SIZE, None, true);
    (w + 2.0 * (PAD_X + BORDER), h + 2.0 * (PAD_Y + BORDER))
}

/// How much a standard stamp's `rect` is scaled from its natural size (the smaller axis).
pub fn scale(name: &str, rect: ferrix_types::PdfBox) -> f32 {
    let (w, h) = standard_size(name);
    (rect.width() / w).min(rect.height() / h).max(0.01)
}

/// The label set alone, as a one-page PDF of its natural text size.
pub(crate) fn render_label(name: &str, icc: bool) -> Result<(Vec<u8>, f32, f32), AnnotError> {
    let (label, ink) = standard(name).unwrap_or(("STAMP", Rgb::BLACK));
    let (w, h) = crate::free_text::measure_with(label, LABEL_SIZE, None, true);
    let pdf = crate::free_text::render_with(label, LABEL_SIZE, ink, w, h, true, icc)?;
    Ok((pdf, w, h))
}

/// Padding and border around a label, for placing it inside the stamp's box.
pub(crate) fn inset() -> (f32, f32) {
    (PAD_X + BORDER, PAD_Y + BORDER)
}

/// Reads a PNG or JPEG's size and checks it can be used, without decoding it.
pub fn image_from_bytes(data: Vec<u8>) -> Result<StampImage, AnnotError> {
    let refuse = |why: &str| Err(AnnotError::NotAllowed(why.into()));
    if data.len() > ferrix_core::MAX_STAMP_IMAGE_BYTES {
        return refuse("the image is larger than 20 MB");
    }
    let (format, width, height) = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let be = |i: usize| {
            data.get(i..i + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        match (be(16), be(20)) {
            (Some(w), Some(h)) => (ImageFormat::Png, w, h),
            _ => return refuse("the PNG image is damaged"),
        }
    } else if data.starts_with(&[0xFF, 0xD8]) {
        match jpeg_info(&data) {
            Some((w, h, components)) => {
                if components == 4 {
                    // CMYK JPEGs come out inverted from krilla without Adobe's marker.
                    return refuse("CMYK JPEG images aren't supported yet; save it as RGB");
                }
                (ImageFormat::Jpeg, w, h)
            }
            None => return refuse("the JPEG image is damaged"),
        }
    } else {
        return refuse("choose a PNG or JPEG image");
    };
    if width == 0 || height == 0 {
        return refuse("the image is empty");
    }
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return refuse("the image is too large; use one under 8000 pixels on a side");
    }
    Ok(StampImage {
        data: data.into(),
        format,
        width,
        height,
    })
}

/// Width, height and colour components from a JPEG's start-of-frame marker.
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        // SOF0–SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]);
            let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]);
            return Some((u32::from(w), u32::from(h), *data.get(i + 9)?));
        }
        i += 2 + len;
    }
    None
}

/// Whether a PNG has transparency (an alpha channel or a tRNS chunk).
pub fn has_alpha(image: &StampImage) -> bool {
    if image.format != ImageFormat::Png {
        return false;
    }
    let color_type = image.data.get(25).copied().unwrap_or(0);
    matches!(color_type, 4 | 6) || image.data.windows(4).any(|w| w == b"tRNS")
}

/// The picture drawn on a page of its pixel size (in points), for an inner form.
pub(crate) fn render_image(image: &StampImage, icc: bool) -> Result<Vec<u8>, AnnotError> {
    use krilla::{SerializeSettings, geom::Size, image::Image, page::PageSettings};
    let fail = |e: String| AnnotError::NotAllowed(format!("couldn’t use the image: {e}"));
    let data: krilla::Data = image.data.to_vec().into();
    let picture = match image.format {
        ImageFormat::Png => Image::from_png(data, false),
        ImageFormat::Jpeg => Image::from_jpeg(data, false),
    }
    .map_err(fail)?;
    let (w, h) = (image.width as f32, image.height as f32);
    let mut pdf = krilla::Document::new_with(SerializeSettings {
        enable_tagging: false,
        no_device_cs: icc,
        ..SerializeSettings::default()
    });
    let settings = PageSettings::from_wh(w, h).ok_or_else(|| fail("invalid size".into()))?;
    let mut page = pdf.start_page_with(settings);
    let mut surface = page.surface();
    let size = Size::from_wh(w, h).ok_or_else(|| fail("invalid size".into()))?;
    surface.draw_image(picture, size);
    surface.finish();
    page.finish();
    pdf.finish().map_err(|e| fail(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_stamps_are_wider_than_tall_and_known() {
        for (name, _, _) in STANDARD {
            let (w, h) = standard_size(name);
            assert!(w > h && h > LABEL_SIZE, "{name}: {w}×{h}");
        }
        assert!(standard("Approved").is_some() && standard("Nope").is_none());
    }

    #[test]
    fn image_headers_are_read_without_decoding() {
        // A 1×1 RGBA PNG header (the IHDR is all that's read).
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend([0, 0, 0, 3, 0, 0, 0, 2, 8, 6, 0, 0, 0]);
        let image = image_from_bytes(png).expect("png");
        assert_eq!(
            (image.width, image.height, image.format),
            (3, 2, ImageFormat::Png)
        );
        assert!(has_alpha(&image));
        assert!(image_from_bytes(b"GIF89a".to_vec()).is_err());
    }
}
