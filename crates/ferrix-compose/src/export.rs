//! PDF export and reading the source back (ADR-0008 §5, §7).

use std::collections::HashMap;

use ferrix_pdf::Source;
use krilla::SerializeSettings;
use krilla::color::rgb;
use krilla::configure::{Archival, ConfigurationBuilder};
use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::metadata::{DateTime, Metadata};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule};
use krilla::surface::Surface;
use krilla::tagging::{
    Artifact, ArtifactType, ContentTag, Identifier, ListNumbering, SpanTag, Tag, TagGroup, TagTree,
};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use serde::{Deserialize, Serialize};

use crate::fonts::{EmbedFonts, Fonts};
use crate::layout::{Layout, Line};
use crate::model::{Color, Document, ListKind, ModelError, ParagraphStyle};
use std::num::NonZeroU16;

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

/// The first page of `layout` as a bare PDF: no tags, metadata, embedded source or second pass.
/// For annotation appearances (ADR-0009 phase 3), whose page is copied into another file as a
/// form XObject; marked-content ids would clash with the host page's structure there.
///
/// With `icc`, colours are written as ICC-based sRGB instead of device colour spaces, which
/// PDF/A allows whatever the file's output intent (ISO 19005-2 §6.2.4.3).
pub fn export_plain_page(
    doc: &Document,
    layout: &Layout,
    fonts: &Fonts,
    icc: bool,
) -> Result<Vec<u8>, ExportError> {
    let fonts = fonts.for_embedding(layout);
    let mut pdf = krilla::Document::new_with(SerializeSettings {
        enable_tagging: false,
        no_device_cs: icc,
        ..SerializeSettings::default()
    });
    let texts: Vec<String> = doc
        .paragraphs
        .iter()
        .map(|p| p.text().replace(['\n', '\r'], " "))
        .collect();
    let mut cache: HashMap<cosmic_text::fontdb::ID, Font> = HashMap::new();
    let settings = PageSettings::from_wh(layout.page.width, layout.page.height)
        .ok_or_else(|| ExportError::Write("invalid page size".into()))?;
    let mut page_out = pdf.start_page_with(settings);
    let mut surface = page_out.surface();
    if let Some(page) = layout.pages.first() {
        for line in &page.lines {
            let text = texts.get(line.block).map_or("", String::as_str);
            draw_line(&mut surface, line, text, &fonts, &mut cache, None)?;
        }
    }
    surface.finish();
    page_out.finish();
    pdf.finish()
        .map_err(|e| ExportError::Write(format!("{e:?}")))
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
    let date = doc.archival.then(now);
    let plain = write(doc, layout, fonts, None, date)?;
    let digest = ferrix_pdf::content_digest(Source::new(&plain))
        .map_err(|e| ExportError::Verify(e.to_string()))?;

    let envelope = Envelope {
        generator: format!("Ferrix {}", env!("CARGO_PKG_VERSION")),
        content_digest: digest.digest.clone(),
        document: serde_json::to_value(doc).map_err(|e| ExportError::Write(e.to_string()))?,
    };
    let source = serde_json::to_vec(&envelope).map_err(|e| ExportError::Write(e.to_string()))?;
    let bytes = write(doc, layout, fonts, Some(source), date)?;

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
    date: Option<DateTime>,
) -> Result<Vec<u8>, ExportError> {
    let mut settings = SerializeSettings {
        enable_tagging: true,
        ..SerializeSettings::default()
    };
    if doc.archival {
        settings.configuration = ConfigurationBuilder::new()
            .with_archival_validator(Archival::A3_B)
            .finish()
            .map_err(|e| ExportError::Write(format!("{e:?}")))?;
    }
    let mut pdf = krilla::Document::new_with(settings);
    let mut metadata = Metadata::new()
        .creator(format!("Ferrix {}", env!("CARGO_PKG_VERSION")))
        .producer("Ferrix".to_owned());
    if let Some(title) = doc.title.as_ref().filter(|t| !t.trim().is_empty()) {
        metadata = metadata.title(title.clone());
    }
    // Archival files must be dated. Other exports are not, so they stay byte-for-byte
    // reproducible.
    if let Some(now) = date {
        metadata = metadata.creation_date(now);
    }
    pdf.set_metadata(metadata);
    let mut tags = Tags::new(doc.paragraphs.len());

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
        if !page.rules.is_empty() {
            // Table borders are drawing, not content.
            surface.start_tagged(ContentTag::Artifact(Artifact::new(
                ArtifactType::Layout,
                None,
            )));
            for rule in &page.rules {
                let Some(rect) = Rect::from_xywh(rule.x, rule.top, rule.width, rule.thickness)
                else {
                    continue;
                };
                let mut path = PathBuilder::new();
                path.push_rect(rect);
                if let Some(path) = path.finish() {
                    surface.set_fill(Some(fill(rule.color)));
                    surface.draw_path(&path);
                }
            }
            surface.end_tagged();
        }
        for line in &page.lines {
            let text = texts.get(line.block).map_or("", String::as_str);
            draw_line(&mut surface, line, text, fonts, &mut cache, Some(&mut tags))?;
        }
        // Headers and footers are page decoration, not content, for screen readers. They carry
        // their own text: their glyph ranges index into it.
        for line in &page.furniture {
            let text = page_text(line);
            let kind = if line.top > layout.page.height / 2.0 {
                ArtifactType::Footer
            } else {
                ArtifactType::Header
            };
            surface.start_tagged(ContentTag::Artifact(Artifact::new(kind, None)));
            draw_line(&mut surface, line, &text, fonts, &mut cache, None)?;
            surface.end_tagged();
        }
        surface.finish();
        page_out.finish();
    }

    pdf.set_tag_tree(tags.tree(doc));

    if let Some(data) = source {
        let file = EmbeddedFile {
            path: SOURCE_NAME.to_owned(),
            mime_type: MimeType::new("application/json"),
            description: Some("Ferrix document source, used to keep editing this PDF".to_owned()),
            association_kind: AssociationKind::Source,
            data: data.into(),
            modification_date: date,
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
    mut tags: Option<&mut Tags>,
) -> Result<(), ExportError> {
    let block = line.block;
    if let Some(image) = &line.image {
        let id = tags
            .as_ref()
            .map(|_| surface.start_tagged(ContentTag::Other));
        draw_image(surface, image)?;
        if let (Some(id), Some(tags)) = (id, tags.as_deref_mut()) {
            surface.end_tagged();
            tags.figures[block].push(id);
        }
    }
    if let Some((marker, glyphs)) = &line.marker {
        let id = tags
            .as_ref()
            .map(|_| surface.start_tagged(ContentTag::Span(SpanTag::empty())));
        draw_glyph_runs(surface, glyphs, marker, fonts, cache)?;
        if let (Some(id), Some(tags)) = (id, tags.as_deref_mut()) {
            surface.end_tagged();
            tags.markers[block].push(id);
        }
    }
    if !line.glyphs.is_empty() {
        let id = tags
            .as_ref()
            .map(|_| surface.start_tagged(ContentTag::Span(SpanTag::empty())));
        draw_glyph_runs(surface, &line.glyphs, text, fonts, cache)?;
        if let (Some(id), Some(tags)) = (id, tags.as_deref_mut()) {
            surface.end_tagged();
            tags.text[block].push(id);
        }
    }
    if !line.decorations.is_empty() {
        // Underlines and strike-through bars are drawing, not text.
        let tagged = tags.is_some();
        if tagged {
            surface.start_tagged(ContentTag::Artifact(Artifact::new(
                ArtifactType::Layout,
                None,
            )));
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
        if tagged {
            surface.end_tagged();
        }
    }
    Ok(())
}

/// Marked content of each paragraph, gathered while drawing, for the structure tree.
struct Tags {
    text: Vec<Vec<Identifier>>,
    markers: Vec<Vec<Identifier>>,
    figures: Vec<Vec<Identifier>>,
}

impl Tags {
    fn new(paragraphs: usize) -> Self {
        Self {
            text: vec![Vec::new(); paragraphs],
            markers: vec![Vec::new(); paragraphs],
            figures: vec![Vec::new(); paragraphs],
        }
    }

    /// The document's logical structure in reading order: headings, paragraphs, figures (with
    /// their caption as alternative text) and lists of items with label and body.
    fn tree(self, doc: &Document) -> TagTree {
        let mut tree = TagTree::new();
        let mut list: Option<(ListKind, TagGroup)> = None;
        let mut table: Option<TableTags> = None;
        let Self {
            text,
            markers,
            figures,
        } = self;
        for (((paragraph, text), markers), figures) in
            doc.paragraphs.iter().zip(text).zip(markers).zip(figures)
        {
            let content = (!text.is_empty()).then(|| {
                let mut group = match paragraph.style {
                    ParagraphStyle::Normal => TagGroup::new(Tag::P),
                    style => {
                        let level = match style {
                            ParagraphStyle::Heading2 => 2,
                            ParagraphStyle::Heading3 => 3,
                            _ => 1,
                        };
                        TagGroup::new(Tag::Hn(
                            NonZeroU16::new(level).unwrap_or(NonZeroU16::MIN),
                            None,
                        ))
                    }
                };
                for id in text {
                    group.push(id);
                }
                group
            });
            let figure = (!figures.is_empty()).then(|| {
                let caption = paragraph.text();
                let alt = if caption.trim().is_empty() {
                    "Picture".to_owned()
                } else {
                    caption
                };
                let mut group = TagGroup::new(Tag::Figure(Some(alt)));
                for id in figures {
                    group.push(id);
                }
                group
            });

            // Table cells: Table > TR > TD, holding the cell's paragraphs.
            if let Some(cell) = paragraph.cell {
                if let Some((_, group)) = list.take() {
                    tree.push(group);
                }
                if table.as_ref().is_some_and(|t| t.id != cell.table)
                    && let Some(done) = table.take()
                {
                    tree.push(done.finish());
                }
                let t = table.get_or_insert_with(|| TableTags::new(cell.table));
                t.cell(cell.row, cell.col);
                if let Some(figure) = figure {
                    t.push(figure);
                }
                if let Some(content) = content {
                    t.push(content);
                }
                continue;
            }
            if let Some(done) = table.take() {
                tree.push(done.finish());
            }
            match paragraph.format.list {
                Some(item) => {
                    if list.as_ref().is_some_and(|(kind, _)| *kind != item.kind)
                        && let Some((_, group)) = list.take()
                    {
                        tree.push(group);
                    }
                    let (_, group) = list.get_or_insert_with(|| {
                        let numbering = match item.kind {
                            ListKind::Bullet => ListNumbering::Disc,
                            ListKind::Numbered => ListNumbering::Decimal,
                        };
                        (item.kind, TagGroup::new(Tag::L(numbering)))
                    });
                    let mut li = TagGroup::new(Tag::LI);
                    if !markers.is_empty() {
                        let mut label = TagGroup::new(Tag::Lbl);
                        for id in markers {
                            label.push(id);
                        }
                        li.push(label);
                    }
                    let mut body = TagGroup::new(Tag::LBody);
                    if let Some(figure) = figure {
                        body.push(figure);
                    }
                    if let Some(content) = content {
                        body.push(content);
                    }
                    li.push(body);
                    group.push(li);
                }
                None => {
                    if let Some((_, group)) = list.take() {
                        tree.push(group);
                    }
                    if let Some(figure) = figure {
                        tree.push(figure);
                    }
                    if let Some(content) = content {
                        tree.push(content);
                    }
                }
            }
        }
        if let Some((_, group)) = list {
            tree.push(group);
        }
        if let Some(done) = table {
            tree.push(done.finish());
        }
        tree
    }
}

/// A table's structure elements while it is being built.
struct TableTags {
    id: u32,
    table: TagGroup,
    row: Option<(u32, TagGroup)>,
    cell: Option<((u32, u32), TagGroup)>,
}

impl TableTags {
    fn new(id: u32) -> Self {
        Self {
            id,
            table: TagGroup::new(Tag::Table),
            row: None,
            cell: None,
        }
    }

    /// Moves to cell (`row`, `col`), closing the previous cell and row as needed.
    fn cell(&mut self, row: u32, col: u32) {
        if self.cell.as_ref().is_some_and(|(at, _)| *at == (row, col)) {
            return;
        }
        self.close_cell();
        if self.row.as_ref().is_some_and(|(r, _)| *r != row) {
            self.close_row();
        }
        if self.row.is_none() {
            self.row = Some((row, TagGroup::new(Tag::TR)));
        }
        self.cell = Some(((row, col), TagGroup::new(Tag::TD)));
    }

    fn push(&mut self, child: TagGroup) {
        if let Some((_, cell)) = &mut self.cell {
            cell.push(child);
        }
    }

    fn close_cell(&mut self) {
        if let (Some((_, cell)), Some((_, row))) = (self.cell.take(), self.row.as_mut()) {
            row.push(cell);
        }
    }

    fn close_row(&mut self) {
        self.close_cell();
        if let Some((_, row)) = self.row.take() {
            self.table.push(row);
        }
    }

    fn finish(mut self) -> TagGroup {
        self.close_row();
        self.table
    }
}

/// The current time in UTC, for archival metadata.
fn now() -> DateTime {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
    DateTime::new(year)
        .month(month)
        .day(day)
        .hour((rest / 3600) as u8)
        .minute((rest % 3600 / 60) as u8)
        .second((rest % 60) as u8)
        .utc_offset_hour(0)
        .utc_offset_minute(0)
}

fn draw_image(
    surface: &mut Surface<'_>,
    image: &crate::layout::PlacedImage,
) -> Result<(), ExportError> {
    let data = krilla::Data::from(image.data.bytes.clone());
    let picture = match image.data.kind {
        crate::model::ImageKind::Png => krilla::image::Image::from_png(data, false),
        crate::model::ImageKind::Jpeg => krilla::image::Image::from_jpeg(data, false),
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
