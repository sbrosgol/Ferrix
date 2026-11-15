//! Creating new documents: blank pages, and pages made from images.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use crate::PdfError;

/// A page size in points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaperSize {
    pub width: f32,
    pub height: f32,
}

impl PaperSize {
    pub const LETTER: Self = Self::new(612.0, 792.0);
    pub const LEGAL: Self = Self::new(612.0, 1008.0);
    pub const A4: Self = Self::new(595.28, 841.89);
    pub const A5: Self = Self::new(419.53, 595.28);

    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn landscape(self) -> Self {
        Self::new(self.width.max(self.height), self.width.min(self.height))
    }
}

/// Pixels for one image page.
#[derive(Debug, Clone)]
pub enum ImageData {
    /// A baseline or progressive JPEG with 1 (gray) or 3 (RGB) components, embedded unchanged.
    Jpeg {
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        components: u8,
    },
    /// 8-bit RGB pixels, plus optional 8-bit alpha, compressed losslessly.
    Rgb {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        alpha: Option<Vec<u8>>,
    },
}

/// An image filling a page of the given size.
#[derive(Debug, Clone)]
pub struct ImagePage {
    pub image: ImageData,
    pub size: PaperSize,
}

/// A new document of `pages` empty pages.
pub fn blank_document(pages: u32, size: PaperSize) -> Result<Vec<u8>, PdfError> {
    if pages == 0 {
        return Err(PdfError::InvalidLayout(
            "a document needs at least one page".into(),
        ));
    }
    let mut writer = Writer::new();
    for _ in 0..pages {
        let contents = writer
            .doc
            .add_object(Stream::new(Dictionary::new(), Vec::new()));
        writer.add_page(size, contents, Dictionary::new());
    }
    writer.finish()
}

/// A new document with one image per page, each scaled to fill its page.
pub fn image_document(pages: &[ImagePage]) -> Result<Vec<u8>, PdfError> {
    if pages.is_empty() {
        return Err(PdfError::InvalidLayout(
            "a document needs at least one page".into(),
        ));
    }
    let mut writer = Writer::new();
    for page in pages {
        let image = writer.add_image(&page.image)?;
        let PaperSize { width, height } = page.size;
        let mut content = Stream::new(
            Dictionary::new(),
            format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q").into_bytes(),
        );
        content
            .compress()
            .map_err(|e| PdfError::Write(e.to_string()))?;
        let contents = writer.doc.add_object(content);
        let resources = dictionary! { "XObject" => dictionary! { "Im0" => image } };
        writer.add_page(page.size, contents, resources);
    }
    writer.finish()
}

struct Writer {
    doc: Document,
    pages_id: ObjectId,
    kids: Vec<Object>,
}

impl Writer {
    fn new() -> Self {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        Self {
            doc,
            pages_id,
            kids: Vec::new(),
        }
    }

    fn add_page(&mut self, size: PaperSize, contents: ObjectId, resources: Dictionary) {
        let page = self.doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), size.width.into(), size.height.into()],
            "Resources" => resources,
            "Contents" => contents,
        });
        self.kids.push(Object::Reference(page));
    }

    fn add_image(&mut self, image: &ImageData) -> Result<ObjectId, PdfError> {
        let stream = match image {
            ImageData::Jpeg {
                bytes,
                width,
                height,
                components,
            } => {
                let color_space = match components {
                    1 => "DeviceGray",
                    3 => "DeviceRGB",
                    n => {
                        return Err(PdfError::InvalidLayout(format!(
                            "JPEG with {n} components must be converted to RGB first"
                        )));
                    }
                };
                Stream::new(
                    dictionary! {
                        "Type" => "XObject", "Subtype" => "Image",
                        "Width" => i64::from(*width), "Height" => i64::from(*height),
                        "ColorSpace" => color_space, "BitsPerComponent" => 8,
                        "Filter" => "DCTDecode",
                    },
                    bytes.clone(),
                )
                .with_compression(false)
            }
            ImageData::Rgb {
                width,
                height,
                pixels,
                alpha,
            } => {
                let expected = *width as usize * *height as usize;
                if pixels.len() != expected * 3
                    || alpha.as_ref().is_some_and(|a| a.len() != expected)
                {
                    return Err(PdfError::InvalidLayout("image buffer size mismatch".into()));
                }
                let mut dict = dictionary! {
                    "Type" => "XObject", "Subtype" => "Image",
                    "Width" => i64::from(*width), "Height" => i64::from(*height),
                    "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
                };
                if let Some(alpha) = alpha {
                    let mut mask = Stream::new(
                        dictionary! {
                            "Type" => "XObject", "Subtype" => "Image",
                            "Width" => i64::from(*width), "Height" => i64::from(*height),
                            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
                        },
                        alpha.clone(),
                    );
                    mask.compress()
                        .map_err(|e| PdfError::Write(e.to_string()))?;
                    dict.set("SMask", self.doc.add_object(mask));
                }
                let mut stream = Stream::new(dict, pixels.clone());
                stream
                    .compress()
                    .map_err(|e| PdfError::Write(e.to_string()))?;
                stream
            }
        };
        Ok(self.doc.add_object(stream))
    }

    fn finish(mut self) -> Result<Vec<u8>, PdfError> {
        let count = self.kids.len() as i64;
        self.doc.objects.insert(
            self.pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => self.kids, "Count" => count,
            }),
        );
        let catalog = self
            .doc
            .add_object(dictionary! { "Type" => "Catalog", "Pages" => self.pages_id });
        let info = self.doc.add_object(dictionary! {
            "Producer" => Object::string_literal("Ferrix"),
        });
        self.doc.trailer.set("Root", catalog);
        self.doc.trailer.set("Info", info);

        let mut out = Vec::new();
        self.doc
            .save_to(&mut out)
            .map_err(|e| PdfError::Write(e.to_string()))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_document_has_the_requested_pages() {
        let bytes = blank_document(3, PaperSize::A4.landscape()).expect("creates");
        let doc = crate::parse(&bytes, None).expect("parses");
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 3);
        let first = doc.get_dictionary(pages[&1]).expect("page");
        let media = first
            .get(b"MediaBox")
            .expect("mediabox")
            .as_array()
            .expect("array");
        assert_eq!(media[2].as_float().expect("width"), 841.89);
    }

    #[test]
    fn rgb_images_get_a_soft_mask_for_alpha() {
        let page = ImagePage {
            image: ImageData::Rgb {
                width: 2,
                height: 1,
                pixels: vec![255, 0, 0, 0, 0, 255],
                alpha: Some(vec![255, 0]),
            },
            size: PaperSize::new(144.0, 72.0),
        };
        let bytes = image_document(&[page]).expect("creates");
        let doc = crate::parse(&bytes, None).expect("parses");
        let masks = doc
            .objects
            .values()
            .filter(|o| matches!(o, Object::Stream(s) if s.dict.has(b"SMask")))
            .count();
        assert_eq!(masks, 1);
    }

    #[test]
    fn rejects_empty_documents_and_bad_buffers() {
        assert!(blank_document(0, PaperSize::LETTER).is_err());
        assert!(image_document(&[]).is_err());
        let bad = ImagePage {
            image: ImageData::Rgb {
                width: 10,
                height: 10,
                pixels: vec![0; 3],
                alpha: None,
            },
            size: PaperSize::LETTER,
        };
        assert!(image_document(&[bad]).is_err());
    }
}
