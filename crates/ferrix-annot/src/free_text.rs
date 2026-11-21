//! Text boxes (`FreeText`, ADR-0009 phase 3): laying out their text with `ferrix-compose` in
//! the bundled Liberation Sans, measuring it, and rendering it with krilla so the font is
//! embedded and any script works. The rendered page becomes the appearance stream.

use std::sync::{LazyLock, Mutex};

use ferrix_compose::{
    CharFormat, Color, Document, Fonts, Layout, Margins, PageSetup, Paragraph, ParagraphStyle,
};
use ferrix_core::Rgb;

use crate::AnnotError;

/// Space between the text and the box edge, in points.
pub const PADDING: f32 = 2.0;
/// The font sizes offered for text boxes.
pub const SIZES: [f32; 7] = [8.0, 10.0, 12.0, 14.0, 18.0, 24.0, 36.0];
const FAMILY: &str = "Liberation Sans";
/// Widest box an auto-width text box grows to before wrapping, in points.
const MAX_AUTO_WIDTH: f32 = 2000.0;

/// Loading the bundled fonts is slow; every text box shares one set.
static FONTS: LazyLock<Mutex<Fonts>> = LazyLock::new(|| Mutex::new(Fonts::bundled()));

fn document(text: &str, size: f32, color: Rgb, width: f32, height: f32) -> Document {
    let mut page = PageSetup::new((width.max(1.0), height.max(1.0)));
    page.margins = Margins::uniform(PADDING);
    let mut doc = Document::with_page(page);
    let format = CharFormat {
        family: Some(FAMILY.into()),
        size: Some(size),
        color: Some(Color(channel(color.r), channel(color.g), channel(color.b))),
        ..CharFormat::default()
    };
    doc.paragraphs = text
        .split('\n')
        .map(|line| {
            let mut p = Paragraph::new(ParagraphStyle::Normal).with_text(line, format.clone());
            p.format.space_before = Some(0.0);
            p.format.space_after = Some(0.0);
            p.format.line_spacing = Some(1.0);
            p
        })
        .collect();
    doc
}

fn channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The box a text needs: `width` given (wrapping inside it), or the text's natural width when
/// `None`. Padding included.
pub fn measure(text: &str, size: f32, width: Option<f32>) -> (f32, f32) {
    let Ok(mut fonts) = FONTS.lock() else {
        return (width.unwrap_or(100.0), size * 1.2 + 2.0 * PADDING);
    };
    let probe_width = width.unwrap_or(MAX_AUTO_WIDTH);
    // Within the largest PDF page (14 400 pt); taller pages are refused by the layout.
    let doc = document(text, size, Rgb::BLACK, probe_width, 14_000.0);
    let layout = Layout::new(&doc, &mut fonts);
    let lines = layout.pages.first().map_or(&[][..], |p| p.lines.as_slice());
    let height = lines
        .last()
        .map_or(size * 1.2, |l| l.top + l.height - PADDING)
        .max(size * 1.2);
    let natural = lines
        .iter()
        .flat_map(|l| l.glyphs.iter())
        .map(|g| g.x + g.advance - PADDING)
        .fold(0.0_f32, f32::max);
    let width = width.unwrap_or(natural.ceil() + 2.0 * PADDING + 1.0);
    (width.max(2.0 * PADDING + size), height + 2.0 * PADDING)
}

/// The text set in a `width` × `height` page, as a one-page PDF (untagged, nothing embedded but
/// the font subset), for [`ferrix_pdf::Revision::import_page_as_form`].
/// With `icc`, colours are ICC-based sRGB, for PDF/A files (see
/// [`ferrix_compose::export_plain_page`]).
pub fn render(
    text: &str,
    size: f32,
    color: Rgb,
    width: f32,
    height: f32,
    icc: bool,
) -> Result<Vec<u8>, AnnotError> {
    let mut fonts = FONTS
        .lock()
        .map_err(|_| AnnotError::NotAllowed("the text layout failed earlier".into()))?;
    let doc = document(text, size, color, width, height);
    let layout = Layout::new(&doc, &mut fonts);
    ferrix_compose::export_plain_page(&doc, &layout, &fonts, icc)
        .map_err(|e| AnnotError::NotAllowed(format!("couldn’t set the text: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_width_fits_the_longest_line_and_lines_add_height() {
        let (w1, h1) = measure("Short", 12.0, None);
        let (w2, h2) = measure("A much longer line of text\nand a second", 12.0, None);
        assert!(w2 > w1 * 2.0, "{w1} {w2}");
        assert!(h2 - h1 > 12.0, "a second line adds its height: {h1} {h2}");
        let (w3, h3) = measure(
            "A much longer line of text that has to wrap",
            12.0,
            Some(80.0),
        );
        assert_eq!(w3, 80.0);
        assert!(h3 > h1 * 2.5, "wraps into several lines: {h3}");
    }

    #[test]
    fn renders_a_pdf_page() {
        let (w, h) = measure("Hello", 14.0, None);
        let pdf = render("Hello", 14.0, Rgb::new(0.1, 0.2, 0.8), w, h, false).expect("renders");
        assert!(pdf.starts_with(b"%PDF"));
    }
}
