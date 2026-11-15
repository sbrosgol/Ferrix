//! Page-layout writing, checked structurally with lopdf and visually with PDFium.

#![allow(clippy::unwrap_used)]

use ferrix_engine::{DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_pdf::{LayoutPage, PdfError, Source, write_page_layout};
use ferrix_types::{PageIndex, Rotation};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

/// A document whose pages say "<prefix> 1", "<prefix> 2", …. Font, MediaBox and Rotate live on
/// the root Pages node and the pages sit under an intermediate node, so every page depends on
/// inheritance.
fn fixture(prefix: &str, pages: u32, root_rotate: i64) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let root_id = doc.new_object_id();
    let middle_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });

    let mut kids = Vec::new();
    for n in 1..=pages {
        let text = format!("BT /F1 24 Tf 72 700 Td ({prefix} {n}) Tj ET");
        let content = doc.add_object(Stream::new(Dictionary::new(), text.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => middle_id, "Contents" => content,
        });
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(
        middle_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Parent" => root_id, "Kids" => kids, "Count" => i64::from(pages),
        }),
    );
    doc.objects.insert(
        root_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(middle_id)],
            "Count" => i64::from(pages),
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Rotate" => root_rotate,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_id });
    doc.trailer.set("Root", catalog);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

fn page(source: usize, page: u32) -> LayoutPage {
    LayoutPage {
        source,
        page,
        rotate_by: Rotation::R0,
    }
}

/// Page texts and `/Rotate` values as lopdf reads them.
fn structure(bytes: &[u8]) -> Vec<(String, i64)> {
    let doc = Document::load_mem(bytes).unwrap();
    doc.get_pages()
        .into_values()
        .map(|id| {
            let content = String::from_utf8_lossy(&doc.get_page_content(id)).into_owned();
            let label = content
                .split(['(', ')'])
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let page = doc.get_dictionary(id).unwrap();
            assert!(
                page.has(b"Resources"),
                "{label} lost its inherited resources"
            );
            assert!(page.has(b"MediaBox"), "{label} lost its inherited MediaBox");
            let rotate = page.get(b"Rotate").unwrap().as_i64().unwrap();
            (label, rotate)
        })
        .collect()
}

/// Page texts as PDFium extracts them, or `None` when PDFium isn't installed locally.
fn rendered_text(bytes: &[u8]) -> Option<Vec<String>> {
    let mut engine = match PdfiumEngine::new() {
        Ok(engine) => engine,
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping PDFium check: {e}");
            return None;
        }
        Err(e) => panic!("PDFium is required on CI: {e}"),
    };
    let doc = engine.open(DocSource::Bytes(bytes.to_vec()), None).unwrap();
    Some(
        (0..doc.page_count)
            .map(|i| {
                engine
                    .text_page(doc.id, PageIndex(i))
                    .unwrap()
                    .text()
                    .trim()
                    .to_owned()
            })
            .collect(),
    )
}

#[test]
fn reorders_and_deletes_as_an_incremental_update() {
    let base = fixture("Page", 4, 0);
    let out = write_page_layout(
        Source::new(&base),
        &[],
        &[page(0, 3), page(0, 0), page(0, 2)],
    )
    .unwrap();

    assert!(
        out.starts_with(&base),
        "original bytes must be preserved verbatim"
    );
    let pages = structure(&out);
    assert_eq!(
        pages,
        [
            ("Page 4".into(), 0),
            ("Page 1".into(), 0),
            ("Page 3".into(), 0)
        ]
    );
    if let Some(text) = rendered_text(&out) {
        assert_eq!(text, ["Page 4", "Page 1", "Page 3"]);
    }
}

#[test]
fn rotation_adds_to_inherited_rotation() {
    let base = fixture("Page", 2, 90);
    let layout = [
        LayoutPage {
            source: 0,
            page: 0,
            rotate_by: Rotation::R90,
        },
        LayoutPage {
            source: 0,
            page: 1,
            rotate_by: Rotation::R270,
        },
    ];
    let out = write_page_layout(Source::new(&base), &[], &layout).unwrap();
    let rotations: Vec<i64> = structure(&out).into_iter().map(|(_, r)| r).collect();
    assert_eq!(rotations, [180, 0]);
}

#[test]
fn imports_pages_from_other_documents() {
    let base = fixture("Base", 2, 0);
    let other = fixture("Other", 3, 0);
    let layout = [page(0, 0), page(1, 2), page(1, 0), page(0, 1)];
    let out = write_page_layout(Source::new(&base), &[Source::new(&other)], &layout).unwrap();

    let labels: Vec<String> = structure(&out).into_iter().map(|(l, _)| l).collect();
    assert_eq!(labels, ["Base 1", "Other 3", "Other 1", "Base 2"]);
    if let Some(text) = rendered_text(&out) {
        assert_eq!(text, ["Base 1", "Other 3", "Other 1", "Base 2"]);
    }
}

#[test]
fn saving_twice_chains_incremental_updates() {
    let base = fixture("Page", 3, 0);
    let first = write_page_layout(
        Source::new(&base),
        &[],
        &[page(0, 2), page(0, 1), page(0, 0)],
    )
    .unwrap();
    let second = write_page_layout(Source::new(&first), &[], &[page(0, 2), page(0, 0)]).unwrap();

    assert!(second.starts_with(&first));
    let labels: Vec<String> = structure(&second).into_iter().map(|(l, _)| l).collect();
    assert_eq!(labels, ["Page 1", "Page 3"]);
}

#[test]
fn rejects_invalid_layouts() {
    let base = fixture("Page", 2, 0);
    let invalid = |layout: &[LayoutPage]| write_page_layout(Source::new(&base), &[], layout);

    assert!(matches!(invalid(&[]), Err(PdfError::InvalidLayout(_))));
    assert!(matches!(
        invalid(&[page(0, 0), page(0, 0)]),
        Err(PdfError::InvalidLayout(_))
    ));
    assert!(matches!(
        invalid(&[page(0, 5)]),
        Err(PdfError::InvalidLayout(_))
    ));
    assert!(matches!(
        invalid(&[page(1, 0)]),
        Err(PdfError::InvalidLayout(_))
    ));
    assert!(matches!(
        write_page_layout(Source::new(b"not a pdf"), &[], &[page(0, 0)]),
        Err(PdfError::Parse(_))
    ));
}
