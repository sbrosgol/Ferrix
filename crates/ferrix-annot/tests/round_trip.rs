//! Annotations written into real PDF files and read back with lopdf and PDFium (ADR-0009 phase 1).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use ferrix_annot::{AnnotError, read_annotations, write_annotations};
use ferrix_core::{
    AnnotEdit, AnnotKind, AnnotSet, Annotation, DocEdit, DocState, LineEnd, NoteIcon, PageList,
    Rgb, SourceId,
};
use ferrix_engine::{DocSource, PdfEngine, RenderFlags, Secret, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_pdf::{LayoutPage, Revision, Source};
use ferrix_types::{PageIndex, PdfBox, PdfPoint, PixelRect, Rotation};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions, Stream,
    dictionary,
};

const NOW: &str = "D:20261121120000Z";

/// Two pages. Page 1 carries annotations made by "another app": a blue square with a vendor key
/// and a popup, a free-text box, a link and a note with rich text. Page 2 is rotated 90° and its
/// crop box does not start at the origin.
fn fixture() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let page1 = doc.new_object_id();
    let page2 = doc.new_object_id();
    let content = doc.add_object(Stream::new(Dictionary::new(), b"0 0 1 rg".to_vec()));

    let square = doc.new_object_id();
    let popup = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Popup", "Parent" => square, "P" => page1,
        "Rect" => vec![300.into(), 600.into(), 450.into(), 700.into()],
    });
    doc.objects.insert(
        square,
        Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "P" => page1,
            "Rect" => vec![50.into(), 50.into(), 150.into(), 120.into()],
            "C" => vec![0.into(), 0.into(), 1.into()],
            "Contents" => Object::string_literal("from another app"),
            "Popup" => popup,
            "FooVendor" => Object::string_literal("keep me"),
        }),
    );
    let free_text = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText", "P" => page1,
        "Rect" => vec![50.into(), 300.into(), 250.into(), 340.into()],
        "DA" => Object::string_literal("/Helv 12 Tf 0 g"),
        "Contents" => Object::string_literal("free text"),
    });
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link",
        "Rect" => vec![50.into(), 400.into(), 150.into(), 420.into()],
    });
    let note = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "P" => page1, "Name" => "Comment",
        "Rect" => vec![500.into(), 700.into(), 520.into(), 720.into()],
        "Contents" => Object::string_literal("Old"),
        "RC" => Object::string_literal("<body><p><b>Old</b></p></body>"),
    });
    let annots = vec![square, popup, free_text, link, note]
        .into_iter()
        .map(Object::Reference)
        .collect::<Vec<_>>();
    doc.objects.insert(
        page1,
        Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content, "Annots" => annots,
        }),
    );
    doc.objects.insert(
        page2,
        Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Rotate" => 90,
            "MediaBox" => vec![0.into(), (-100).into(), 600.into(), 700.into()],
            "CropBox" => vec![30.into(), (-40).into(), 530.into(), 660.into()],
        }),
    );
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page1.into(), page2.into()], "Count" => 2,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    // Encryption keys derive from the file identifier, so real files always have one.
    let id = Object::String(
        b"ferrix-fixture-id".to_vec(),
        lopdf::StringFormat::Hexadecimal,
    );
    doc.trailer.set("ID", vec![id.clone(), id]);
    save(&mut doc)
}

fn save(doc: &mut Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// The editing state for `bytes`, as an app would build it on open.
fn open_state(bytes: &[u8], password: Option<&str>) -> DocState {
    let doc = ferrix_pdf::load(Source { bytes, password }).unwrap();
    let per_page = read_annotations(&doc, SourceId::BASE);
    let pages = PageList::identity(per_page.len() as u32);
    let mut annots = AnnotSet::new();
    for (entry, list) in pages.entries().iter().zip(per_page) {
        annots.insert_loaded(entry.key, list);
    }
    DocState::new(pages, annots)
}

fn add(state: &mut DocState, page: usize, annot: Annotation) {
    let key = state.pages.entries()[page].key;
    let index = state.annots.page(key).len();
    let id = state.annots.new_id();
    state
        .apply(&DocEdit::Annot(AnnotEdit::Add {
            page: key,
            index,
            id,
            annot: Box::new(annot),
        }))
        .unwrap();
}

/// Saves the annotations of `state` as an incremental update of `bytes`.
fn save_state(bytes: &[u8], password: Option<&str>, state: &DocState) -> Vec<u8> {
    let mut revision = Revision::new(Source { bytes, password }, &[]).unwrap();
    let report = write_annotations(&mut revision, &state.pages, &state.annots, NOW).unwrap();
    assert!(!report.is_empty());
    let out = revision.finish().unwrap();
    ferrix_annot::verify(&out, password, &state.pages, &state.annots).unwrap();
    out
}

fn red_square(rect: PdfBox) -> Annotation {
    let mut a = Annotation::new(AnnotKind::Square, rect);
    a.props.color = Some(Rgb::new(1.0, 0.0, 0.0));
    a.props.interior = Some(Rgb::new(1.0, 0.0, 0.0));
    a.props.border_width = 2.0;
    a
}

fn pt(x: f32, y: f32) -> PdfPoint {
    PdfPoint::new(x, y)
}

fn pdfium() -> Option<PdfiumEngine> {
    match PdfiumEngine::new() {
        Ok(engine) => Some(engine),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("skipping PDFium check: {e}");
            None
        }
        Err(e) => panic!("PDFium is required on CI: {e}"),
    }
}

/// `(subtype, rect)` per page as PDFium sees them, without popups, links and widgets.
fn pdfium_annotations(
    engine: &mut PdfiumEngine,
    bytes: &[u8],
    password: Option<&str>,
) -> Vec<Vec<(String, PdfBox)>> {
    let doc = engine
        .open(
            DocSource::Bytes(bytes.to_vec()),
            password.map(Secret::new).as_ref(),
        )
        .unwrap();
    (0..doc.page_count)
        .map(|i| {
            engine
                .annotations(doc.id, PageIndex(i))
                .unwrap()
                .into_iter()
                .filter(|a| !["Popup", "Link", "Widget"].contains(&a.subtype.as_str()))
                .map(|a| (a.subtype, a.rect))
                .collect()
        })
        .collect()
}

fn close(a: PdfBox, b: PdfBox) -> bool {
    [
        a.left - b.left,
        a.bottom - b.bottom,
        a.right - b.right,
        a.top - b.top,
    ]
    .iter()
    .all(|d| d.abs() < 0.01)
}

#[test]
fn reads_what_ferrix_models_and_skips_the_rest() {
    let state = open_state(&fixture(), None);
    let page1 = state.pages.entries()[0].key;
    let kinds: Vec<String> = state
        .annots
        .page(page1)
        .iter()
        .map(|(_, a)| a.kind.subtype().to_owned())
        .collect();
    assert_eq!(
        kinds,
        ["Square", "FreeText", "Text"],
        "popup and link not loaded"
    );
    let (_, square) = &state.annots.page(page1)[0];
    assert_eq!(square.props.color, Some(Rgb::new(0.0, 0.0, 1.0)));
    assert_eq!(square.props.contents.as_deref(), Some("from another app"));
    assert!(square.is_editable());
    // A plain text box (no callout, fill or styled rich text) is editable since phase 3; its
    // size and colour come from /DA.
    assert_eq!(
        state.annots.page(page1)[1].1.kind,
        AnnotKind::FreeText {
            size: 12.0,
            color: Rgb::BLACK
        }
    );
    let (_, note) = &state.annots.page(page1)[2];
    assert_eq!(
        note.kind,
        AnnotKind::Note {
            icon: NoteIcon::Comment,
            open: false
        }
    );
}

#[test]
fn adds_every_phase_one_type_and_reads_them_back() {
    let base = fixture();
    let mut state = open_state(&base, None);
    add(
        &mut state,
        0,
        red_square(PdfBox::from_corners(pt(100.0, 600.0), pt(200.0, 700.0))),
    );
    let mut circle = Annotation::new(
        AnnotKind::Circle,
        PdfBox::from_corners(pt(250.0, 600.0), pt(350.0, 650.0)),
    );
    circle.props.opacity = 0.5;
    add(&mut state, 0, circle);
    add(
        &mut state,
        0,
        Annotation::new(
            AnnotKind::Line {
                from: pt(100.0, 500.0),
                to: pt(300.0, 450.0),
                ends: [LineEnd::None, LineEnd::ClosedArrow],
            },
            PdfBox::default(),
        ),
    );
    let mut ink = Annotation::new(
        AnnotKind::Ink {
            strokes: vec![vec![pt(100.0, 200.0), pt(150.0, 260.0), pt(200.0, 210.0)]],
        },
        PdfBox::default(),
    );
    ink.props.border_width = 3.0;
    ink.props.contents = Some("Unterschrift – ünïcödé".into());
    add(&mut state, 0, ink);
    // On the rotated, offset page.
    let mut note = Annotation::new(
        AnnotKind::Note {
            icon: NoteIcon::Comment,
            open: false,
        },
        ferrix_annot::note_rect(pt(100.0, 300.0)),
    );
    note.props.contents = Some("On the rotated page".into());
    add(&mut state, 1, note);

    let out = save_state(&base, None, &state);
    assert!(
        out.starts_with(&base),
        "incremental: original bytes untouched"
    );

    let reread = open_state(&out, None);
    let summary = |s: &DocState, page: usize| -> Vec<(String, Option<String>)> {
        s.annots
            .page(s.pages.entries()[page].key)
            .iter()
            .map(|(_, a)| (a.kind.subtype().to_owned(), a.props.contents.clone()))
            .collect()
    };
    assert_eq!(summary(&reread, 0), summary(&state, 0));
    assert_eq!(summary(&reread, 1), summary(&state, 1));
    let page1 = reread.pages.entries()[0].key;
    for (_, a) in reread.annots.page(page1).iter().skip(3) {
        assert_eq!(a.props.modified.as_deref(), Some(NOW));
        assert!(
            a.props
                .name
                .as_deref()
                .is_some_and(|n| n.starts_with("ferrix-"))
        );
        assert!(a.props.flags.has(ferrix_core::AnnotFlags::PRINT));
    }
    // Geometry survives exactly (written as reals).
    let line = reread.annots.page(page1)[5].1.kind.clone();
    assert_eq!(
        line,
        AnnotKind::Line {
            from: pt(100.0, 500.0),
            to: pt(300.0, 450.0),
            ends: [LineEnd::None, LineEnd::ClosedArrow],
        }
    );

    // Every new annotation has an appearance stream.
    let doc = Document::load_mem(&out).unwrap();
    for page in doc.get_pages().into_values() {
        let annots = doc
            .get_dictionary(page)
            .unwrap()
            .get_deref(b"Annots", &doc)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default();
        for item in annots {
            let dict = doc.get_dictionary(item.as_reference().unwrap()).unwrap();
            if dict.get(b"NM").is_ok() {
                let ap = dict.get(b"AP").unwrap().as_dict().unwrap();
                let n = doc.get_object(ap.get(b"N").unwrap().as_reference().unwrap());
                assert!(matches!(n, Ok(Object::Stream(_))), "/AP /N is a form");
            }
        }
    }

    let Some(mut engine) = pdfium() else { return };
    let seen = pdfium_annotations(&mut engine, &out, None);
    let written = |s: &DocState, page: usize| -> Vec<String> {
        s.annots
            .page(s.pages.entries()[page].key)
            .iter()
            .map(|(_, a)| a.kind.subtype().to_owned())
            .collect()
    };
    assert_eq!(seen.len(), 2);
    for (page, seen_page) in seen.iter().enumerate() {
        let subtypes: Vec<String> = seen_page.iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(subtypes, written(&reread, page), "page {}", page + 1);
        for ((_, pdfium_rect), (_, a)) in seen_page
            .iter()
            .zip(reread.annots.page(reread.pages.entries()[page].key))
        {
            assert!(
                close(*pdfium_rect, a.props.rect),
                "{pdfium_rect:?} vs {:?}",
                a.props.rect
            );
        }
    }

    // The red square really draws: the pixel at its centre is red.
    let doc = engine.open(DocSource::Bytes(out), None).unwrap();
    let tile = engine
        .render_tile(&TileRequest {
            doc: doc.id,
            page: PageIndex(0),
            scale: 1.0,
            rotation: Rotation::R0,
            // User space (150, 650) is page space (150, 792 - 650).
            region: PixelRect::new(146, 138, 8, 8),
            flags: RenderFlags::default(),
        })
        .unwrap();
    let centre = (4 * 8 + 4) * 4;
    assert_eq!(
        &tile.pixels[centre..centre + 3],
        &[255, 0, 0],
        "square centre pixel"
    );
}

#[test]
fn edits_and_deletes_foreign_annotations_without_losing_their_data() {
    let base = fixture();
    let mut state = open_state(&base, None);
    let page1 = state.pages.entries()[0].key;
    let list = state.annots.page(page1).to_vec();
    let (square_id, _) = list[0];
    let (note_id, mut note) = list[2].clone();

    // Delete the square (its popup goes too) and edit the note's text.
    state
        .apply(&DocEdit::Annot(AnnotEdit::Remove { id: square_id }))
        .unwrap();
    note.props.contents = Some("New & improved".into());
    state
        .apply(&DocEdit::Annot(AnnotEdit::Set {
            id: note_id,
            annot: Box::new(note),
        }))
        .unwrap();
    let out = save_state(&base, None, &state);
    assert!(out.starts_with(&base));

    let original = Document::load_mem(&base).unwrap();
    let doc = Document::load_mem(&out).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let annots: Vec<_> = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_reference().unwrap())
        .collect();
    let subtype = |id| {
        doc.get_dictionary(id)
            .unwrap()
            .get(b"Subtype")
            .unwrap()
            .as_name()
            .unwrap()
            .to_vec()
    };
    let subtypes: Vec<Vec<u8>> = annots.iter().map(|id| subtype(*id)).collect();
    assert_eq!(
        subtypes,
        [b"FreeText".to_vec(), b"Link".to_vec(), b"Text".to_vec()],
        "square and its popup are gone, the rest keep their order"
    );

    // The edited note keeps its object number and unknown keys; stale /RC is dropped.
    let note_obj = annots[2];
    let before = original.get_dictionary(note_obj).unwrap();
    let after = doc.get_dictionary(note_obj).unwrap();
    assert_eq!(after.get(b"Name").unwrap(), before.get(b"Name").unwrap());
    assert!(
        after.get(b"RC").is_err(),
        "rich text would disagree with /Contents"
    );
    assert_eq!(
        lopdf::decode_text_string(after.get(b"Contents").unwrap()).unwrap(),
        "New & improved"
    );
    assert!(after.get(b"AP").is_ok());

    // Untouched annotations are not rewritten: their objects aren't in the update section.
    let update = &out[base.len()..];
    let free_text_obj = annots[0];
    let marker = format!("\n{} {} obj", free_text_obj.0, free_text_obj.1);
    assert!(
        !String::from_utf8_lossy(update).contains(&marker),
        "free text object was rewritten"
    );
    assert_eq!(
        doc.get_dictionary(free_text_obj).unwrap(),
        original.get_dictionary(free_text_obj).unwrap()
    );
}

/// An annotation with values Ferrix normalizes on load (an all-zero dash, which the spec
/// forbids) must not count as changed when something else on the page is edited.
#[test]
fn normalized_foreign_values_do_not_count_as_changes() {
    let mut doc = Document::load_mem(&fixture()).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let odd = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square",
        "Rect" => vec![10.into(), 10.into(), 40.into(), 40.into()],
        "BS" => dictionary! { "W" => 1, "S" => "D", "D" => vec![0.into(), 0.into()] },
    });
    let annots = doc
        .get_dictionary_mut(page)
        .unwrap()
        .get_mut(b"Annots")
        .unwrap()
        .as_array_mut()
        .unwrap();
    annots.push(Object::Reference(odd));
    let base = save(&mut doc);

    let mut state = open_state(&base, None);
    add(
        &mut state,
        0,
        red_square(PdfBox::from_corners(pt(100.0, 600.0), pt(200.0, 700.0))),
    );
    let mut revision = Revision::new(Source::new(&base), &[]).unwrap();
    let report = write_annotations(&mut revision, &state.pages, &state.annots, NOW).unwrap();
    assert_eq!(report.changed, 0, "{report:?}");
    assert_eq!(report.added, 1);
}

#[test]
fn nothing_changed_writes_nothing() {
    let base = fixture();
    let state = open_state(&base, None);
    let mut revision = Revision::new(Source::new(&base), &[]).unwrap();
    let report = write_annotations(&mut revision, &state.pages, &state.annots, NOW).unwrap();
    assert!(report.is_empty());
}

fn encrypted_fixture(aes: bool) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture()).unwrap();
    let state = if aes {
        let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
        EncryptionState::try_from(EncryptionVersion::V4 {
            document: &doc,
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "owner",
            user_password: "user",
            permissions: Permissions::all(),
        })
    } else {
        EncryptionState::try_from(EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: "user",
            key_length: 128,
            permissions: Permissions::all(),
        })
    }
    .unwrap();
    doc.encrypt(&state).unwrap();
    save(&mut doc)
}

#[test]
fn encrypted_documents_stay_encrypted() {
    for aes in [false, true] {
        let base = encrypted_fixture(aes);
        let password = Some("user");
        let mut state = open_state(&base, password);
        let mut square = red_square(PdfBox::from_corners(pt(100.0, 600.0), pt(200.0, 700.0)));
        square.props.contents = Some("SECRET-COMMENT-TEXT".into());
        add(&mut state, 0, square);
        let out = save_state(&base, password, &state);

        assert!(
            !String::from_utf8_lossy(&out).contains("SECRET-COMMENT-TEXT"),
            "aes={aes}: comment written in plain text"
        );
        let reread = open_state(&out, password);
        let page1 = reread.pages.entries()[0].key;
        let last = &reread.annots.page(page1).last().unwrap().1;
        assert_eq!(last.props.contents.as_deref(), Some("SECRET-COMMENT-TEXT"));

        let Some(mut engine) = pdfium() else { continue };
        let seen = pdfium_annotations(&mut engine, &out, password);
        assert_eq!(seen[0].last().unwrap().0, "Square", "aes={aes}");
    }
}

fn certified_fixture(p: i64) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture()).unwrap();
    let sig = doc.add_object(dictionary! {
        "Type" => "Sig",
        "Reference" => vec![Object::Dictionary(dictionary! {
            "Type" => "SigRef", "TransformMethod" => "DocMDP",
            "TransformParams" => dictionary! { "Type" => "TransformParams", "P" => p, "V" => "1.2" },
        })],
    });
    let catalog_id = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = doc.get_dictionary_mut(catalog_id).unwrap();
    catalog.set("Perms", dictionary! { "DocMDP" => sig });
    save(&mut doc)
}

#[test]
fn certified_documents_refuse_annotations_unless_permitted() {
    for (p, allowed) in [(1, false), (2, false), (3, true)] {
        let base = certified_fixture(p);
        let mut state = open_state(&base, None);
        add(
            &mut state,
            0,
            red_square(PdfBox::from_corners(pt(10.0, 10.0), pt(20.0, 20.0))),
        );
        let mut revision = Revision::new(Source::new(&base), &[]).unwrap();
        let result = write_annotations(&mut revision, &state.pages, &state.annots, NOW);
        assert_eq!(result.is_ok(), allowed, "P={p}: {result:?}");
        if !allowed {
            assert!(matches!(result, Err(AnnotError::NotAllowed(_))));
        }
    }
}

#[test]
fn page_layout_and_annotations_land_in_one_revision_including_imports() {
    let base = fixture();
    let other = fixture();
    // The document is: page 2 of base (rotated), then page 1 of the imported copy.
    let mut state = open_state(&base, None);
    let imported = ferrix_pdf::load(Source::new(&other)).unwrap();
    let import_annots = read_annotations(&imported, SourceId(1));
    let entry = state.pages.new_entries(SourceId(1), [PageIndex(0)])[0];
    state
        .apply(&DocEdit::Pages(ferrix_core::Edit::Insert {
            pages: vec![(2, entry)],
        }))
        .unwrap();
    state
        .annots
        .insert_loaded(entry.key, import_annots[0].clone());
    let first = state.pages.entries()[0].key;
    state
        .apply(&DocEdit::Pages(ferrix_core::Edit::Remove {
            keys: vec![first],
        }))
        .unwrap();

    // Recolour the imported square and add a line to the rotated page.
    let (square_id, mut square) = state.annots.page(entry.key)[0].clone();
    square.props.color = Some(Rgb::new(0.0, 0.5, 0.0));
    state
        .apply(&DocEdit::Annot(AnnotEdit::Set {
            id: square_id,
            annot: Box::new(square),
        }))
        .unwrap();
    add(
        &mut state,
        0,
        Annotation::new(
            AnnotKind::Line {
                from: pt(100.0, 100.0),
                to: pt(200.0, 200.0),
                ends: [LineEnd::OpenArrow, LineEnd::None],
            },
            PdfBox::default(),
        ),
    );

    let layout: Vec<LayoutPage> = state
        .pages
        .entries()
        .iter()
        .map(|e| LayoutPage {
            source: e.source.0 as usize,
            page: e.page.0,
            rotate_by: e.rotation,
        })
        .collect();
    let mut revision = Revision::new(Source::new(&base), &[Source::new(&other)]).unwrap();
    revision.set_layout(layout).unwrap();
    let report = write_annotations(&mut revision, &state.pages, &state.annots, NOW).unwrap();
    assert_eq!(
        report,
        ferrix_annot::WriteReport {
            added: 1,
            changed: 1,
            removed: 0
        }
    );
    let out = revision.finish().unwrap();

    let reread = open_state(&out, None);
    let kinds = |page: usize| -> Vec<String> {
        reread
            .annots
            .page(reread.pages.entries()[page].key)
            .iter()
            .map(|(_, a)| a.kind.subtype().to_owned())
            .collect()
    };
    assert_eq!(kinds(0), ["Line"]);
    assert_eq!(kinds(1), ["Square", "FreeText", "Text"]);
    let imported_square = &reread.annots.page(reread.pages.entries()[1].key)[0].1;
    assert_eq!(imported_square.props.color, Some(Rgb::new(0.0, 0.5, 0.0)));
    assert_eq!(
        imported_square.props.contents.as_deref(),
        Some("from another app")
    );

    let Some(mut engine) = pdfium() else { return };
    let seen = pdfium_annotations(&mut engine, &out, None);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].len(), 1);
    assert_eq!(seen[1].len(), 3);
}

/// Page 1 of `fixture()` plus a reply to the note and the given extra annotation dictionaries.
fn fixture_with(extra: impl FnOnce(&mut Document, lopdf::ObjectId) -> Vec<Object>) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture()).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let added = extra(&mut doc, page);
    doc.get_dictionary_mut(page)
        .unwrap()
        .get_mut(b"Annots")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .extend(added);
    save(&mut doc)
}

fn note_object(doc: &Document, page: lopdf::ObjectId) -> lopdf::ObjectId {
    let annots = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap();
    annots
        .iter()
        .map(|o| o.as_reference().unwrap())
        .find(|id| {
            doc.get_dictionary(*id)
                .unwrap()
                .get(b"Subtype")
                .unwrap()
                .as_name()
                .unwrap()
                == b"Text"
        })
        .unwrap()
}

#[test]
fn deleting_a_note_takes_its_replies_and_orphans_are_refused() {
    let base = fixture_with(|doc, page| {
        let note = note_object(doc, page);
        let reply = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "P" => page, "IRT" => note,
            "Rect" => vec![500.into(), 700.into(), 520.into(), 720.into()],
            "Contents" => Object::string_literal("I agree"),
        });
        let reply2 = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "P" => page, "IRT" => reply,
            "Rect" => vec![500.into(), 700.into(), 520.into(), 720.into()],
            "Contents" => Object::string_literal("Me too"),
        });
        vec![reply.into(), reply2.into()]
    });
    let state = open_state(&base, None);
    let page1 = state.pages.entries()[0].key;
    let note_id = state
        .annots
        .page(page1)
        .iter()
        .find(|(_, a)| matches!(a.kind, AnnotKind::Note { .. }))
        .unwrap()
        .0;
    let thread = state.annots.thread(note_id);
    assert_eq!(thread.len(), 3, "note, reply and reply to the reply");

    // Deleting only the note would orphan the replies: refused.
    let mut alone = state.clone();
    alone
        .apply(&DocEdit::Annot(AnnotEdit::Remove { id: note_id }))
        .unwrap();
    let mut revision = Revision::new(Source::new(&base), &[]).unwrap();
    assert!(matches!(
        write_annotations(&mut revision, &alone.pages, &alone.annots, NOW),
        Err(AnnotError::NotAllowed(_))
    ));

    // Deleting the thread works and leaves no /IRT pointing at a missing annotation.
    let mut whole = state.clone();
    let edits = thread
        .into_iter()
        .map(|id| DocEdit::Annot(AnnotEdit::Remove { id }))
        .collect();
    whole.apply(&DocEdit::Batch(edits)).unwrap();
    let out = save_state(&base, None, &whole);
    let reread = open_state(&out, None);
    let kinds: Vec<&str> = reread
        .annots
        .page(reread.pages.entries()[0].key)
        .iter()
        .map(|(_, a)| a.kind.subtype())
        .collect();
    assert_eq!(kinds, ["Square", "FreeText"]);
}

#[test]
fn shapes_ferrix_cannot_redraw_stay_display_only_and_rd_is_honoured() {
    let base = fixture_with(|doc, _| {
        let leader = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Line", "LL" => 10,
            "Rect" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "L" => vec![10.into(), 10.into(), 90.into(), 10.into()],
        });
        let cloudy = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square",
            "Rect" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "BE" => dictionary! { "S" => "C", "I" => 1 },
        });
        let inset = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Circle",
            "Rect" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "RD" => vec![5.into(), 5.into(), 5.into(), 5.into()],
        });
        vec![leader.into(), cloudy.into(), inset.into()]
    });
    let state = open_state(&base, None);
    let list = state.annots.page(state.pages.entries()[0].key);
    let tail: Vec<&Annotation> = list.iter().rev().take(3).map(|(_, a)| a).collect();
    assert!(!tail[2].is_editable(), "line with leader lines");
    assert!(!tail[1].is_editable(), "cloudy square");
    assert_eq!(
        tail[0].props.rect,
        PdfBox::from_corners(pt(5.0, 5.0), pt(95.0, 95.0)),
        "/RD insets the shape"
    );
}

fn pdfa_fixture(part: u8, cmyk: bool) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture()).unwrap();
    let xmp = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="{part}" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta>"#
    );
    let metadata = doc.add_object(Stream::new(
        dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
        xmp.into_bytes(),
    ));
    let profile = doc.add_object(Stream::new(
        dictionary! { "N" => if cmyk { 4 } else { 3 } },
        b"not a real ICC profile".to_vec(),
    ));
    let intent = dictionary! {
        "Type" => "OutputIntent", "S" => "GTS_PDFA1",
        "OutputConditionIdentifier" => Object::string_literal("test"),
        "DestOutputProfile" => profile,
    };
    let catalog_id = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = doc.get_dictionary_mut(catalog_id).unwrap();
    catalog.set("Metadata", metadata);
    catalog.set("OutputIntents", vec![Object::Dictionary(intent)]);
    save(&mut doc)
}

/// The content of the appearance (and of its inner group form, if any) of the last annotation
/// on page 1.
fn last_appearance(bytes: &[u8]) -> (Dictionary, String) {
    let doc = Document::load_mem(bytes).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let last = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_reference()
        .unwrap();
    let ap = doc
        .get_dictionary(last)
        .unwrap()
        .get(b"AP")
        .unwrap()
        .as_dict()
        .unwrap();
    let n = doc
        .get_object(ap.get(b"N").unwrap().as_reference().unwrap())
        .unwrap()
        .as_stream()
        .unwrap();
    let mut content = String::from_utf8_lossy(&n.decompressed_content().unwrap()).into_owned();
    if let Ok(inner) = n
        .dict
        .get(b"Resources")
        .and_then(Object::as_dict)
        .and_then(|r| r.get(b"XObject"))
        .and_then(Object::as_dict)
        .and_then(|x| x.get(b"Fm0"))
        .and_then(Object::as_reference)
    {
        let inner = doc.get_object(inner).unwrap().as_stream().unwrap();
        assert!(
            inner.dict.get(b"Group").is_ok(),
            "inner form is a transparency group"
        );
        content.push_str(&String::from_utf8_lossy(
            &inner.decompressed_content().unwrap(),
        ));
    }
    (n.dict.clone(), content)
}

#[test]
fn pdfa_files_get_compliant_appearances() {
    // PDF/A-1 forbids transparency.
    let base = pdfa_fixture(1, false);
    let mut state = open_state(&base, None);
    let mut faint = red_square(PdfBox::from_corners(pt(10.0, 10.0), pt(50.0, 50.0)));
    faint.props.opacity = 0.5;
    add(&mut state, 0, faint);
    let mut revision = Revision::new(Source::new(&base), &[]).unwrap();
    assert!(matches!(
        write_annotations(&mut revision, &state.pages, &state.annots, NOW),
        Err(AnnotError::NotAllowed(_))
    ));

    // A CMYK output intent: appearances use DeviceCMYK, not DeviceRGB.
    let base = pdfa_fixture(2, true);
    let mut state = open_state(&base, None);
    add(
        &mut state,
        0,
        red_square(PdfBox::from_corners(pt(10.0, 10.0), pt(50.0, 50.0))),
    );
    let (_, content) = last_appearance(&save_state(&base, None, &state));
    assert!(
        content.contains(" k") && content.contains(" K"),
        "{content}"
    );
    assert!(
        !content.contains("rg") && !content.contains("RG"),
        "{content}"
    );

    // An RGB intent keeps RGB.
    let base = pdfa_fixture(2, false);
    let mut state = open_state(&base, None);
    add(
        &mut state,
        0,
        red_square(PdfBox::from_corners(pt(10.0, 10.0), pt(50.0, 50.0))),
    );
    let (_, content) = last_appearance(&save_state(&base, None, &state));
    assert!(content.contains("rg"), "{content}");
}

#[test]
fn translucent_shapes_are_painted_as_one_group() {
    let base = fixture();
    let mut state = open_state(&base, None);
    let mut faint = red_square(PdfBox::from_corners(pt(100.0, 600.0), pt(200.0, 700.0)));
    faint.props.opacity = 0.5;
    faint.props.border_width = 20.0;
    faint.props.color = Some(Rgb::new(0.0, 0.0, 1.0));
    add(&mut state, 0, faint);
    let out = save_state(&base, None, &state);
    let (outer, content) = last_appearance(&out);
    assert!(content.starts_with("q /GS0 gs /Fm0 Do Q"), "{content}");
    assert!(
        outer.get(b"Group").is_err(),
        "only the inner form is the group"
    );

    // Where the border overlaps the fill, the colour is the border's at 50%, not darker.
    let Some(mut engine) = pdfium() else { return };
    let doc = engine.open(DocSource::Bytes(out), None).unwrap();
    // User (105, 650) is inside the border's inner half, over the fill. Page y = 792 - 650.
    let tile = engine
        .render_tile(&TileRequest {
            doc: doc.id,
            page: PageIndex(0),
            scale: 1.0,
            rotation: Rotation::R0,
            region: PixelRect::new(101, 138, 8, 8),
            flags: RenderFlags::default(),
        })
        .unwrap();
    let at = (4 * 8 + 4) * 4;
    let px = &tile.pixels[at..at + 3];
    assert!(px[0] > 110 && px[0] < 145 && px[2] > 110, "{px:?}");
}

#[test]
fn text_markup_round_trips_and_highlights_multiply() {
    use ferrix_core::MarkupStyle;
    let base = fixture();
    let mut state = open_state(&base, None);
    // One quad per style, Acrobat order: top-left, top-right, bottom-left, bottom-right.
    let quad = |y: f32| {
        [
            pt(100.0, y + 12.0),
            pt(300.0, y + 12.0),
            pt(100.0, y),
            pt(300.0, y),
        ]
    };
    for (i, style) in MarkupStyle::ALL.into_iter().enumerate() {
        let mut a = Annotation::new(
            AnnotKind::Markup {
                style,
                quads: vec![quad(500.0 + 30.0 * i as f32)],
            },
            PdfBox::default(),
        );
        a.props.color = Some(Rgb::new(1.0, 0.85, 0.0));
        add(&mut state, 0, a);
    }
    let out = save_state(&base, None, &state);
    let reread = open_state(&out, None);
    let page = reread.pages.entries()[0].key;
    let marks: Vec<&Annotation> = reread
        .annots
        .page(page)
        .iter()
        .map(|(_, a)| a)
        .filter(|a| matches!(a.kind, AnnotKind::Markup { .. }))
        .collect();
    assert_eq!(marks.len(), 4);
    let AnnotKind::Markup { style, quads } = &marks[0].kind else {
        unreachable!()
    };
    assert_eq!(
        (*style, quads.as_slice()),
        (MarkupStyle::Highlight, &[quad(500.0)][..])
    );

    let Some(mut engine) = pdfium() else { return };
    let seen = pdfium_annotations(&mut engine, &out, None);
    for subtype in ["Highlight", "Underline", "StrikeOut", "Squiggly"] {
        assert!(
            seen[0].iter().any(|(s, _)| s == subtype),
            "PDFium sees {subtype}"
        );
    }
    // The fixture's page content fills everything with blue (0 0 1 rg but draws nothing), so
    // over white paper a multiplied yellow highlight is yellow: red and green high, blue low.
    let doc = engine.open(DocSource::Bytes(out), None).unwrap();
    let tile = engine
        .render_tile(&TileRequest {
            doc: doc.id,
            page: PageIndex(0),
            scale: 1.0,
            rotation: Rotation::R0,
            // User (200, 506) → page (200, 792 - 506).
            region: PixelRect::new(196, 282, 8, 8),
            flags: RenderFlags::default(),
        })
        .unwrap();
    let at = (4 * 8 + 4) * 4;
    let px = &tile.pixels[at..at + 3];
    assert!(px[0] > 230 && px[1] > 200 && px[2] < 40, "{px:?}");
}

#[test]
fn quad_points_in_specification_order_are_reordered() {
    let base = fixture_with(|doc, page| {
        // Counter-clockwise from bottom-left: BL, BR, TR, TL.
        let quads: Vec<Object> = [100, 500, 300, 500, 300, 512, 100, 512]
            .iter()
            .map(|v| Object::Integer(*v))
            .collect();
        let h = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Highlight", "P" => page,
            "Rect" => vec![100.into(), 500.into(), 300.into(), 512.into()],
            "QuadPoints" => quads,
        });
        vec![h.into()]
    });
    let state = open_state(&base, None);
    let page = state.pages.entries()[0].key;
    let (_, h) = state.annots.page(page).last().unwrap();
    let AnnotKind::Markup { quads, .. } = &h.kind else {
        panic!("not markup: {:?}", h.kind)
    };
    assert_eq!(
        quads[0],
        [
            pt(100.0, 512.0),
            pt(300.0, 512.0),
            pt(100.0, 500.0),
            pt(300.0, 500.0)
        ]
    );
}

#[test]
fn text_boxes_round_trip_with_an_embedded_font_and_render() {
    let base = fixture();
    let mut state = open_state(&base, None);
    let text = "Hello, Ferrix — привет";
    let (w, h) = ferrix_annot::free_text::measure(text, 18.0, None);
    let mut box_ = Annotation::new(
        AnnotKind::FreeText {
            size: 18.0,
            color: Rgb::new(0.1, 0.1, 0.6),
        },
        PdfBox::from_corners(pt(100.0, 700.0 - h), pt(100.0 + w, 700.0)),
    );
    box_.props.color = None;
    box_.props.border_width = 0.0;
    box_.props.contents = Some(text.into());
    add(&mut state, 0, box_);
    let out = save_state(&base, None, &state);

    let reread = open_state(&out, None);
    let page = reread.pages.entries()[0].key;
    let (_, b) = reread.annots.page(page).last().unwrap();
    assert_eq!(
        b.kind,
        AnnotKind::FreeText {
            size: 18.0,
            color: Rgb::new(0.1, 0.1, 0.6)
        },
        "size and colour come back from /DA"
    );
    assert_eq!(b.props.contents.as_deref(), Some(text));

    // The appearance carries its own embedded font (no reliance on the reader's Helvetica).
    let doc = Document::load_mem(&out).unwrap();
    let embedded = doc
        .objects
        .values()
        .any(|o| o.as_dict().ok().is_some_and(|d| d.has(b"FontFile2")));
    assert!(embedded, "a font subset is embedded");

    let Some(mut engine) = pdfium() else { return };
    let seen = pdfium_annotations(&mut engine, &out, None);
    assert!(seen[0].iter().any(|(s, _)| s == "FreeText"));
    // Some dark text pixels inside the box.
    let doc = engine.open(DocSource::Bytes(out), None).unwrap();
    let top = (792.0 - 700.0) as u32;
    let tile = engine
        .render_tile(&TileRequest {
            doc: doc.id,
            page: PageIndex(0),
            scale: 1.0,
            rotation: Rotation::R0,
            region: PixelRect::new(100, top, w as u32, h as u32),
            flags: RenderFlags::default(),
        })
        .unwrap();
    let dark = tile
        .pixels
        .chunks(4)
        .filter(|p| p[0] < 120 && p[2] > 60)
        .count();
    assert!(dark > 20, "text drawn: {dark} dark pixels");
}

/// Text boxes in PDF/A files are set in ICC-based colour, valid under any output intent.
#[test]
fn text_boxes_in_cmyk_pdfa_files_use_icc_colour() {
    let base = pdfa_fixture(2, true);
    let mut state = open_state(&base, None);
    let (w, h) = ferrix_annot::free_text::measure("Archived", 12.0, None);
    let mut b = Annotation::new(
        AnnotKind::FreeText {
            size: 12.0,
            color: Rgb::BLACK,
        },
        PdfBox::from_corners(pt(50.0, 700.0 - h), pt(50.0 + w, 700.0)),
    );
    b.props.color = None;
    b.props.border_width = 0.0;
    b.props.contents = Some("Archived".into());
    add(&mut state, 0, b);
    let out = save_state(&base, None, &state);
    let doc = Document::load_mem(&out).unwrap();
    let icc = doc.objects.values().any(|o| match o {
        Object::Array(a) => a
            .first()
            .and_then(|n| n.as_name().ok())
            .is_some_and(|n| n == b"ICCBased"),
        _ => false,
    }) || String::from_utf8_lossy(&out).contains("/ICCBased");
    assert!(icc, "the text box uses an ICC-based colour space");
}

/// A reply can name a parent added in the same save: ids are reserved before dictionaries are
/// built, so /IRT points at the new note's object.
#[test]
fn replies_link_to_new_and_existing_parents() {
    use ferrix_core::{ReplyTo, ReviewState};
    let base = fixture_with(|doc, page| {
        let note = note_object(doc, page);
        let state = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "P" => page, "IRT" => note,
            "Rect" => vec![500.into(), 700.into(), 520.into(), 720.into()],
            "State" => Object::string_literal("Accepted"), "StateModel" => "Review",
            "T" => Object::string_literal("Ben"),
        });
        vec![state.into()]
    });
    let mut state = open_state(&base, None);
    let page = state.pages.entries()[0].key;

    // The review state reads as a read-only reply of the existing note.
    let list = state.annots.page(page).to_vec();
    let (note_id, note) = list
        .iter()
        .find(|(_, a)| matches!(a.kind, AnnotKind::Note { .. }))
        .cloned()
        .unwrap();
    let review = list
        .iter()
        .find_map(|(_, a)| match &a.kind {
            AnnotKind::Reply { state: Some(s) } => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        review,
        ReviewState {
            model: "Review".into(),
            state: "Accepted".into()
        }
    );
    assert_eq!(state.annots.replies(note_id).len(), 1);

    // A new note, a reply to it, and a reply to the existing note, in one save.
    let new_note = state.annots.new_id();
    let mut n = Annotation::new(
        AnnotKind::Note {
            icon: NoteIcon::Comment,
            open: false,
        },
        ferrix_annot::note_rect(pt(100.0, 700.0)),
    );
    n.props.contents = Some("Parent".into());
    state
        .apply(&DocEdit::Annot(AnnotEdit::Add {
            page,
            index: state.annots.page(page).len(),
            id: new_note,
            annot: Box::new(n.clone()),
        }))
        .unwrap();
    let reply = |to: ReplyTo, text: &str, rect: PdfBox| {
        let mut r = Annotation::new(AnnotKind::Reply { state: None }, rect);
        r.reply_to = Some(to);
        r.props.contents = Some(text.into());
        r.props.author = Some("Ana".into());
        r
    };
    add(
        &mut state,
        0,
        reply(ReplyTo::New(new_note), "Child of new", n.props.rect),
    );
    let ferrix_core::AnnotOrigin::Existing { object, .. } = note.origin else {
        panic!("fixture note is in the file")
    };
    add(
        &mut state,
        0,
        reply(ReplyTo::File(object), "Child of existing", note.props.rect),
    );
    let out = save_state(&base, None, &state);

    let doc = Document::load_mem(&out).unwrap();
    let page_id = *doc.get_pages().get(&1).unwrap();
    let annots: Vec<lopdf::ObjectId> = doc
        .get_dictionary(page_id)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_reference().unwrap())
        .collect();
    let by_contents = |text: &str| {
        annots
            .iter()
            .copied()
            .find(|id| {
                doc.get_dictionary(*id)
                    .unwrap()
                    .get(b"Contents")
                    .ok()
                    .and_then(|c| lopdf::decode_text_string(c).ok())
                    .as_deref()
                    == Some(text)
            })
            .unwrap()
    };
    let parent = by_contents("Parent");
    let child = doc.get_dictionary(by_contents("Child of new")).unwrap();
    assert_eq!(child.get(b"IRT").unwrap().as_reference().unwrap(), parent);
    assert!(child.get(b"BS").is_err(), "replies have no border style");
    let other = doc
        .get_dictionary(by_contents("Child of existing"))
        .unwrap();
    assert_eq!(
        other.get(b"IRT").unwrap().as_reference().unwrap(),
        (object.number, object.generation)
    );

    let reread = open_state(&out, None);
    let page = reread.pages.entries()[0].key;
    let replies = reread
        .annots
        .page(page)
        .iter()
        .filter(|(_, a)| matches!(a.kind, AnnotKind::Reply { state: None }))
        .count();
    assert_eq!(replies, 2);
}
