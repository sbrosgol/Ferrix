//! The selection model on text PDFium extracted from a real PDF.

#![allow(clippy::unwrap_used)]

use ferrix_compose::{CharFormat, Document, Fonts, Layout, Paragraph, ParagraphStyle};
use ferrix_engine::{DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_text::{PageText, Selection};
use ferrix_types::{PageIndex, Point};

fn engine() -> Option<PdfiumEngine> {
    match PdfiumEngine::new() {
        Ok(e) => Some(e),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping: {e}");
            None
        }
        Err(e) => panic!("PDFium is required on CI: {e}"),
    }
}

#[test]
fn selects_words_and_lines_of_a_real_pdf() {
    let Some(mut engine) = engine() else { return };
    let mut doc = Document::new();
    doc.paragraphs = vec![
        Paragraph::new(ParagraphStyle::Normal)
            .with_text("Quarterly figures look good", CharFormat::default()),
        Paragraph::new(ParagraphStyle::Normal)
            .with_text("Second paragraph here", CharFormat::default()),
    ];
    let mut fonts = Fonts::bundled();
    let layout = Layout::new(&doc, &mut fonts);
    let pdf = ferrix_compose::export_pdf(&doc, &layout, &fonts).unwrap();

    let info = engine.open(DocSource::Bytes(pdf), None).unwrap();
    let text = PageText::new(&engine.text_page(info.id, PageIndex(0)).unwrap());
    let all = text.text(0..text.len());
    assert!(all.contains("Quarterly figures look good"), "{all:?}");

    // Positions are glyph indices, so search the raw glyphs (copied text drops '\r').
    let raw: String = text.glyphs().iter().map(|g| g.ch).collect();
    let index_of = |needle: &str| raw[..raw.find(needle).unwrap()].chars().count();

    // Click in the middle of "figures".
    let start = index_of("figures");
    let g = &text.glyphs()[start + 2];
    let mid = Point::new(
        (g.bounds.x0 + g.bounds.x1) / 2.0,
        (g.bounds.y0 + g.bounds.y1) / 2.0,
    );
    let caret = text.caret_at(mid).unwrap();
    assert_eq!(text.text(text.word_at(caret)), "figures");
    assert_eq!(
        text.text(text.line_at(caret)),
        "Quarterly figures look good"
    );

    // A selection from "look" to "Second" spans two lines and two boxes.
    let from = index_of("look");
    let to = index_of("paragraph") - 1;
    let sel = Selection {
        anchor: from,
        focus: to,
    }
    .range();
    assert_eq!(text.boxes(sel.clone()).len(), 2);
    let copied = text.text(sel);
    assert!(
        copied.starts_with("look good") && copied.ends_with("Second"),
        "{copied:?}"
    );
}
