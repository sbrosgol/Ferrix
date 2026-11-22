//! `ferrix stress FILE`: runs one document through everything the app does with a file, for the
//! soak test (`cargo xtask soak`). Each file runs in its own process, so a crash or hang in
//! PDFium or the parser is caught and attributed to the file that caused it.
//!
//! A document the engine or parser refuses is fine (that's what malformed files should get);
//! only a panic, a crash or a hang is a failure. The result is one JSON line on stdout.

use std::time::Instant;

use anyhow::Result;
use ferrix_core::{AnnotEdit, AnnotSet, Annotation, DocEdit, DocState, PageList, SourceId};
use ferrix_engine::{DocSource, PdfEngine, RenderFlags, Secret, TileRequest};
use ferrix_types::{PageIndex, PdfBox, PixelRect, Rotation};
use serde_json::{Value, json};

/// Pages rendered as thumbnails and read for text, links and annotations.
const MAX_PAGES: u32 = 300;
/// Pages also rendered at 100 %, and at 400 % in tiles.
const DETAIL_PAGES: u32 = 10;
const ZOOM_PAGES: u32 = 3;

/// What one stage did: done, or refused with a message. Panics are not caught; they are the
/// failures the soak looks for.
fn stage(
    stages: &mut serde_json::Map<String, Value>,
    name: &str,
    run: impl FnOnce() -> Result<Value>,
) {
    let start = Instant::now();
    let result = run();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let entry = match result {
        Ok(detail) => json!({ "ok": true, "ms": ms, "detail": detail }),
        Err(e) => json!({ "ok": false, "ms": ms, "error": format!("{e:#}") }),
    };
    stages.insert(name.to_owned(), entry);
}

pub(crate) fn run(
    engine: &mut impl PdfEngine,
    file: &std::path::Path,
    password: Option<&str>,
) -> Result<()> {
    let mut stages = serde_json::Map::new();
    let secret = password.map(Secret::new);
    let bytes = std::fs::read(file)?;

    let mut info = None;
    stage(&mut stages, "open", || {
        let opened = engine.open(DocSource::Bytes(bytes.clone()), secret.as_ref())?;
        let detail = json!({ "pages": opened.page_count, "encrypted": opened.encrypted });
        info = Some(opened);
        Ok(detail)
    });

    if let Some(info) = &info {
        let doc = info.id;
        let pages = info.page_count;
        stage(&mut stages, "measure", || {
            for i in 0..pages.min(10_000) {
                engine.page_info(doc, PageIndex(i))?;
            }
            Ok(json!(pages.min(10_000)))
        });
        stage(&mut stages, "thumbnails", || {
            let mut failed = 0;
            for i in 0..pages.min(MAX_PAGES) {
                if render(engine, doc, i, 0.25, None).is_err() {
                    failed += 1;
                }
            }
            Ok(json!({ "pages": pages.min(MAX_PAGES), "failed": failed }))
        });
        stage(&mut stages, "pages", || {
            for i in 0..pages.min(DETAIL_PAGES) {
                render(engine, doc, i, 96.0 / 72.0 * 2.0, None)?;
            }
            Ok(json!(pages.min(DETAIL_PAGES)))
        });
        stage(&mut stages, "zoomed_tiles", || {
            for i in 0..pages.min(ZOOM_PAGES) {
                // Four 512 px tiles near the middle of the page at 400 % on a 2× screen.
                let scale = 96.0 / 72.0 * 4.0 * 2.0;
                let size = engine.page_info(doc, PageIndex(i))?.size;
                let (w, h) = size.to_pixels(scale);
                for (dx, dy) in [(0, 0), (512, 0), (0, 512), (512, 512)] {
                    let (x, y) = (
                        (w / 2).saturating_sub(512) + dx,
                        (h / 2).saturating_sub(512) + dy,
                    );
                    // Small pages are only a tile or two across, even at 400 %.
                    if x >= w || y >= h {
                        continue;
                    }
                    let region = PixelRect::new(x, y, 512.min(w - x), 512.min(h - y));
                    render(engine, doc, i, scale, Some(region))?;
                }
            }
            Ok(json!(pages.min(ZOOM_PAGES)))
        });
        stage(&mut stages, "text_links_annots", || {
            let (mut chars, mut links, mut annots) = (0, 0, 0);
            for i in 0..pages.min(MAX_PAGES) {
                chars += engine
                    .text_page(doc, PageIndex(i))
                    .map_or(0, |t| t.chars.len());
                links += engine.links(doc, PageIndex(i)).map_or(0, |l| l.len());
                annots += engine.annotations(doc, PageIndex(i)).map_or(0, |a| a.len());
            }
            Ok(json!({ "chars": chars, "links": links, "annotations": annots }))
        });
        stage(&mut stages, "outline", || {
            let outline = engine.outline(doc)?;
            Ok(json!(outline.len()))
        });
        engine.close(doc);
    }

    // The object layer, as the app reads a file for annotations and writes a save.
    stage(&mut stages, "parse", || {
        let source = ferrix_pdf::Source {
            bytes: &bytes,
            password,
        };
        let pdf = ferrix_pdf::load(source)?;
        let annots = ferrix_annot::read_annotations(&pdf, SourceId::BASE);
        let transforms = ferrix_pdf::page_transforms(&pdf);
        let digest = ferrix_pdf::content_digest(source).is_ok();
        Ok(json!({
            "pages": annots.len(),
            "annotations": annots.iter().map(Vec::len).sum::<usize>(),
            "transforms": transforms.len(),
            "digest": digest,
        }))
    });
    if info.as_ref().is_some_and(|i| !i.encrypted) {
        stage(&mut stages, "save_annotation", || {
            save_round_trip(engine, &bytes, password)
        });
    }

    let line = json!({ "file": file.display().to_string(), "stages": stages });
    println!("{line}");
    Ok(())
}

fn render(
    engine: &mut impl PdfEngine,
    doc: ferrix_types::DocId,
    page: u32,
    scale: f32,
    region: Option<PixelRect>,
) -> Result<()> {
    let size = engine.page_info(doc, PageIndex(page))?.size;
    let (w, h) = size.to_pixels(scale);
    // Never ask for more than the viewer would: one whole page up to 3072 px, else a tile.
    let region = region.unwrap_or_else(|| PixelRect::new(0, 0, w.min(3072), h.min(3072)));
    engine.render_tile(&TileRequest {
        doc,
        page: PageIndex(page),
        scale,
        rotation: Rotation::R0,
        region,
        flags: RenderFlags::default(),
    })?;
    Ok(())
}

/// Adds a square to page 1 and saves, as the app would, then checks the result reads back.
fn save_round_trip(
    engine: &mut impl PdfEngine,
    bytes: &[u8],
    password: Option<&str>,
) -> Result<Value> {
    let source = ferrix_pdf::Source { bytes, password };
    let pdf = ferrix_pdf::load(source)?;
    let per_page = ferrix_annot::read_annotations(&pdf, SourceId::BASE);
    let pages = PageList::identity(per_page.len() as u32);
    let mut annots = AnnotSet::new();
    for (entry, list) in pages.entries().iter().zip(per_page) {
        annots.insert_loaded(entry.key, list);
    }
    let mut state = DocState::new(pages, annots);
    let Some(first) = state.pages.get(0).map(|e| e.key) else {
        return Ok(json!("no pages"));
    };
    let id = state.annots.new_id();
    let index = state.annots.page(first).len();
    let square = Annotation::new(
        ferrix_core::AnnotKind::Square,
        PdfBox {
            left: 72.0,
            bottom: 72.0,
            right: 144.0,
            top: 144.0,
        },
    );
    state.apply(&DocEdit::Annot(AnnotEdit::Add {
        page: first,
        index,
        id,
        annot: Box::new(square),
    }))?;
    let mut revision = ferrix_pdf::Revision::new(source, &[])?;
    let now = ferrix_annot::pdf_date(std::time::SystemTime::now());
    ferrix_annot::write_annotations(&mut revision, &state.pages, &state.annots, &now)?;
    let out = revision.finish()?;
    ferrix_annot::verify(&out, password, &state.pages, &state.annots)?;
    let secret = password.map(Secret::new);
    let check = engine.open(DocSource::Bytes(out.clone()), secret.as_ref())?;
    let ok = check.page_count as usize == state.pages.len();
    engine.close(check.id);
    anyhow::ensure!(ok, "the saved copy has {} pages", check.page_count);
    Ok(json!({ "bytes": out.len() }))
}
