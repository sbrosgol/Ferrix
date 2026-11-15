//! The document model (ADR-0008 §2): page setup and paragraphs of formatted text.

use serde::{Deserialize, Serialize};

/// Version of the serialized model. Bump when a change is not backwards compatible.
pub const FORMAT_VERSION: u32 = 1;

/// A written document. Units are PDF points (1/72 inch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: u32,
    #[serde(default)]
    pub title: Option<String>,
    pub page: PageSetup,
    /// Base font for every style.
    pub font: FontChoice,
    /// Text at the top of every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<PageText>,
    /// Text at the bottom of every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<PageText>,
    /// Save as PDF/A-3b, for long-term archiving.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archival: bool,
    /// Never empty: a document always has at least one paragraph to type into.
    pub paragraphs: Vec<Paragraph>,
}

impl Document {
    /// An empty document with A4 pages.
    pub fn new() -> Self {
        Self::with_page(PageSetup::default())
    }

    pub fn with_page(page: PageSetup) -> Self {
        Self {
            format: FORMAT_VERSION,
            title: None,
            page,
            font: FontChoice::default(),
            header: None,
            footer: None,
            archival: false,
            paragraphs: vec![Paragraph::new(ParagraphStyle::Normal)],
        }
    }

    /// Reads a document from its JSON source.
    pub fn from_json(json: &[u8]) -> Result<Self, ModelError> {
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        let header: Header =
            serde_json::from_slice(json).map_err(|e| ModelError::Invalid(e.to_string()))?;
        if header.format > FORMAT_VERSION {
            return Err(ModelError::NewerFormat(header.format));
        }
        let mut document: Self =
            serde_json::from_slice(json).map_err(|e| ModelError::Invalid(e.to_string()))?;
        document.validate()?;
        // Edits rely on normalized runs; a file written elsewhere may not have them.
        for paragraph in &mut document.paragraphs {
            for run in std::mem::take(&mut paragraph.runs) {
                paragraph.push(&run.text, run.format);
            }
        }
        Ok(document)
    }

    pub fn to_json(&self) -> Vec<u8> {
        // Serializing plain data structures cannot fail.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Checks the invariants that edits and layout rely on.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.paragraphs.is_empty() {
            return Err(ModelError::Invalid(
                "a document needs at least one paragraph".into(),
            ));
        }
        self.page.validate()?;
        for paragraph in &self.paragraphs {
            if let Some(image) = &paragraph.image
                && !(1.0..=14_400.0).contains(&image.width)
            {
                return Err(ModelError::Invalid("an image has an invalid width".into()));
            }
            for run in &paragraph.runs {
                if let Some(size) = run.format.size
                    && !(1.0..=1000.0).contains(&size)
                {
                    return Err(ModelError::Invalid(format!(
                        "font size {size} is out of range"
                    )));
                }
            }
        }
        Ok(())
    }

    /// The end of the document.
    pub fn end(&self) -> Position {
        let block = self.paragraphs.len() - 1;
        Position::new(block, self.paragraphs[block].len())
    }

    /// All text, one line per paragraph.
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for (i, paragraph) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&paragraph.text());
        }
        text
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// A header or footer: one line of text, where `{page}` and `{pages}` become the page number
/// and the page count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageText {
    pub text: String,
    #[serde(default)]
    pub align: Align,
    /// Leave it off the first page (for a title page).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_first: bool,
}

impl PageText {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            align: Align::Center,
            skip_first: false,
        }
    }

    /// The text for page `page` (1-based) of `pages`.
    pub fn for_page(&self, page: usize, pages: usize) -> String {
        self.text
            .replace("{page}", &page.to_string())
            .replace("{pages}", &pages.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("the document is invalid: {0}")]
    Invalid(String),
    #[error("the document was written by a newer version of Ferrix (format {0})")]
    NewerFormat(u32),
}

/// Page size and margins, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub margins: Margins,
}

impl PageSetup {
    pub const A4: (f32, f32) = (595.28, 841.89);
    pub const LETTER: (f32, f32) = (612.0, 792.0);

    pub fn new((width, height): (f32, f32)) -> Self {
        Self {
            width,
            height,
            margins: Margins::uniform(72.0),
        }
    }

    pub fn landscape(self) -> Self {
        Self {
            width: self.height,
            height: self.width,
            ..self
        }
    }

    /// Width available to text.
    pub fn content_width(&self) -> f32 {
        self.width - self.margins.left - self.margins.right
    }

    /// Height available to text.
    pub fn content_height(&self) -> f32 {
        self.height - self.margins.top - self.margins.bottom
    }

    fn validate(&self) -> Result<(), ModelError> {
        let m = self.margins;
        let ok = (72.0..=14_400.0).contains(&self.width)
            && (72.0..=14_400.0).contains(&self.height)
            && [m.top, m.right, m.bottom, m.left].iter().all(|v| *v >= 0.0)
            && self.content_width() >= 36.0
            && self.content_height() >= 36.0;
        if ok {
            Ok(())
        } else {
            Err(ModelError::Invalid(
                "page size or margins are out of range".into(),
            ))
        }
    }
}

impl Default for PageSetup {
    fn default() -> Self {
        Self::new(Self::A4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Margins {
    pub fn uniform(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// A font family and size that styles build on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontChoice {
    pub family: String,
    pub size: f32,
}

impl Default for FontChoice {
    fn default() -> Self {
        Self {
            family: crate::fonts::SANS.to_owned(),
            size: 11.0,
        }
    }
}

/// A paragraph: its style, formatting and text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub style: ParagraphStyle,
    #[serde(default)]
    pub format: ParagraphFormat,
    /// Normalized: no empty runs, and neighbouring runs differ in format.
    pub runs: Vec<Run>,
    /// A picture shown above the paragraph's text (which then serves as its caption).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
}

impl Paragraph {
    pub fn new(style: ParagraphStyle) -> Self {
        Self {
            style,
            format: ParagraphFormat::default(),
            runs: Vec::new(),
            image: None,
        }
    }

    /// A paragraph with the same style and formatting but no text.
    pub fn empty_like(&self) -> Self {
        Self {
            style: self.style,
            format: self.format,
            runs: Vec::new(),
            image: self.image.clone(),
        }
    }

    /// The paragraph that continues this one after a break (Enter): same style and formatting,
    /// but no picture and no page break.
    pub fn continuation(&self) -> Self {
        let mut next = self.empty_like();
        next.image = None;
        next.format.page_break_before = false;
        next
    }

    pub fn with_text(mut self, text: &str, format: CharFormat) -> Self {
        self.push(text, format);
        self
    }

    /// Length in bytes of UTF-8 text.
    pub fn len(&self) -> usize {
        self.runs.iter().map(|r| r.text.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Whether `offset` falls on a character boundary of the text.
    pub fn is_boundary(&self, offset: usize) -> bool {
        let mut start = 0;
        for run in &self.runs {
            let end = start + run.text.len();
            if offset <= end {
                return run.text.is_char_boundary(offset - start);
            }
            start = end;
        }
        offset == 0
    }

    /// Appends text, merging it into the last run when the format is the same.
    pub fn push(&mut self, text: &str, format: CharFormat) {
        if text.is_empty() {
            return;
        }
        match self.runs.last_mut() {
            Some(last) if last.format == format => last.text.push_str(text),
            _ => self.runs.push(Run {
                text: text.to_owned(),
                format,
            }),
        }
    }

    /// The character format that text typed at `offset` takes: that of the character before it,
    /// or of the first character at the start of the paragraph.
    pub fn format_at(&self, offset: usize) -> CharFormat {
        let mut start = 0;
        for run in &self.runs {
            let end = start + run.text.len();
            if offset > start && offset <= end {
                return run.format.clone();
            }
            start = end;
        }
        self.runs
            .first()
            .map(|r| r.format.clone())
            .unwrap_or_default()
    }

    /// The text and runs in `range` (byte offsets, on character boundaries).
    pub(crate) fn slice(&self, range: std::ops::Range<usize>) -> Paragraph {
        let mut out = self.empty_like();
        let mut start = 0;
        for run in &self.runs {
            let end = start + run.text.len();
            let from = range.start.max(start);
            let to = range.end.min(end);
            if from < to {
                out.push(&run.text[from - start..to - start], run.format.clone());
            }
            start = end;
        }
        out
    }

    /// Appends another paragraph's runs (its style and format are ignored).
    pub(crate) fn append(&mut self, other: Paragraph) {
        for run in other.runs {
            self.push(&run.text, run.format);
        }
    }
}

/// Built-in paragraph styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParagraphStyle {
    Normal,
    Title,
    Heading1,
    Heading2,
    Heading3,
}

impl ParagraphStyle {
    pub const ALL: [Self; 5] = [
        Self::Normal,
        Self::Title,
        Self::Heading1,
        Self::Heading2,
        Self::Heading3,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Title => "Title",
            Self::Heading1 => "Heading 1",
            Self::Heading2 => "Heading 2",
            Self::Heading3 => "Heading 3",
        }
    }

    pub fn is_heading(self) -> bool {
        self != Self::Normal
    }

    /// The style for a paragraph started by pressing Enter at the end of this one.
    pub fn next(self) -> Self {
        Self::Normal
    }

    /// Font size relative to the document's base size, whether bold, and spacing before and after
    /// in points.
    pub(crate) fn defaults(self) -> StyleDefaults {
        let (scale, bold, before, after) = match self {
            Self::Normal => (1.0, false, 0.0, 8.0),
            Self::Title => (2.4, true, 0.0, 12.0),
            Self::Heading1 => (1.6, true, 18.0, 6.0),
            Self::Heading2 => (1.3, true, 14.0, 4.0),
            Self::Heading3 => (1.1, true, 12.0, 4.0),
        };
        StyleDefaults {
            scale,
            bold,
            space_before: before,
            space_after: after,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StyleDefaults {
    pub scale: f32,
    pub bold: bool,
    pub space_before: f32,
    pub space_after: f32,
}

/// Paragraph formatting; `None` keeps the style's value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParagraphFormat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// Multiple of the normal line height (1.0 = single).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_spacing: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_before: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_after: Option<f32>,
    /// Left indent in points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indent: Option<f32>,
    /// A bulleted or numbered list item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<List>,
    /// Starts a new page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub page_break_before: bool,
}

/// List membership of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct List {
    pub kind: ListKind,
    /// Nesting depth, 0 for the outermost level.
    #[serde(default)]
    pub level: u8,
}

impl List {
    pub const MAX_LEVEL: u8 = 8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ListKind {
    Bullet,
    Numbered,
}

/// A picture placed in a paragraph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub data: std::sync::Arc<ImageData>,
    /// Displayed width in points (shrunk to the text width if wider).
    pub width: f32,
}

/// The encoded bytes of a picture, as found in the file it came from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "SerializedImage", into = "SerializedImage")]
pub struct ImageData {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Identifies the bytes cheaply (for comparing and caching).
    pub hash: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    Png,
    Jpeg,
}

/// Pictures larger than this are refused.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

impl ImageData {
    /// Reads a PNG or JPEG file's bytes, checking that they decode.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, ModelError> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(ModelError::Invalid("the picture is too large".into()));
        }
        let kind = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Png) => ImageKind::Png,
            Ok(image::ImageFormat::Jpeg) => ImageKind::Jpeg,
            _ => {
                return Err(ModelError::Invalid(
                    "only PNG and JPEG pictures are supported".into(),
                ));
            }
        };
        let (pixel_width, pixel_height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| ModelError::Invalid(e.to_string()))?
            .into_dimensions()
            .map_err(|e| ModelError::Invalid(format!("the picture can't be read: {e}")))?;
        if pixel_width == 0 || pixel_height == 0 || pixel_width > 30_000 || pixel_height > 30_000 {
            return Err(ModelError::Invalid(
                "the picture has an unusable size".into(),
            ));
        }
        let hash = hash_bytes(&bytes);
        Ok(Self {
            kind,
            bytes,
            pixel_width,
            pixel_height,
            hash,
        })
    }

    /// Height for a given displayed width, keeping the aspect ratio.
    pub fn height_for(&self, width: f32) -> f32 {
        width * self.pixel_height as f32 / self.pixel_width as f32
    }

    /// Natural width in points, at 96 pixels per inch.
    pub fn natural_width(&self) -> f32 {
        self.pixel_width as f32 * 0.75
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl PartialEq for ImageData {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.bytes == other.bytes
    }
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageData")
            .field("kind", &self.kind)
            .field("size", &(self.pixel_width, self.pixel_height))
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// How a picture is stored in the JSON source: base64 bytes.
#[derive(Serialize, Deserialize)]
struct SerializedImage {
    kind: ImageKind,
    data: String,
}

impl From<ImageData> for SerializedImage {
    fn from(image: ImageData) -> Self {
        Self {
            kind: image.kind,
            data: base64::encode(&image.bytes),
        }
    }
}

impl TryFrom<SerializedImage> for ImageData {
    type Error = String;

    fn try_from(image: SerializedImage) -> Result<Self, String> {
        let bytes = base64::decode(&image.data).ok_or("invalid picture data")?;
        let data = Self::from_bytes(bytes).map_err(|e| e.to_string())?;
        if data.kind != image.kind {
            return Err("picture data does not match its kind".into());
        }
        Ok(data)
    }
}

pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    base64::encode(bytes)
}

/// Standard base64 (RFC 4648) with padding.
mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(super) fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
        let text = text.trim_end_matches('=');
        let mut out = Vec::with_capacity(text.len() * 3 / 4);
        let (mut acc, mut bits) = (0u32, 0u32);
        for c in text.bytes() {
            let value = ALPHABET.iter().position(|&a| a == c)? as u32;
            acc = (acc << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn round_trips() {
            for len in 0..20 {
                let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
                let text = super::encode(&bytes);
                assert_eq!(text.len() % 4, 0);
                assert_eq!(super::decode(&text), Some(bytes));
            }
            assert_eq!(super::encode(b"Man"), "TWFu");
            assert_eq!(super::encode(b"Ma"), "TWE=");
            assert_eq!(super::decode("not base64!"), None);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// A stretch of text with one character format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    #[serde(default)]
    pub format: CharFormat,
}

/// Character formatting; `None` keeps the paragraph style's value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharFormat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strikethrough: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// An sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color(pub u8, pub u8, pub u8);

impl Color {
    pub const BLACK: Self = Self(0, 0, 0);
}

/// A place in the text: a paragraph and a byte offset within its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub block: usize,
    pub offset: usize,
}

impl Position {
    pub const START: Self = Self {
        block: 0,
        offset: 0,
    };

    pub fn new(block: usize, offset: usize) -> Self {
        Self { block, offset }
    }
}

/// A span of text from `start` (inclusive) to `end` (exclusive). `start <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

impl TextRange {
    /// The range between two positions, in either order.
    pub fn between(a: Position, b: Position) -> Self {
        Self {
            start: a.min(b),
            end: a.max(b),
        }
    }

    pub fn caret(at: Position) -> Self {
        Self { start: at, end: at }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}
