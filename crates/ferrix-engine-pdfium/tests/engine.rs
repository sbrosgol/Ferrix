//! Integration tests against the real PDFium library.
//!
//! Requires `cargo xtask fetch-pdfium` (or `FERRIX_PDFIUM_DIR`). Without it, tests are skipped
//! locally and fail on CI.

#![allow(clippy::unwrap_used)]

mod fixtures;

use ferrix_engine::{DocSource, EngineError, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PixelRect, Point, Rotation, Size};
use fixtures::Page;

fn engine() -> Option<PdfiumEngine> {
    match PdfiumEngine::new() {
        Ok(engine) => Some(engine),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping: {e}");
            None
        }
        Err(e) => panic!("PDFium is required on CI: {e} (tried {:?})", e.tried),
    }
}

/// Opens a fixture with the given pages.
fn open(engine: &mut PdfiumEngine, pages: &[Page]) -> DocId {
    let bytes = fixtures::pdf("Fixture", pages);
    engine.open(DocSource::Bytes(bytes), None).unwrap().id
}

fn render(engine: &mut PdfiumEngine, doc: DocId, rotation: Rotation, region: PixelRect) -> Vec<u8> {
    let tile = engine
        .render_tile(&TileRequest {
            doc,
            page: PageIndex(0),
            scale: 1.0,
            rotation,
            region,
            flags: RenderFlags::default(),
        })
        .unwrap();
    assert_eq!(tile.region, region);
    tile.pixels
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn opens_document_and_reads_metadata() {
    let Some(mut engine) = engine() else { return };
    let bytes = fixtures::pdf("Quarterly Report", &[Page::letter(), Page::letter()]);
    let info = engine.open(DocSource::Bytes(bytes), None).unwrap();

    assert_eq!(info.page_count, 2);
    assert_eq!(info.pdf_version.as_deref(), Some("1.7"));
    assert_eq!(info.metadata.title.as_deref(), Some("Quarterly Report"));
    assert_eq!(info.metadata.producer.as_deref(), Some("ferrix-tests"));
    assert_eq!(info.metadata.author, None);
}

#[test]
fn page_info_applies_crop_box_and_rotation() {
    let Some(mut engine) = engine() else { return };
    let doc = open(
        &mut engine,
        &[
            Page::letter(),
            Page::letter().rotated(90),
            Page::letter().cropped([50.0, 100.0, 550.0, 700.0]),
        ],
    );

    let info = |engine: &mut PdfiumEngine, i| engine.page_info(doc, PageIndex(i)).unwrap();
    assert_eq!(info(&mut engine, 0).size, Size::new(612.0, 792.0));
    let rotated = info(&mut engine, 1);
    assert_eq!(
        (rotated.size, rotated.rotation),
        (Size::new(792.0, 612.0), Rotation::R90)
    );
    assert_eq!(info(&mut engine, 2).size, Size::new(500.0, 600.0));
}

#[test]
fn renders_tiles_at_the_requested_offset() {
    let Some(mut engine) = engine() else { return };
    let doc = open(&mut engine, &[Page::letter()]);

    // The red square sits at the bottom-left of the sheet: page space x 0..100, y 692..792.
    let full = render(
        &mut engine,
        doc,
        Rotation::R0,
        PixelRect::new(0, 0, 612, 792),
    );
    assert_eq!(pixel(&full, 612, 50, 742), RED);
    assert_eq!(pixel(&full, 612, 300, 300), WHITE);

    // A tile covering just the square is entirely red (sampling inside the anti-aliased edge).
    let tile = render(
        &mut engine,
        doc,
        Rotation::R0,
        PixelRect::new(0, 692, 100, 100),
    );
    for (x, y) in [(2, 2), (50, 50), (97, 97)] {
        assert_eq!(pixel(&tile, 100, x, y), RED, "({x}, {y})");
    }
}

#[test]
fn tile_regions_are_clamped_to_the_page() {
    let Some(mut engine) = engine() else { return };
    let doc = open(&mut engine, &[Page::letter()]);
    let tile = engine
        .render_tile(&TileRequest {
            doc,
            page: PageIndex(0),
            scale: 1.0,
            rotation: Rotation::R0,
            region: PixelRect::new(512, 768, 256, 256),
            flags: RenderFlags::default(),
        })
        .unwrap();
    assert_eq!(tile.region, PixelRect::new(512, 768, 100, 24));
    assert_eq!(tile.pixels.len(), 100 * 24 * 4);
}

#[test]
fn intrinsic_rotation_matches_page_transform() {
    let Some(mut engine) = engine() else { return };
    let doc = open(&mut engine, &[Page::letter().rotated(90)]);

    // Rotating 90° clockwise moves the sheet's bottom-left corner (the square) to the top-left.
    let full = render(
        &mut engine,
        doc,
        Rotation::R0,
        PixelRect::new(0, 0, 792, 612),
    );
    assert_eq!(pixel(&full, 792, 50, 50), RED);
    assert_eq!(pixel(&full, 792, 50, 562), WHITE);

    // Text bounds must land where the engine draws: baseline at PDF (72, 700) → page (700, 72),
    // with the glyph advancing down the page.
    let text = engine.text_page(doc, PageIndex(0)).unwrap();
    let b = text.chars.iter().find(|c| c.ch == 'H').unwrap().bounds;
    assert!(b.contains(Point::new(700.0, 73.0)), "{b:?}");
    assert!((b.y0 - 72.0).abs() < 1.0, "{b:?}");

    let dark_pixels = (b.y0 as u32..b.y1 as u32)
        .flat_map(|y| (b.x0 as u32..b.x1 as u32).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&full, 792, x, y)[0] < 128)
        .count();
    assert!(dark_pixels > 20, "no glyph drawn inside {b:?}");
}

#[test]
fn view_rotation_rotates_the_output() {
    let Some(mut engine) = engine() else { return };
    let doc = open(&mut engine, &[Page::letter()]);

    // Rotated 180°, the square moves from bottom-left to top-right.
    let full = render(
        &mut engine,
        doc,
        Rotation::R180,
        PixelRect::new(0, 0, 612, 792),
    );
    assert_eq!(pixel(&full, 612, 562, 50), RED);
    assert_eq!(pixel(&full, 612, 50, 742), WHITE);
}

#[test]
fn extracts_text_with_page_space_bounds() {
    let Some(mut engine) = engine() else { return };
    let doc = open(&mut engine, &[Page::letter()]);
    let text = engine.text_page(doc, PageIndex(0)).unwrap();

    assert_eq!(text.text().trim(), "Hello");
    let h = &text.chars[0];
    assert_eq!(h.ch, 'H');
    assert!((h.font_size - 24.0).abs() < 0.01);
    // Baseline at PDF y=700 is page y=92; the glyph sits above it.
    assert!((70.0..=72.5).contains(&h.bounds.x0), "{:?}", h.bounds);
    assert!(
        h.bounds.y0 < 92.0 && (88.0..=100.0).contains(&h.bounds.y1),
        "{:?}",
        h.bounds
    );
}

#[test]
fn reads_outline() {
    let Some(mut engine) = engine() else { return };
    let doc = open(
        &mut engine,
        &[Page::letter(), Page::letter(), Page::letter()],
    );
    let outline = engine.outline(doc).unwrap();

    let entries: Vec<_> = outline.iter().map(|i| (i.title.as_str(), i.page)).collect();
    assert_eq!(
        entries,
        [
            ("Page 1", Some(PageIndex(0))),
            ("Page 2", Some(PageIndex(1))),
            ("Page 3", Some(PageIndex(2))),
        ]
    );
}

#[test]
fn reports_errors_without_panicking() {
    let Some(mut engine) = engine() else { return };
    assert!(matches!(
        engine.open(DocSource::Bytes(b"not a pdf".to_vec()), None),
        Err(EngineError::InvalidDocument(_))
    ));
    assert!(matches!(
        engine.open(DocSource::Path("/definitely/missing.pdf".into()), None),
        Err(EngineError::Io(_))
    ));

    let doc = open(&mut engine, &[Page::letter()]);
    assert_eq!(
        engine.page_info(doc, PageIndex(1)),
        Err(EngineError::PageOutOfRange {
            page: PageIndex(1),
            count: 1
        })
    );
    assert_eq!(
        engine.page_info(DocId(999), PageIndex(0)),
        Err(EngineError::UnknownDocument(DocId(999)))
    );

    let bad = |scale, region| TileRequest {
        doc,
        page: PageIndex(0),
        scale,
        rotation: Rotation::R0,
        region,
        flags: RenderFlags::default(),
    };
    for request in [
        bad(0.0, PixelRect::new(0, 0, 10, 10)),
        bad(f32::NAN, PixelRect::new(0, 0, 10, 10)),
        bad(1.0, PixelRect::new(5000, 0, 10, 10)),
        bad(1e9, PixelRect::new(0, 0, 10, 10)),
    ] {
        assert!(
            matches!(
                engine.render_tile(&request),
                Err(EngineError::InvalidRequest(_))
            ),
            "{request:?}"
        );
    }

    engine.close(doc);
    assert_eq!(engine.outline(doc), Err(EngineError::UnknownDocument(doc)));
}
