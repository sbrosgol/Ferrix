//! Export to PDF, read back by PDFium and by `open_pdf`.

#![allow(clippy::unwrap_used)]

use ferrix_compose::{
    CharFormat, Document, Fonts, Layout, Opened, Paragraph, ParagraphStyle, export_pdf, open_pdf,
};
use ferrix_engine::{DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_pdf::{LayoutPage, Source, write_page_layout};
use ferrix_types::{PageIndex, Rotation};

fn sample() -> Document {
    let mut doc = Document::new();
    doc.title = Some("Quarterly report".into());
    let mut title =
        Paragraph::new(ParagraphStyle::Title).with_text("Quarterly report", CharFormat::default());
    title.format.align = Some(ferrix_compose::Align::Center);
    let mut body =
        Paragraph::new(ParagraphStyle::Normal).with_text("Sales grew ", CharFormat::default());
    body.push(
        "twelve percent",
        CharFormat {
            bold: Some(true),
            underline: true,
            ..CharFormat::default()
        },
    );
    body.push(" — straße, 日本, café.", CharFormat::default());
    doc.paragraphs = vec![title, body];
    // Enough text for a second page.
    for i in 0..80 {
        doc.paragraphs.push(
            Paragraph::new(ParagraphStyle::Normal)
                .with_text(&format!("Line {i} of the appendix."), CharFormat::default()),
        );
    }
    doc
}

fn export(doc: &Document) -> Vec<u8> {
    let mut fonts = Fonts::bundled();
    let layout = Layout::new(doc, &mut fonts);
    export_pdf(doc, &layout, &fonts).unwrap()
}

fn engine() -> Option<PdfiumEngine> {
    match PdfiumEngine::new() {
        Ok(engine) => Some(engine),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping: {e}");
            None
        }
        Err(e) => panic!("PDFium is required on CI: {e}"),
    }
}

#[test]
fn exported_pdf_opens_in_pdfium_with_the_right_pages_and_text() {
    let doc = sample();
    let mut fonts = Fonts::bundled();
    let layout = Layout::new(&doc, &mut fonts);
    assert!(layout.pages.len() >= 2);
    let bytes = export_pdf(&doc, &layout, &fonts).unwrap();

    let Some(mut engine) = engine() else { return };
    let info = engine.open(DocSource::Bytes(bytes), None).unwrap();
    assert_eq!(info.page_count as usize, layout.pages.len());
    let page = engine.page_info(info.id, PageIndex(0)).unwrap();
    assert!((page.size.width - 595.28).abs() < 0.5, "{:?}", page.size);
    let text = engine.text_page(info.id, PageIndex(0)).unwrap().text();
    assert!(text.contains("Quarterly report"), "{text}");
    assert!(text.contains("twelve percent"), "{text}");
    assert!(text.contains("straße"), "{text}");
    let last = engine
        .text_page(info.id, PageIndex(info.page_count - 1))
        .unwrap()
        .text();
    assert!(last.contains("Line 79 of the appendix."), "{last}");
}

#[test]
fn the_source_comes_back_from_the_pdf() {
    let doc = sample();
    let bytes = export(&doc);
    assert_eq!(open_pdf(&bytes).unwrap(), Opened::Editable(doc));
}

#[test]
fn export_is_deterministic() {
    let doc = sample();
    assert_eq!(export(&doc), export(&doc));
}

#[test]
fn changes_made_elsewhere_are_detected() {
    let doc = sample();
    let bytes = export(&doc);

    // Another application deletes the last page.
    let pages = ferrix_pdf::content_digest(Source::new(&bytes))
        .unwrap()
        .pages as u32;
    let fewer: Vec<LayoutPage> = (0..pages - 1)
        .map(|page| LayoutPage {
            source: 0,
            page,
            rotate_by: Rotation::R0,
        })
        .collect();
    let edited = write_page_layout(Source::new(&bytes), &[], &fewer).unwrap();
    assert_eq!(open_pdf(&edited).unwrap(), Opened::Changed(doc.clone()));

    // ...or rotates a page.
    let mut rotated: Vec<LayoutPage> = (0..pages)
        .map(|page| LayoutPage {
            source: 0,
            page,
            rotate_by: Rotation::R0,
        })
        .collect();
    rotated[0].rotate_by = Rotation::R90;
    let edited = write_page_layout(Source::new(&bytes), &[], &rotated).unwrap();
    assert_eq!(open_pdf(&edited).unwrap(), Opened::Changed(doc));
}

#[test]
fn ordinary_pdfs_are_plain() {
    let blank = ferrix_pdf::blank_document(1, ferrix_pdf::PaperSize::A4).unwrap();
    assert_eq!(open_pdf(&blank).unwrap(), Opened::Plain);
    assert_eq!(open_pdf(b"not a pdf").unwrap(), Opened::Plain);
}

#[test]
fn an_empty_document_exports_one_page() {
    let doc = Document::new();
    let bytes = export(&doc);
    let digest = ferrix_pdf::content_digest(Source::new(&bytes)).unwrap();
    assert_eq!(digest.pages, 1);
    assert_eq!(open_pdf(&bytes).unwrap(), Opened::Editable(doc));
}
