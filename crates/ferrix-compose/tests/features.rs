//! Phase 3 features: lists, page breaks, pictures, headers and footers, find and replace.

#![allow(clippy::unwrap_used)]

use ferrix_compose::{
    CharFormat, Document, Editor, Fonts, ImageData, Layout, ListKind, Motion, Opened, PageText,
    Paragraph, ParagraphStyle, Position, Rasterizer, TextRange, export_pdf, open_pdf,
};
use ferrix_engine::{DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;

fn editor(paragraphs: &[&str]) -> Editor {
    let mut doc = Document::new();
    doc.paragraphs = paragraphs
        .iter()
        .map(|t| Paragraph::new(ParagraphStyle::Normal).with_text(t, CharFormat::default()))
        .collect();
    Editor::new(doc, Fonts::bundled())
}

fn texts(e: &Editor) -> Vec<String> {
    e.document()
        .paragraphs
        .iter()
        .map(Paragraph::text)
        .collect()
}

fn markers(e: &Editor) -> Vec<Option<String>> {
    let mut out = vec![None; e.document().paragraphs.len()];
    for line in e.layout().pages.iter().flat_map(|p| &p.lines) {
        if let Some((marker, _)) = &line.marker {
            out[line.block] = Some(marker.clone());
        }
    }
    out
}

/// A small red-and-blue PNG.
fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            image::Rgba([220, 20, 20, 255])
        } else {
            image::Rgba([20, 20, 220, 255])
        }
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
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
fn numbered_lists_count_restart_and_nest() {
    let mut e = editor(&["one", "two", "three", "break", "again"]);
    e.select(TextRange::between(Position::new(0, 0), Position::new(2, 0)));
    e.toggle_list(ListKind::Numbered);
    e.select(TextRange::caret(Position::new(4, 0)));
    e.toggle_list(ListKind::Numbered);
    assert_eq!(
        markers(&e),
        [Some("1."), Some("2."), Some("3."), None, Some("1.")].map(|m| m.map(str::to_owned))
    );

    // Tab at the start of an item nests it; numbering at the new level starts at "a".
    e.select(TextRange::caret(Position::new(1, 0)));
    e.tab(false);
    assert_eq!(markers(&e)[1].as_deref(), Some("a."));
    assert_eq!(markers(&e)[2].as_deref(), Some("2."));
    e.tab(true);
    assert_eq!(markers(&e)[1].as_deref(), Some("2."));

    // List items are indented, with the marker hanging to their left.
    let line = &e.layout().pages[0].lines[0];
    let (_, marker) = line.marker.as_ref().unwrap();
    assert!(line.left > 72.0);
    assert!(marker[0].x < line.left);

    // Toggling again turns them back into plain paragraphs.
    e.select(TextRange::between(Position::new(0, 0), Position::new(2, 0)));
    e.toggle_list(ListKind::Numbered);
    assert_eq!(markers(&e)[..3], [None, None, None]);
}

#[test]
fn enter_and_backspace_in_lists() {
    let mut e = editor(&["item"]);
    e.toggle_list(ListKind::Bullet);
    e.move_caret(Motion::DocumentEnd, false);
    e.enter();
    assert_eq!(markers(&e), [Some("•".into()), Some("•".into())]);
    // Enter in the new, empty item ends the list instead of adding another.
    e.enter();
    assert_eq!(texts(&e), ["item", ""]);
    assert_eq!(markers(&e), [Some("•".into()), None]);

    // Backspace at the start of an item removes the bullet, not the text.
    e.select(TextRange::caret(Position::new(0, 0)));
    e.backspace(false);
    assert_eq!(texts(&e), ["item", ""]);
    assert_eq!(markers(&e), [None, None]);
}

#[test]
fn tab_outside_lists_types_a_tab() {
    let mut e = editor(&["a"]);
    e.move_caret(Motion::DocumentEnd, false);
    e.tab(false);
    assert_eq!(texts(&e), ["a\t"]);
}

#[test]
fn page_breaks_start_new_pages_and_backspace_removes_them() {
    let mut e = editor(&["first page second page"]);
    e.select(TextRange::caret(Position::new(0, 11)));
    e.page_break();
    assert_eq!(texts(&e), ["first page ", "second page"]);
    assert_eq!(e.layout().pages.len(), 2);
    assert_eq!(e.layout().pages[1].lines[0].block, 1);
    assert_eq!(e.focus(), Position::new(1, 0));

    // Enter after the break does not create a second break.
    e.move_caret(Motion::DocumentEnd, false);
    e.enter();
    assert_eq!(e.layout().pages.len(), 2);

    e.select(TextRange::caret(Position::new(1, 0)));
    e.backspace(false);
    assert_eq!(texts(&e), ["first page ", "second page", ""]);
    assert_eq!(e.layout().pages.len(), 1);
    e.undo();
    assert_eq!(e.layout().pages.len(), 2);
}

#[test]
fn pictures_are_laid_out_drawn_and_removed() {
    let mut e = editor(&["Before", "After"]);
    e.move_caret(Motion::DocumentStart, false);
    e.move_caret(Motion::LineEnd, false);
    let data = ImageData::from_bytes(png(400, 200)).unwrap();
    assert_eq!((data.pixel_width, data.pixel_height), (400, 200));
    e.insert_image(data);
    assert_eq!(texts(&e), ["Before", "", "", "After"]);
    assert!(e.document().paragraphs[1].image.is_some());

    // The picture has a line of its own, 300 × 150 pt (400 px at 96 dpi), centred.
    let page = &e.layout().pages[0];
    let line = page.lines.iter().find(|l| l.image.is_some()).unwrap();
    let image = line.image.clone().unwrap();
    assert!((image.width - 300.0).abs() < 0.01);
    assert!((image.height - 150.0).abs() < 0.01);
    let centre = image.x + image.width / 2.0;
    assert!((centre - e.document().page.width / 2.0).abs() < 1.0);

    // The rasterized page shows red and blue where the picture is.
    let mut rasterizer = Rasterizer::new();
    let raster = e.rasterize(&mut rasterizer, 0, 1.0).unwrap();
    let pixel = |x: f32, y: f32| {
        let i = (y as usize * raster.width as usize + x as usize) * 4;
        [raster.pixels[i], raster.pixels[i + 1], raster.pixels[i + 2]]
    };
    let mid_y = image.top + image.height / 2.0;
    assert!(pixel(image.x + 20.0, mid_y)[0] > 180, "red half");
    assert!(
        pixel(image.x + image.width - 20.0, mid_y)[2] > 180,
        "blue half"
    );

    // Backspace at the start of the picture's paragraph removes the picture.
    e.select(TextRange::caret(Position::new(1, 0)));
    e.backspace(false);
    assert!(e.document().paragraphs.iter().all(|p| p.image.is_none()));
    e.undo();
    assert!(e.document().paragraphs[1].image.is_some());
}

#[test]
fn wide_pictures_shrink_to_the_text_width() {
    let mut e = editor(&[""]);
    e.insert_image(ImageData::from_bytes(png(2000, 1000)).unwrap());
    let line = &e.layout().pages[0].lines[0];
    let image = line.image.as_ref().unwrap();
    let text_width = e.document().page.content_width();
    assert!((image.width - text_width).abs() < 0.01);
    assert!((image.height - text_width / 2.0).abs() < 0.01);
}

#[test]
fn only_png_and_jpeg_pictures_are_accepted() {
    assert!(ImageData::from_bytes(b"GIF89a....".to_vec()).is_err());
    assert!(ImageData::from_bytes(b"not a picture".to_vec()).is_err());
}

#[test]
fn pictures_survive_saving_and_reopening() {
    let mut e = editor(&["Figure:"]);
    e.move_caret(Motion::DocumentEnd, false);
    e.insert_image(ImageData::from_bytes(png(64, 32)).unwrap());
    let doc = e.document().clone();
    let json = doc.to_json();
    assert_eq!(Document::from_json(&json).unwrap(), doc);

    let mut fonts = Fonts::bundled();
    let layout = Layout::new(&doc, &mut fonts);
    let pdf = export_pdf(&doc, &layout, &fonts).unwrap();
    assert_eq!(open_pdf(&pdf).unwrap(), Opened::Editable(doc));

    let Some(mut engine) = engine() else { return };
    let info = engine.open(DocSource::Bytes(pdf), None).unwrap();
    assert_eq!(info.page_count, 1);
}

#[test]
fn headers_and_footers_number_the_pages() {
    let mut e = editor(&["Title page", "Body"]);
    e.select(TextRange::caret(Position::new(1, 0)));
    e.page_break();
    let mut settings = e.document().settings();
    settings.header = Some(PageText {
        skip_first: true,
        ..PageText::new("Quarterly report")
    });
    settings.footer = Some(PageText::new("Page {page} of {pages}"));
    e.set_settings(settings);

    let pages = &e.layout().pages;
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].furniture.len(), 1, "no header on the title page");
    assert_eq!(pages[1].furniture.len(), 2);
    let footer = pages[1].furniture.iter().find(|l| l.top > 700.0).unwrap();
    assert_eq!(footer.source_text.as_deref(), Some("Page 2 of 2"));
    assert!(footer.top + footer.height <= e.document().page.height);
    // Page furniture is not editable text: clicks and the caret ignore it.
    let count = e.document().paragraphs.len();
    assert!(pages.iter().flat_map(|p| &p.lines).all(|l| l.block < count));

    let doc = e.document().clone();
    let mut fonts = Fonts::bundled();
    let layout = Layout::new(&doc, &mut fonts);
    let pdf = export_pdf(&doc, &layout, &fonts).unwrap();
    let Some(mut engine) = engine() else { return };
    let info = engine.open(DocSource::Bytes(pdf), None).unwrap();
    let first = engine
        .text_page(info.id, ferrix_types::PageIndex(0))
        .unwrap()
        .text();
    let second = engine
        .text_page(info.id, ferrix_types::PageIndex(1))
        .unwrap()
        .text();
    assert!(
        first.contains("Page 1 of 2") && !first.contains("Quarterly"),
        "{first}"
    );
    assert!(
        second.contains("Page 2 of 2") && second.contains("Quarterly report"),
        "{second}"
    );
}

#[test]
fn find_and_replace() {
    let mut e = editor(&["The cat sat.", "Another Cat, another cat."]);
    assert_eq!(e.find_all("cat", false).len(), 3);
    assert_eq!(e.find_all("cat", true).len(), 2);
    assert!(e.find_all("", false).is_empty());

    e.move_caret(Motion::DocumentStart, false);
    assert!(e.find_next("cat", false, false));
    assert_eq!(e.selected_text(), "cat");
    assert_eq!(e.selection().start, Position::new(0, 4));
    assert!(e.find_next("cat", false, false));
    assert_eq!(e.selection().start, Position::new(1, 8));
    assert!(e.find_next("cat", false, true), "backwards");
    assert_eq!(e.selection().start, Position::new(0, 4));

    // Replace replaces the current match and moves on.
    e.replace("cat", "dog", false);
    assert_eq!(texts(&e)[0], "The dog sat.");
    assert_eq!(e.selection().start, Position::new(1, 8));

    // Replace All is one undo step.
    assert_eq!(e.replace_all("cat", "bird", false), 2);
    assert_eq!(texts(&e), ["The dog sat.", "Another bird, another bird."]);
    assert_eq!(e.undo_label(), Some("Replace All"));
    e.undo();
    assert_eq!(texts(&e), ["The dog sat.", "Another Cat, another cat."]);
    assert!(!e.find_next("zebra", false, false));
}

#[test]
fn case_insensitive_matching_keeps_byte_offsets_right() {
    let e = editor(&["Straße STRASSE ÉCOLE école"]);
    let found = e.find_all("école", false);
    assert_eq!(found.len(), 2);
    for range in found {
        assert!(e.document().check(range.start).is_ok());
        assert!(e.document().check(range.end).is_ok());
    }
}
