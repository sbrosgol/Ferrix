//! Reading things back out of a PDF: embedded files and a digest of page content (ADR-0008 §7).

use lopdf::{Dictionary, Document, Object};
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
