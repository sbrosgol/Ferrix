//! Reading things back out of a PDF: embedded files and a digest of page content (ADR-0008 §7).

use lopdf::{Dictionary, Document, Object, ObjectId};
use sha2::{Digest, Sha256};

use crate::{MAX_STREAM_BYTES, PdfError, Source, parse};

/// Name trees deeper than this are treated as malformed.
const MAX_TREE_DEPTH: usize = 16;

/// The contents of the file embedded under `name`, if the document has one.
pub fn embedded_file(source: Source<'_>, name: &str) -> Result<Option<Vec<u8>>, PdfError> {
    let doc = parse(source.bytes, source.password)?;
    let Some(tree) = (|| {
        let names = doc
            .catalog()
            .ok()?
            .get_deref(b"Names", &doc)
            .ok()?
            .as_dict()
            .ok()?;
        names.get_deref(b"EmbeddedFiles", &doc).ok()?.as_dict().ok()
    })() else {
        return Ok(None);
    };
    let Some(spec) = find_name(&doc, tree, name.as_bytes(), 0) else {
        return Ok(None);
    };
    let stream = (|| {
        let spec = doc.dereference(spec).ok()?.1.as_dict().ok()?;
        let files = spec.get_deref(b"EF", &doc).ok()?.as_dict().ok()?;
        let file = files
            .get_deref(b"UF", &doc)
            .or_else(|_| files.get_deref(b"F", &doc))
            .ok()?;
        file.as_stream().ok()
    })();
    let Some(stream) = stream else {
        return Ok(None);
    };
    let content = if stream.dict.get(b"Filter").is_ok() {
        stream.decompressed_content_with_limit(MAX_STREAM_BYTES)?
    } else {
        stream.content.clone()
    };
    Ok(Some(content))
}

/// Looks `key` up in a name tree.
fn find_name<'a>(
    doc: &'a Document,
    node: &'a Dictionary,
    key: &[u8],
    depth: usize,
) -> Option<&'a Object> {
    if depth > MAX_TREE_DEPTH {
        return None;
    }
    if let Ok(names) = node.get_deref(b"Names", doc).and_then(Object::as_array) {
        for pair in names.chunks(2) {
            if let [name, value] = pair
                && doc.dereference(name).ok()?.1.as_str().ok() == Some(key)
            {
                return Some(value);
            }
        }
    }
    let kids = node
        .get_deref(b"Kids", doc)
        .and_then(Object::as_array)
        .ok()?;
    kids.iter().find_map(|kid| {
        let kid = doc.dereference(kid).ok()?.1.as_dict().ok()?;
        find_name(doc, kid, key, depth + 1)
    })
}

/// A fingerprint of what a PDF's pages show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDigest {
    /// SHA-256 over each page's size, rotation and decoded content streams, as lowercase hex.
    pub digest: String,
    pub pages: usize,
    /// Whether any page has annotations (comments, links, form widgets...).
    pub has_annotations: bool,
}

/// Fingerprints the page content, to tell whether another application has changed it.
pub fn content_digest(source: Source<'_>) -> Result<ContentDigest, PdfError> {
    let doc = parse(source.bytes, source.password)?;
    let pages = doc.get_pages();
    let mut hasher = Sha256::new();
    let mut has_annotations = false;
    for id in pages.values() {
        let page = doc.get_dictionary(*id)?;
        if let Ok(media_box) = page.get_deref(b"MediaBox", &doc).and_then(Object::as_array) {
            for value in media_box {
                let value = doc
                    .dereference(value)
                    .map(|(_, v)| v.as_float().unwrap_or(0.0));
                hasher.update(value.unwrap_or(0.0).to_le_bytes());
            }
        }
        let rotate = page
            .get_deref(b"Rotate", &doc)
            .and_then(Object::as_i64)
            .unwrap_or(0);
        hasher.update(rotate.rem_euclid(360).to_le_bytes());
        let content = doc.get_page_content_with_limit(*id, MAX_STREAM_BYTES)?;
        hasher.update((content.len() as u64).to_le_bytes());
        hasher.update(&content);
        if let Ok(annots) = page.get_deref(b"Annots", &doc).and_then(Object::as_array) {
            has_annotations |= !annots.is_empty();
        }
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(ContentDigest {
        digest,
        pages: pages.len(),
        has_annotations,
    })
}

/// Each page's visible area (crop box clipped to the media box, inheritance resolved) and its
/// `/Rotate`, as a transform from PDF user space to page space. In page order.
pub fn page_transforms(doc: &Document) -> Vec<ferrix_types::PageTransform> {
    use ferrix_types::{PageTransform, PdfBox, Rotation};
    const LETTER: PdfBox = PdfBox {
        left: 0.0,
        bottom: 0.0,
        right: 612.0,
        top: 792.0,
    };
    let inherited = |page: ObjectId, key: &[u8]| -> Option<Object> {
        let mut node = doc.get_dictionary(page).ok()?;
        for _ in 0..MAX_TREE_DEPTH {
            if let Ok(value) = node.get(key) {
                return doc.dereference(value).ok().map(|(_, v)| v.clone());
            }
            node = doc
                .get_dictionary(node.get(b"Parent").ok()?.as_reference().ok()?)
                .ok()?;
        }
        None
    };
    let rect = |object: Option<Object>| -> Option<PdfBox> {
        let array = object?.as_array().ok()?.clone();
        let n: Vec<f32> = array
            .iter()
            .filter_map(|o| doc.dereference(o).ok()?.1.as_float().ok())
            .collect();
        let [a, b, c, d] = n.as_slice() else {
            return None;
        };
        let b = PdfBox {
            left: a.min(*c),
            bottom: b.min(*d),
            right: a.max(*c),
            top: b.max(*d),
        };
        (b.width() > 0.0 && b.height() > 0.0).then_some(b)
    };
    doc.get_pages()
        .into_values()
        .map(|page| {
            let media = rect(inherited(page, b"MediaBox")).unwrap_or(LETTER);
            let crop = rect(inherited(page, b"CropBox"))
                .map(|c| PdfBox {
                    left: c.left.max(media.left),
                    bottom: c.bottom.max(media.bottom),
                    right: c.right.min(media.right),
                    top: c.top.min(media.top),
                })
                .filter(|c| c.width() > 0.0 && c.height() > 0.0)
                .unwrap_or(media);
            let rotation = inherited(page, b"Rotate")
                .and_then(|r| r.as_i64().ok())
                .and_then(|r| Rotation::from_degrees(((r / 90) * 90) as i32))
                .unwrap_or_default();
            PageTransform::new(crop, rotation)
        })
        .collect()
}

#[cfg(test)]
mod geometry_tests {
    use ferrix_types::{PdfPoint, Point};
    use lopdf::dictionary;

    use super::*;

    #[test]
    fn transforms_resolve_inherited_boxes_and_rotation() {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "CropBox" => vec![50.into(), 100.into(), 550.into(), 900.into()],
        });
        doc.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Rotate" => 90,
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        let t = page_transforms(&doc);
        assert_eq!(t.len(), 1);
        // The crop box is clipped to the media box (top 900 → 792) and the rotation inherited.
        assert_eq!(t[0].page_size(), ferrix_types::Size::new(692.0, 500.0));
        assert_eq!(t[0].to_page(50.0, 100.0), Point::new(0.0, 0.0));
        assert_eq!(
            t[0].to_pdf(Point::new(0.0, 0.0)),
            PdfPoint::new(50.0, 100.0)
        );
    }
}
