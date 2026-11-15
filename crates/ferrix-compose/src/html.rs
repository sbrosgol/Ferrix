//! Formatted text on the clipboard (ADR-0008 phase 4): fragments to HTML for other apps, and
//! HTML from other apps (word processors, browsers, mail) to fragments.
//!
//! The reader is deliberately tolerant and small: it understands the structure and character
//! formatting that word processors put on the clipboard and ignores everything else.

use crate::edit::Fragment;
use crate::model::{CharFormat, Color, List, ListKind, Paragraph, ParagraphStyle};

/// Clipboard HTML larger than this is pasted as plain text instead.
const MAX_HTML: usize = 8 * 1024 * 1024;

// ── Writing ───────────────────────────────────────────────────────────────────

/// HTML for a fragment, for other applications to paste.
pub fn fragment_to_html(fragment: &Fragment) -> String {
    let mut out = String::from("<meta charset=\"utf-8\">");
    let mut open_list: Option<ListKind> = None;
    for paragraph in fragment.paragraphs() {
        let list = paragraph.format.list.map(|l| l.kind);
        if open_list != list {
            if let Some(kind) = open_list {
                out.push_str(list_tag(kind, true));
            }
            if let Some(kind) = list {
                out.push_str(list_tag(kind, false));
            }
            open_list = list;
        }
        if let Some(image) = &paragraph.image {
            let mime = match image.data.kind {
                crate::model::ImageKind::Png => "image/png",
                crate::model::ImageKind::Jpeg => "image/jpeg",
            };
            out.push_str(&format!(
                "<p><img width=\"{}\" src=\"data:{mime};base64,{}\"></p>",
                (image.width / 0.75).round(),
                crate::model::encode_base64(&image.data.bytes)
            ));
        }
        let tag = match (list, paragraph.style) {
            (Some(_), _) => "li",
            (None, ParagraphStyle::Normal) => "p",
            (None, ParagraphStyle::Title | ParagraphStyle::Heading1) => "h1",
            (None, ParagraphStyle::Heading2) => "h2",
            (None, ParagraphStyle::Heading3) => "h3",
        };
        let align = match paragraph.format.align {
            Some(crate::model::Align::Center) => " style=\"text-align:center\"",
            Some(crate::model::Align::Right) => " style=\"text-align:right\"",
            Some(crate::model::Align::Justify) => " style=\"text-align:justify\"",
            _ => "",
        };
        out.push_str(&format!("<{tag}{align}>"));
        for run in &paragraph.runs {
            let f = &run.format;
            let mut style = Vec::new();
            if let Some(family) = &f.family {
                style.push(format!("font-family:'{}'", escape(family)));
            }
            if let Some(size) = f.size {
                style.push(format!("font-size:{size}pt"));
            }
            if let Some(Color(r, g, b)) = f.color {
                style.push(format!("color:#{r:02x}{g:02x}{b:02x}"));
            }
            if let Some(bold) = f.bold {
                style.push(format!(
                    "font-weight:{}",
                    if bold { "bold" } else { "normal" }
                ));
            }
            if f.italic == Some(true) {
                style.push("font-style:italic".into());
            }
            let decorations: Vec<&str> = [
                (f.underline, "underline"),
                (f.strikethrough, "line-through"),
            ]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
            if !decorations.is_empty() {
                style.push(format!("text-decoration:{}", decorations.join(" ")));
            }
            let text = escape(&run.text);
            if style.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&format!(
                    "<span style=\"{}\">{text}</span>",
                    style.join(";")
                ));
            }
        }
        out.push_str(&format!("</{tag}>"));
    }
    if let Some(kind) = open_list {
        out.push_str(list_tag(kind, true));
    }
    out
}

fn list_tag(kind: ListKind, close: bool) -> &'static str {
    match (kind, close) {
        (ListKind::Bullet, false) => "<ul>",
        (ListKind::Bullet, true) => "</ul>",
        (ListKind::Numbered, false) => "<ol>",
        (ListKind::Numbered, true) => "</ol>",
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

// ── Reading ───────────────────────────────────────────────────────────────────

/// Paragraphs and formatting from clipboard HTML; `None` if it holds no text.
pub fn fragment_from_html(html: &str) -> Option<Fragment> {
    if html.len() > MAX_HTML {
        return None;
    }
    let mut reader = Reader::default();
    for token in tokenize(html) {
        reader.token(token);
    }
    reader.finish_paragraph();
    let paragraphs: Vec<Paragraph> = reader.done;
    if paragraphs.iter().all(Paragraph::is_empty) {
        return None;
    }
    Fragment::new(paragraphs)
}

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Open {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Close(String),
    Text(&'a str),
}

/// Splits HTML into tags and text. Comments, doctypes and the contents of `<script>`,
/// `<style>`, `<head>` and `<title>` are dropped.
fn tokenize(html: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    let mut skip_until: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |p| i + p);
            if skip_until.is_none() {
                tokens.push(Token::Text(&html[i..end]));
            }
            i = end;
            continue;
        }
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(html.len(), |p| i + p + 3);
            continue;
        }
        let Some(end) = html[i..].find('>').map(|p| i + p) else {
            break;
        };
        let inner = html[i + 1..end].trim();
        i = end + 1;
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        let (close, inner) = match inner.strip_prefix('/') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, inner),
        };
        let name_end = inner
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if let Some(waiting) = &skip_until {
            if close && *waiting == name {
                skip_until = None;
            }
            continue;
        }
        if close {
            tokens.push(Token::Close(name));
            continue;
        }
        if matches!(name.as_str(), "script" | "style" | "head" | "title") && !inner.ends_with('/') {
            skip_until = Some(name);
            continue;
        }
        tokens.push(Token::Open {
            attrs: attributes(&inner[name_end..]),
            name,
        });
    }
    tokens
}

fn attributes(text: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '/') {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '/' {
            i += 1;
        }
        let name: String = chars[start..i]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < chars.len() && chars[i] == '=' {
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
                let quote = chars[i];
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                value = chars[start..i].iter().collect();
                i += 1;
            } else {
                let start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                value = chars[start..i].iter().collect();
            }
        }
        if !name.is_empty() {
            attrs.push((name, decode_entities(&value)));
        }
    }
    attrs
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "bull" => Some('•'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Builds paragraphs from tokens.
#[derive(Default)]
struct Reader {
    done: Vec<Paragraph>,
    current: Option<Paragraph>,
    /// Character formatting of open inline elements, innermost last, with their tag name.
    formats: Vec<(String, CharFormat)>,
    /// Open `<ul>` / `<ol>`, innermost last.
    lists: Vec<ListKind>,
    /// Whether the last character added was a (collapsed) space.
    space: bool,
}

impl Reader {
    fn format(&self) -> CharFormat {
        self.formats
            .last()
            .map(|(_, f)| f.clone())
            .unwrap_or_default()
    }

    fn finish_paragraph(&mut self) {
        if let Some(mut paragraph) = self.current.take() {
            // Drop trailing whitespace.
            while let Some(last) = paragraph.runs.last_mut() {
                let trimmed = last.text.trim_end().len();
                if trimmed == 0 {
                    paragraph.runs.pop();
                } else {
                    last.text.truncate(trimmed);
                    break;
                }
            }
            self.done.push(paragraph);
        }
        self.space = false;
    }

    fn start_paragraph(&mut self, style: ParagraphStyle, list: bool) {
        self.finish_paragraph();
        let mut paragraph = Paragraph::new(style);
        if list && let Some(kind) = self.lists.last() {
            paragraph.format.list = Some(List {
                kind: *kind,
                level: (self.lists.len() - 1).min(usize::from(List::MAX_LEVEL)) as u8,
            });
        }
        self.current = Some(paragraph);
    }

    fn text(&mut self, raw: &str) {
        let decoded = decode_entities(raw);
        let mut text = String::new();
        for c in decoded.chars() {
            if c.is_whitespace() && c != '\u{a0}' && c != '\t' {
                // Collapse runs of whitespace into one space, none at a paragraph's start.
                let at_start =
                    self.current.as_ref().is_none_or(Paragraph::is_empty) && text.is_empty();
                if !self.space && !at_start {
                    text.push(' ');
                }
                self.space = true;
            } else {
                text.push(if c == '\u{a0}' { ' ' } else { c });
                self.space = false;
            }
        }
        if text.is_empty() {
            return;
        }
        let format = self.format();
        self.current
            .get_or_insert_with(|| Paragraph::new(ParagraphStyle::Normal))
            .push(&text, format);
    }

    fn token(&mut self, token: Token<'_>) {
        match token {
            Token::Text(text) => self.text(text),
            Token::Open { name, attrs } => {
                let style_attr = attrs
                    .iter()
                    .find(|(k, _)| k == "style")
                    .map_or("", |(_, v)| v.as_str());
                match name.as_str() {
                    "p" | "div" | "blockquote" | "tr" | "dt" | "dd" | "pre" => {
                        self.start_paragraph(ParagraphStyle::Normal, false);
                    }
                    "h1" => self.start_paragraph(ParagraphStyle::Heading1, false),
                    "h2" => self.start_paragraph(ParagraphStyle::Heading2, false),
                    "h3" | "h4" | "h5" | "h6" => {
                        self.start_paragraph(ParagraphStyle::Heading3, false);
                    }
                    "li" => self.start_paragraph(ParagraphStyle::Normal, true),
                    "ul" => {
                        self.finish_paragraph();
                        self.lists.push(ListKind::Bullet);
                    }
                    "ol" => {
                        self.finish_paragraph();
                        self.lists.push(ListKind::Numbered);
                    }
                    "br" => {
                        let style = self
                            .current
                            .as_ref()
                            .map_or(ParagraphStyle::Normal, |p| p.style);
                        let list = self.current.as_ref().and_then(|p| p.format.list);
                        self.finish_paragraph();
                        let mut next = Paragraph::new(style);
                        next.format.list = list;
                        self.current = Some(next);
                    }
                    "td" | "th" => self.text(" "),
                    _ => {}
                }
                if let Some(paragraph) = &mut self.current
                    && matches!(
                        name.as_str(),
                        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li"
                    )
                {
                    paragraph.format.align = text_align(style_attr);
                }
                if is_void(&name) {
                    return;
                }
                let mut format = self.format();
                match name.as_str() {
                    "b" | "strong" => format.bold = Some(true),
                    "i" | "em" | "cite" => format.italic = Some(true),
                    "u" | "ins" => format.underline = true,
                    "s" | "strike" | "del" => format.strikethrough = true,
                    "font" => {
                        if let Some((_, color)) = attrs.iter().find(|(k, _)| k == "color") {
                            format.color = parse_color(color).or(format.color);
                        }
                        if let Some((_, face)) = attrs.iter().find(|(k, _)| k == "face") {
                            format.family = first_family(face).or(format.family);
                        }
                    }
                    _ => {}
                }
                apply_css(style_attr, &mut format);
                self.formats.push((name, format));
            }
            Token::Close(name) => {
                match name.as_str() {
                    "p" | "div" | "blockquote" | "tr" | "dt" | "dd" | "pre" | "h1" | "h2"
                    | "h3" | "h4" | "h5" | "h6" | "li" => self.finish_paragraph(),
                    "ul" | "ol" => {
                        self.finish_paragraph();
                        self.lists.pop();
                    }
                    _ => {}
                }
                // Close the innermost element of this name (tolerating bad nesting).
                if let Some(at) = self.formats.iter().rposition(|(n, _)| *n == name) {
                    self.formats.truncate(at);
                }
            }
        }
    }
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "br" | "img" | "hr" | "meta" | "link" | "input" | "col" | "wbr" | "area" | "base"
    )
}

/// Applies the CSS declarations word processors use for character formatting.
fn apply_css(style: &str, format: &mut CharFormat) {
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim();
        let lower = value.to_ascii_lowercase();
        match property.as_str() {
            "font-weight" => {
                format.bold = Some(match lower.as_str() {
                    "bold" | "bolder" => true,
                    "normal" | "lighter" => false,
                    n => n.parse::<u16>().is_ok_and(|w| w >= 600),
                });
            }
            "font-style" => {
                format.italic = Some(lower.contains("italic") || lower.contains("oblique"))
            }
            "text-decoration" | "text-decoration-line" => {
                if lower.contains("none") {
                    format.underline = false;
                    format.strikethrough = false;
                }
                format.underline |= lower.contains("underline");
                format.strikethrough |= lower.contains("line-through");
            }
            "color" => format.color = parse_color(&lower).or(format.color),
            "font-size" => format.size = parse_size(&lower).or(format.size),
            "font-family" => format.family = first_family(value).or(format.family.clone()),
            _ => {}
        }
    }
}

fn text_align(style: &str) -> Option<crate::model::Align> {
    use crate::model::Align;
    style.split(';').find_map(|d| {
        let (property, value) = d.split_once(':')?;
        if !property.trim().eq_ignore_ascii_case("text-align") {
            return None;
        }
        match value.trim().to_ascii_lowercase().as_str() {
            "center" => Some(Align::Center),
            "right" | "end" => Some(Align::Right),
            "justify" => Some(Align::Justify),
            _ => None,
        }
    })
}

/// `#rgb`, `#rrggbb` or `rgb(r, g, b)`. Black is "automatic" and stored as none.
fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    let color = if let Some(hex) = value.strip_prefix('#') {
        let expand = |s: &str| u8::from_str_radix(s, 16).ok();
        match hex.len() {
            3 => {
                let c: Vec<u8> = hex
                    .chars()
                    .filter_map(|c| expand(&format!("{c}{c}")))
                    .collect();
                (c.len() == 3).then(|| Color(c[0], c[1], c[2]))?
            }
            6 => Color(
                expand(&hex[0..2])?,
                expand(&hex[2..4])?,
                expand(&hex[4..6])?,
            ),
            _ => return None,
        }
    } else {
        let inner = value
            .strip_prefix("rgba(")
            .or_else(|| value.strip_prefix("rgb("))
            .and_then(|v| v.strip_suffix(')'))?;
        let parts: Vec<u8> = inner
            .split(',')
            .take(3)
            .filter_map(|p| p.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 255.0) as u8)
            .collect();
        if parts.len() != 3 {
            return None;
        }
        Color(parts[0], parts[1], parts[2])
    };
    (color != Color::BLACK).then_some(color)
}

/// `12pt`, `16px` (→ 12 pt), `1.5em` is ignored.
fn parse_size(value: &str) -> Option<f32> {
    let (number, factor) = match value.strip_suffix("pt") {
        Some(n) => (n, 1.0),
        None => (value.strip_suffix("px")?, 0.75),
    };
    number
        .trim()
        .parse::<f32>()
        .ok()
        .map(|v| v * factor)
        .filter(|v| (4.0..=400.0).contains(v))
}

fn first_family(value: &str) -> Option<String> {
    let family = value
        .split(',')
        .next()?
        .trim()
        .trim_matches(['"', '\''])
        .trim();
    let generic = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "inherit",
    ];
    (!family.is_empty() && !generic.contains(&family.to_ascii_lowercase().as_str()))
        .then(|| family.to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::Align;

    fn read(html: &str) -> Vec<Paragraph> {
        fragment_from_html(html).unwrap().paragraphs().to_vec()
    }

    #[test]
    fn paragraphs_headings_and_inline_formatting() {
        let p = read(
            "<h1>Title</h1><p>Plain <b>bold</b> and <i>italic <u>both</u></i>.</p><p>Next</p>",
        );
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].style, ParagraphStyle::Heading1);
        assert_eq!(p[0].text(), "Title");
        assert_eq!(p[1].text(), "Plain bold and italic both.");
        let runs = &p[1].runs;
        assert_eq!(runs[1].text, "bold");
        assert_eq!(runs[1].format.bold, Some(true));
        let both = runs.iter().find(|r| r.text == "both").unwrap();
        assert_eq!(both.format.italic, Some(true));
        assert!(both.format.underline);
        assert_eq!(runs.last().unwrap().format, CharFormat::default());
    }

    #[test]
    fn google_docs_wrapper_bold_is_cancelled_by_its_style() {
        let html = r#"<meta charset="utf-8"><b style="font-weight:normal;" id="docs-internal-guid-1"><p dir="ltr"><span style="font-size:11pt;font-family:Arial,sans-serif;color:#000000;font-weight:400;">Hello </span><span style="font-size:11pt;font-family:Arial;color:#ff0000;font-weight:700;">world</span></p></b>"#;
        let p = read(html);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text(), "Hello world");
        let hello = &p[0].runs[0];
        assert_eq!(hello.format.bold, Some(false));
        assert_eq!(hello.format.size, Some(11.0));
        assert_eq!(hello.format.family.as_deref(), Some("Arial"));
        assert_eq!(hello.format.color, None, "black is automatic");
        let world = &p[0].runs[1];
        assert_eq!(world.format.bold, Some(true));
        assert_eq!(world.format.color, Some(Color(255, 0, 0)));
    }

    #[test]
    fn lists_nest_and_breaks_split_paragraphs() {
        let p = read(
            "<ul><li>one</li><li>two<ul><li>deep</li></ul></li></ul><ol><li>first</li></ol>line<br>break",
        );
        let lists: Vec<_> = p.iter().map(|p| p.format.list).collect();
        assert_eq!(p[0].text(), "one");
        assert_eq!(
            lists[0],
            Some(List {
                kind: ListKind::Bullet,
                level: 0
            })
        );
        assert_eq!(p[2].text(), "deep");
        assert_eq!(
            lists[2],
            Some(List {
                kind: ListKind::Bullet,
                level: 1
            })
        );
        assert_eq!(
            lists[3],
            Some(List {
                kind: ListKind::Numbered,
                level: 0
            })
        );
        assert_eq!(p[4].text(), "line");
        assert_eq!(p[5].text(), "break");
        assert_eq!(lists[4], None);
    }

    #[test]
    fn whitespace_entities_and_junk() {
        let html = "<html><head><title>x</title><style>p{color:red}</style></head><body>\n  <!-- note -->\n  <p style=\"text-align:center\">  A&amp;B&nbsp;&lt;tag&gt; &#8364;&#x41;   end  </p><script>alert(1)</script><p>&unknown; &</p></body></html>";
        let p = read(html);
        assert_eq!(p[0].text(), "A&B <tag> €A end");
        assert_eq!(p[0].format.align, Some(Align::Center));
        assert_eq!(p[1].text(), "&unknown; &");
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn empty_or_huge_html_is_not_formatted_text() {
        assert!(fragment_from_html("<p>  </p><br>").is_none());
        assert!(fragment_from_html(&"x".repeat(MAX_HTML + 1)).is_none());
    }

    #[test]
    fn ferrix_html_reads_back_with_its_formatting() {
        let mut heading =
            Paragraph::new(ParagraphStyle::Heading2).with_text("Plan", CharFormat::default());
        heading.format.align = Some(Align::Center);
        let mut item =
            Paragraph::new(ParagraphStyle::Normal).with_text("Ship <it> & ", CharFormat::default());
        item.push(
            "now",
            CharFormat {
                bold: Some(true),
                italic: Some(true),
                underline: true,
                strikethrough: true,
                color: Some(Color(0x1a, 0x5f, 0xb4)),
                size: Some(14.0),
                family: Some("Liberation Serif".into()),
            },
        );
        item.format.list = Some(List {
            kind: ListKind::Numbered,
            level: 0,
        });
        let fragment = Fragment::new(vec![heading.clone(), item.clone()]).unwrap();
        let html = fragment_to_html(&fragment);
        let back = read(&html);
        assert_eq!(back[0].style, ParagraphStyle::Heading2);
        assert_eq!(back[0].format.align, Some(Align::Center));
        assert_eq!(back[1].runs, item.runs);
        assert_eq!(back[1].format.list, item.format.list);
    }
}
