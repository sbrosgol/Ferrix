//! Images to PDF, one image per page.

use std::{io::Cursor, path::PathBuf};

use ferrix_pdf::{ImageData, ImagePage, PaperSize};
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader,
    codecs::jpeg::{JpegDecoder, JpegEncoder},
    metadata::Orientation,
};

use crate::ConvertError;

/// Images are placed at 96 pixels per inch …
const POINTS_PER_PIXEL: f32 = 72.0 / 96.0;
/// … but never on a page larger than A4 (in the image's orientation).
const MAX_PAGE: PaperSize = PaperSize::A4;
/// Quality used only when a JPEG must be re-encoded to apply its EXIF orientation.
const JPEG_QUALITY: u8 = 92;

/// Builds a PDF with one page per image file. Blocking.
pub fn images_to_pdf(paths: &[PathBuf]) -> Result<Vec<u8>, ConvertError> {
    let pages = paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)?;
            let image = image_data(bytes)?;
            let (width, height) = match &image {
                ImageData::Jpeg { width, height, .. } | ImageData::Rgb { width, height, .. } => {
                    (*width, *height)
                }
            };
            Ok(ImagePage {
                image,
                size: page_size(width, height),
            })
        })
        .collect::<Result<Vec<_>, ConvertError>>()?;
    ferrix_pdf::image_document(&pages).map_err(|e| ConvertError::Image(e.to_string()))
}

fn image_data(bytes: Vec<u8>) -> Result<ImageData, ConvertError> {
    let error = |e: image::ImageError| ConvertError::Image(e.to_string());
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| ConvertError::Image(e.to_string()))?;

    if reader.format() == Some(ImageFormat::Jpeg) {
        let mut decoder = JpegDecoder::new(Cursor::new(&bytes)).map_err(error)?;
        let (width, height) = decoder.dimensions();
        let upright = decoder
            .orientation()
            .map_or(true, |o| o == Orientation::NoTransforms);
        let components = match decoder.original_color_type() {
            ExtendedColorType::L8 => Some(1),
            ExtendedColorType::Rgb8 => Some(3),
            _ => None, // CMYK and friends: decode to RGB below
        };
        if let (true, Some(components)) = (upright, components) {
            // Embed the original JPEG untouched: no quality loss, small file.
            return Ok(ImageData::Jpeg {
                bytes,
                width,
                height,
                components,
            });
        }
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
        image.apply_orientation(orientation);
        let rgb = image.to_rgb8();
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
            .encode_image(&rgb)
            .map_err(error)?;
        return Ok(ImageData::Jpeg {
            bytes: encoded,
            width: rgb.width(),
            height: rgb.height(),
            components: 3,
        });
    }

    let image = reader.decode().map_err(error)?;
    let (width, height) = (image.width(), image.height());
    if image.color().has_alpha() {
        let rgba = image.to_rgba8().into_raw();
        let (pixels, alpha) = rgba.as_chunks::<4>().0.iter().fold(
            (
                Vec::with_capacity(rgba.len() / 4 * 3),
                Vec::with_capacity(rgba.len() / 4),
            ),
            |(mut rgb, mut a), px| {
                rgb.extend_from_slice(&px[..3]);
                a.push(px[3]);
                (rgb, a)
            },
        );
        // Fully opaque images don't need a mask.
        let alpha = alpha.iter().any(|&a| a != 255).then_some(alpha);
        Ok(ImageData::Rgb {
            width,
            height,
            pixels,
            alpha,
        })
    } else {
        Ok(ImageData::Rgb {
            width,
            height,
            pixels: image.to_rgb8().into_raw(),
            alpha: None,
        })
    }
}

/// Natural size at 96 dpi, shrunk to fit an A4 page in the image's own orientation.
fn page_size(width: u32, height: u32) -> PaperSize {
    let (w, h) = (
        width as f32 * POINTS_PER_PIXEL,
        height as f32 * POINTS_PER_PIXEL,
    );
    let bounds = if w > h {
        MAX_PAGE.landscape()
    } else {
        MAX_PAGE
    };
    let fit = (bounds.width / w).min(bounds.height / h).min(1.0);
    PaperSize::new(w * fit, h * fit)
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    use super::*;

    fn scratch_file(
        name: &str,
        write: impl FnOnce(&std::path::Path),
    ) -> (crate::ScratchDir, PathBuf) {
        let dir = crate::ScratchDir::new().expect("scratch");
        let path = dir.path().join(name);
        write(&path);
        (dir, path)
    }

    #[test]
    fn small_images_keep_their_size_and_photos_fit_a4() {
        let small = page_size(400, 300);
        assert_eq!((small.width, small.height), (300.0, 225.0));
        let photo = page_size(4000, 3000);
        // 4:3 is narrower than landscape A4, so the height is the limiting side.
        assert!((photo.height - 595.28).abs() < 0.01, "{photo:?}");
        assert!(photo.width <= 841.89);
    }

    #[test]
    fn jpegs_are_embedded_without_reencoding() {
        let (_dir, path) = scratch_file("photo.jpg", |p| {
            RgbImage::from_pixel(64, 32, Rgb([200, 30, 30]))
                .save(p)
                .expect("save jpeg");
        });
        let original = std::fs::read(&path).expect("read");
        let pdf = images_to_pdf(&[path]).expect("converts");
        let embedded = pdf
            .windows(original.len())
            .any(|w| w == original.as_slice());
        assert!(embedded, "the original JPEG bytes should appear in the PDF");
    }

    #[test]
    fn transparent_pngs_convert_with_a_mask() {
        let (_dir, path) = scratch_file("logo.png", |p| {
            let mut img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 255, 255]));
            img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
            img.save(p).expect("save png");
        });
        let pdf = images_to_pdf(&[path]).expect("converts");
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.windows(5).any(|w| w == b"SMask"));
    }

    #[test]
    fn unreadable_images_are_reported() {
        let (_dir, path) = scratch_file("broken.png", |p| {
            std::fs::write(p, b"not an image").expect("write");
        });
        assert!(matches!(
            images_to_pdf(&[path]),
            Err(ConvertError::Image(_))
        ));
    }
}
