//! PDF export and reading the source back (ADR-0008 §5, §7).

use std::collections::HashMap;

use ferrix_pdf::Source;
use krilla::color::rgb;
use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use serde::{Deserialize, Serialize};

use crate::fonts::{EmbedFonts, Fonts};
use crate::layout::{Layout, Line};
use crate::model::{Color, Document, ModelError};

/// Name of the embedded source file.
pub const SOURCE_NAME: &str = "ferrix-document.json";

/// What is embedded in the PDF: the document plus a digest of the pages it produced.
#[derive(Serialize, Deserialize)]
struct Envelope {
    generator: String,
    content_digest: String,
    document: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExportError {
    #[error("could not write the PDF: {0}")]
    Write(String),
    #[error("the font for some text could not be embedded")]
    MissingFont,
    #[error("the PDF could not be verified: {0}")]
    Verify(String),
}

/// Writes the document as a PDF with its source embedded.
///
/// The pages are written twice: first to fingerprint their content, then with the source and that
/// fingerprint embedded. The second pass is checked to produce the same pages.
pub fn export_pdf(doc: &Document, layout: &Layout, fonts: &Fonts) -> Result<Vec<u8>, ExportError> {
    export_with(doc, layout, &fonts.for_embedding(layout))
}

/// Everything needed to write a PDF, detached from the editor so it can run on another thread.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub document: Document,
    pub layout: Layout,
    pub fonts: EmbedFonts,
}

impl ExportJob {
    pub fn new(doc: &Document, layout: &Layout, fonts: &Fonts) -> Self {
        Self {
            document: doc.clone(),
            layout: layout.clone(),
            fonts: fonts.for_embedding(layout),
        }
    }

    pub fn run(&self) -> Result<Vec<u8>, ExportError> {
        export_with(&self.document, &self.layout, &self.fonts)
    }
}

fn export_with(
    doc: &Document,
    layout: &Layout,
    fonts: &EmbedFonts,
) -> Result<Vec<u8>, ExportError> {
    let plain = write(doc, layout, fonts, None)?;
    let digest = ferrix_pdf::content_digest(Source::new(&plain))
        .map_err(|e| ExportError::Verify(e.to_string()))?;

    let envelope = Envelope {
        generator: format!("Ferrix {}", env!("CARGO_PKG_VERSION")),
        content_digest: digest.digest.clone(),
        document: serde_json::to_value(doc).map_err(|e| ExportError::Write(e.to_string()))?,
    };
    let source = serde_json::to_vec(&envelope).map_err(|e| ExportError::Write(e.to_string()))?;
    let bytes = write(doc, layout, fonts, Some(source))?;

    let check = ferrix_pdf::content_digest(Source::new(&bytes))
        .map_err(|e| ExportError::Verify(e.to_string()))?;
    if check.digest != digest.digest || check.pages != layout.pages.len() {
        return Err(ExportError::Verify(
            "page content changed between passes".into(),
        ));
    }
    Ok(bytes)
}

fn write(
    doc: &Document,
    layout: &Layout,
    fonts: &EmbedFonts,
    source: Option<Vec<u8>>,
) -> Result<Vec<u8>, ExportError> {
    let mut pdf = krilla::Document::new();
    let mut metadata = Metadata::new()
        .creator(format!("Ferrix {}", env!("CARGO_PKG_VERSION")))
        .producer("Ferrix".to_owned());
    if let Some(title) = doc.title.as_ref().filter(|t| !t.trim().is_empty()) {
        metadata = metadata.title(title.clone());
    }
    pdf.set_metadata(metadata);

    let texts: Vec<String> = doc
        .paragraphs
        .iter()
        .map(|p| p.text().replace(['\n', '\r'], " "))
        .collect();
    let mut cache: HashMap<cosmic_text::fontdb::ID, Font> = HashMap::new();

    let settings = PageSettings::from_wh(layout.page.width, layout.page.height)
        .ok_or_else(|| ExportError::Write("invalid page size".into()))?;
    for page in &layout.pages {
        let mut page_out = pdf.start_page_with(settings.clone());
        let mut surface = page_out.surface();
        for line in &page.lines {
            let text = texts.get(line.block).map_or("", String::as_str);
            draw_line(&mut surface, line, text, fonts, &mut cache)?;
        }
        // Headers and footers carry their own text: their glyph ranges index into it.
        for line in &page.furniture {
            let text = page_text(line);
            draw_line(&mut surface, line, &text, fonts, &mut cache)?;
        }
        surface.finish();
        page_out.finish();
    }

    if let Some(data) = source {
        let file = EmbeddedFile {
            path: SOURCE_NAME.to_owned(),
            mime_type: MimeType::new("application/json"),
            description: Some("Ferrix document source, used to keep editing this PDF".to_owned()),
            association_kind: AssociationKind::Source,
            data: data.into(),
            modification_date: None,
            compress: Some(true),
            location: None,
        };
        pdf.embed_file(file)
            .ok_or_else(|| ExportError::Write("the source was embedded twice".into()))?;
    }

    pdf.finish()
        .map_err(|e| ExportError::Write(format!("{e:?}")))
}

fn fill(color: Color) -> Fill {
    Fill {
        paint: rgb::Color::new(color.0, color.1, color.2).into(),
        opacity: NormalizedF32::ONE,
        rule: FillRule::default(),
    }
}

/// The text of a header or footer line, rebuilt from the layout's paragraph.
fn page_text(line: &Line) -> String {
    line.source_text.clone().unwrap_or_default()
}

fn draw_line(
    surface: &mut Surface<'_>,
    line: &Line,
    text: &str,
    fonts: &EmbedFonts,
    cache: &mut HashMap<cosmic_text::fontdb::ID, Font>,
) -> Result<(), ExportError> {
    if let Some(image) = &line.image {
        draw_image(surface, image)?;
    }
    draw_glyph_runs(surface, &line.glyphs, text, fonts, cache)?;
    if let Some((marker, glyphs)) = &line.marker {
        draw_glyph_runs(surface, glyphs, marker, fonts, cache)?;
    }
    for bar in &line.decorations {
        let Some(rect) = Rect::from_xywh(bar.x, bar.top, bar.width, bar.thickness) else {
            continue;
        };
        let mut path = PathBuilder::new();
        path.push_rect(rect);
        if let Some(path) = path.finish() {
            surface.set_fill(Some(fill(bar.color)));
            surface.draw_path(&path);
        }
    }
    Ok(())
}

fn draw_image(
    surface: &mut Surface<'_>,
    image: &crate::layout::PlacedImage,
) -> Result<(), ExportError> {
    let data = krilla::Data::from(image.data.bytes.clone());
    let picture = match image.data.kind {
        crate::model::ImageKind::Png => krilla::image::Image::from_png(data, true),
        crate::model::ImageKind::Jpeg => krilla::image::Image::from_jpeg(data, true),
    }
    .map_err(|e| ExportError::Write(format!("a picture could not be embedded: {e}")))?;
    let Some(size) = krilla::geom::Size::from_wh(image.width, image.height) else {
        return Ok(());
    };
    surface.push_transform(&krilla::geom::Transform::from_translate(image.x, image.top));
    surface.draw_image(picture, size);
    surface.pop();
    Ok(())
}

fn draw_glyph_runs(
    surface: &mut Surface<'_>,
    glyphs: &[crate::layout::PlacedGlyph],
    text: &str,
    fonts: &EmbedFonts,
    cache: &mut HashMap<cosmic_text::fontdb::ID, Font>,
) -> Result<(), ExportError> {
    let mut start = 0;
    while start < glyphs.len() {
        // A run of glyphs drawn in one go: same font, size, colour and baseline.
        let first = &glyphs[start];
        let mut end = start + 1;
        while end < glyphs.len() {
            let g = &glyphs[end];
            if g.font != first.font
                || g.size != first.size
                || g.color != first.color
                || g.y != first.y
            {
                break;
            }
            end += 1;
        }
        let group = &glyphs[start..end];

        let font = match cache.get(&first.font) {
            Some(font) => font.clone(),
            None => {
                let (data, index) = fonts
                    .get(first.font)
                    .cloned()
                    .ok_or(ExportError::MissingFont)?;
                let font = Font::new(data, index).ok_or(ExportError::MissingFont)?;
                cache.insert(first.font, font.clone());
                font
            }
        };

        // The text the glyphs show, so the PDF can be searched and copied from.
        let lo = group.iter().map(|g| g.range.start).min().unwrap_or(0);
        let hi = group.iter().map(|g| g.range.end).max().unwrap_or(0);
        let (lo, hi) = if lo <= hi
            && hi <= text.len()
            && text.is_char_boundary(lo)
            && text.is_char_boundary(hi)
        {
            (lo, hi)
        } else {
            (0, 0)
        };
        let size = first.size;
        let placed: Vec<KrillaGlyph> = group
            .iter()
            .enumerate()
            .map(|(i, g)| {
                // Advance to the next glyph's actual position, which includes justification.
                let advance = group.get(i + 1).map_or(g.advance, |next| next.x - g.x);
                let range = if hi > lo {
                    g.range.start.saturating_sub(lo).min(hi - lo)
                        ..g.range.end.saturating_sub(lo).min(hi - lo)
                } else {
                    0..0
                };
                KrillaGlyph::new(
                    GlyphId::new(u32::from(g.glyph_id)),
                    advance / size,
                    0.0,
                    0.0,
                    0.0,
                    range,
                    None,
                )
            })
            .collect();
        surface.set_fill(Some(fill(first.color)));
        surface.draw_glyphs(
            Point::from_xy(first.x, first.y),
            &placed,
            font,
            &text[lo..hi],
            size,
            false,
        );
        start = end;
    }
    Ok(())
}

/// What a PDF holds, as far as writing is concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    /// Written by Ferrix and unchanged since: open it for writing.
    Editable(Document),
    /// Written by Ferrix, but another application changed it since. Editing the source would
    /// throw those changes away, so it should open as an ordinary PDF.
    Changed(Document),
    /// No usable source: an ordinary PDF.
    Plain,
}

/// Looks for an embedded source and checks it still matches the pages.
pub fn open_pdf(bytes: &[u8]) -> Result<Opened, ModelError> {
    let source = Source::new(bytes);
    let Ok(Some(json)) = ferrix_pdf::embedded_file(source, SOURCE_NAME) else {
        return Ok(Opened::Plain);
    };
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&json) else {
        return Ok(Opened::Plain);
    };
    let document_json =
        serde_json::to_vec(&envelope.document).map_err(|e| ModelError::Invalid(e.to_string()))?;
    let document = Document::from_json(&document_json)?;
    let unchanged = ferrix_pdf::content_digest(source)
        .is_ok_and(|d| d.digest == envelope.content_digest && !d.has_annotations);
    Ok(if unchanged {
        Opened::Editable(document)
    } else {
        Opened::Changed(document)
    })
}
