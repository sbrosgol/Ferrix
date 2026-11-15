//! Tagged PDF structure and PDF/A-3b export.

#![allow(clippy::unwrap_used)]

use ferrix_compose::{
    CharFormat, Document, Editor, Fonts, ImageData, Layout, ListKind, Motion, Opened, PageText,
    Paragraph, ParagraphStyle, Position, TextRange, export_pdf, open_pdf,
};
use lopdf::Object;

/// A document using every feature: title, headings, a list, a picture, a page break, a footer.
fn rich() -> Document {
    let mut doc = Document::new();
    doc.paragraphs = vec![
        Paragraph::new(ParagraphStyle::Title).with_text("Annual report", CharFormat::default()),
        Paragraph::new(ParagraphStyle::Heading2).with_text("Summary", CharFormat::default()),
        Paragraph::new(ParagraphStyle::Normal).with_text(
            "Revenue grew. ",
            CharFormat {
                underline: true,
                ..CharFormat::default()
            },
        ),
        Paragraph::new(ParagraphStyle::Normal).with_text("first", CharFormat::default()),
        Paragraph::new(ParagraphStyle::Normal).with_text("second", CharFormat::default()),
        Paragraph::new(ParagraphStyle::Normal),
    ];
    let mut e = Editor::new(doc, Fonts::bundled());
    e.select(TextRange::between(Position::new(3, 0), Position::new(4, 0)));
    e.toggle_list(ListKind::Numbered);
    e.move_caret(Motion::DocumentEnd, false);
    let picture = image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 120, 200, 255]));
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    e.insert_image(ImageData::from_bytes(png).unwrap());
    e.type_text("Chart of revenue");
    e.page_break();
    e.type_text("Appendix");
    let mut settings = e.document().settings();
    settings.title = Some("Annual report".into());
    settings.footer = Some(PageText::new("Page {page} of {pages}"));
    e.set_settings(settings);
    e.document().clone()
}

fn export(doc: &Document) -> Vec<u8> {
    let mut fonts = Fonts::bundled();
    let layout = Layout::new(doc, &mut fonts);
    export_pdf(doc, &layout, &fonts).unwrap()
}

/// Structure element types in document order, depth-first.
fn structure(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    assert!(catalog.get(b"MarkInfo").is_ok(), "marked as tagged");
    let root = catalog
        .get_deref(b"StructTreeRoot", &doc)
        .unwrap()
        .as_dict()
        .unwrap();
    let mut out = Vec::new();
    fn walk(doc: &lopdf::Document, node: &Object, out: &mut Vec<String>) {
        let Ok((_, node)) = doc.dereference(node) else {
            return;
        };
        match node {
            Object::Array(items) => items.iter().for_each(|i| walk(doc, i, out)),
            Object::Dictionary(dict) => {
                if let Ok(Object::Name(kind)) = dict.get(b"S") {
                    out.push(String::from_utf8_lossy(kind).into_owned());
                }
                if let Ok(kids) = dict.get(b"K") {
                    walk(doc, kids, out);
                }
            }
            _ => {}
        }
    }
    walk(&doc, root.get(b"K").unwrap(), &mut out);
    out
}

#[test]
fn exported_pdfs_are_tagged_in_reading_order() {
    let tags = structure(&export(&rich()));
    let pos = |t: &str| {
        tags.iter()
            .position(|s| s == t)
            .unwrap_or_else(|| panic!("{t} in {tags:?}"))
    };
    assert!(pos("H1") < pos("H2"), "{tags:?}");
    assert!(pos("H2") < pos("P"));
    assert!(pos("L") < pos("Figure"));
    for kind in ["LI", "Lbl", "LBody"] {
        assert!(tags.contains(&kind.to_owned()), "{kind} missing: {tags:?}");
    }
    assert_eq!(tags.iter().filter(|t| *t == "LI").count(), 2);
}

#[test]
fn archival_documents_export_as_pdf_a_3b_and_stay_editable() {
    let mut doc = rich();
    doc.archival = true;
    let pdf = export(&doc);
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("pdfaid:part"),
        "PDF/A identification in the XMP metadata"
    );
    assert!(text.contains("OutputIntent") || text.contains("OutputIntents"));
    assert_eq!(open_pdf(&pdf).unwrap(), Opened::Editable(doc));
}

#[test]
fn ordinary_exports_are_not_dated() {
    let doc = rich();
    assert_eq!(export(&doc), export(&doc));
}
