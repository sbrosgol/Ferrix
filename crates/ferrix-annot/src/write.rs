//! Writing annotation changes into an incremental revision (ADR-0009 §4–6).

use std::collections::{HashMap, HashSet};

use ferrix_core::{
    AnnotId, AnnotKind, AnnotOrigin, AnnotSet, Annotation, PageEntry, PageList, ReplyTo,
    StampContent,
};
use ferrix_pdf::Revision;
use ferrix_types::PdfBox;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::{AnnotError, Appearance, appearance, read::read_page, unique_name};

/// What [`write_annotations`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteReport {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    /// New annotations left out of a tagged file's structure tree, because the tree isn't
    /// one Ferrix can safely extend.
    pub untagged: usize,
}

impl WriteReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether a document lets annotations be added (ISO 32000-2 §12.8.2.2, DocMDP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    Allowed,
    /// Signed; annotating is permitted, but validators will report changes after signing.
    Signed,
    /// Certified with no changes, or form filling only, allowed.
    Forbidden(String),
}

/// Checks the base document's certification and signatures.
pub fn annotation_permission(doc: &Document) -> Permission {
    let catalog = match doc.catalog() {
        Ok(catalog) => catalog,
        Err(_) => return Permission::Allowed,
    };
    let docmdp = catalog
        .get_deref(b"Perms", doc)
        .and_then(Object::as_dict)
        .and_then(|perms| perms.get_deref(b"DocMDP", doc))
        .and_then(Object::as_dict);
    if let Ok(sig) = docmdp {
        let p = sig
            .get_deref(b"Reference", doc)
            .and_then(Object::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| doc.dereference(r).ok()?.1.as_dict().ok())
            .find(|r| {
                r.get(b"TransformMethod")
                    .and_then(Object::as_name)
                    .is_ok_and(|m| m == b"DocMDP")
            })
            .and_then(|r| r.get_deref(b"TransformParams", doc).ok()?.as_dict().ok())
            .and_then(|params| params.get(b"P").ok()?.as_i64().ok())
            .unwrap_or(2);
        return match p {
            1 => Permission::Forbidden("the author certified it and allows no changes".into()),
            2 => Permission::Forbidden(
                "the author certified it and allows only filling in forms".into(),
            ),
            _ => Permission::Signed,
        };
    }
    // Reader-extended files (`/Perms /UR3`) lose their extra Reader features on any change,
    // which is worth the same warning as a signature.
    let reader_extended = catalog
        .get_deref(b"Perms", doc)
        .and_then(Object::as_dict)
        .is_ok_and(|perms| perms.has(b"UR3"));
    let signed = reader_extended
        || catalog
            .get_deref(b"AcroForm", doc)
            .and_then(Object::as_dict)
            .and_then(|form| form.get_deref(b"SigFlags", doc))
            .and_then(Object::as_i64)
            .is_ok_and(|flags| flags & 1 != 0);
    if signed {
        Permission::Signed
    } else {
        Permission::Allowed
    }
}

/// What a PDF/A claim requires of new appearance streams.
#[derive(Debug, Clone, Copy, Default)]
struct PdfaRules {
    /// `pdfaid:part` from the XMP metadata.
    part: Option<u8>,
    /// The output intent's profile is CMYK, so DeviceRGB may not be used (ISO 19005-2 §6.2.4.3).
    cmyk: bool,
}

fn pdfa_rules(doc: &Document) -> PdfaRules {
    let Ok(catalog) = doc.catalog() else {
        return PdfaRules::default();
    };
    let xmp = catalog
        .get_deref(b"Metadata", doc)
        .and_then(Object::as_stream)
        .ok()
        .and_then(|s| {
            s.get_plain_content()
                .ok()
                .or_else(|| Some(s.content.clone()))
        })
        .unwrap_or_default();
    let xmp = String::from_utf8_lossy(&xmp);
    // Either `pdfaid:part="2"` or `<pdfaid:part>2</pdfaid:part>`.
    let part = xmp.find("pdfaid:part").and_then(|at| {
        xmp[at + "pdfaid:part".len()..]
            .trim_start_matches(['=', '"', '\'', '>', ' '])
            .chars()
            .next()?
            .to_digit(10)
            .map(|d| d as u8)
    });
    let components: Vec<i64> = catalog
        .get_deref(b"OutputIntents", doc)
        .and_then(Object::as_array)
        .into_iter()
        .flatten()
        .filter_map(|intent| {
            let intent = doc.dereference(intent).ok()?.1.as_dict().ok()?;
            let profile = intent
                .get_deref(b"DestOutputProfile", doc)
                .ok()?
                .as_stream()
                .ok()?;
            profile.dict.get(b"N").ok()?.as_i64().ok()
        })
        .collect();
    PdfaRules {
        part,
        cmyk: part.is_some() && components.contains(&4) && !components.contains(&3),
    }
}

/// Writes the annotations of every page in `pages` that changed into `revision`.
///
/// `SourceId(n)` in the model is source `n` of the revision. For each page, the original
/// `/Annots` array is kept in order, minus deleted annotations and their popups, followed by new
/// annotations. Changed annotations keep their object number; only modelled keys are rewritten.
/// `now` is a PDF date (see [`crate::pdf_date`]).
pub fn write_annotations(
    revision: &mut Revision,
    pages: &PageList,
    annots: &AnnotSet,
    now: &str,
) -> Result<WriteReport, AnnotError> {
    write_page_annotations(revision, pages.entries(), annots, now)
}

/// [`write_annotations`] for some pages only, e.g. the document's own pages when building the
/// on-screen preview of a revision (ADR-0009 §7).
pub fn write_page_annotations(
    revision: &mut Revision,
    pages: &[PageEntry],
    annots: &AnnotSet,
    now: &str,
) -> Result<WriteReport, AnnotError> {
    let mut report = WriteReport::default();
    let pdfa = pdfa_rules(revision.source(0)?);
    let (mut tagger, untaggable) = match crate::tag::Tagger::load(revision.source(0)?) {
        None => (None, false),
        Some(Ok(tagger)) => (Some(tagger), false),
        Some(Err(())) => (None, true),
    };
    for entry in pages {
        let source = entry.source.0 as usize;
        let page_id = revision.page_id(source, entry.page.0)?;
        let doc = revision.source(source)?;
        let loaded = read_page(doc, page_id, entry.source)?;

        let mut kept: HashMap<ObjectId, &Annotation> = HashMap::new();
        let mut added = Vec::new();
        for (annot_id, annot) in annots.page(entry.key) {
            match annot.origin {
                AnnotOrigin::New => added.push((*annot_id, annot)),
                AnnotOrigin::Existing { source: s, object } => {
                    let id = (s == entry.source)
                        .then(|| revision.object_id(source, (object.number, object.generation)))
                        .flatten()
                        .filter(|id| loaded.iter().any(|(l, _)| l == id))
                        .ok_or_else(|| {
                            ferrix_pdf::PdfError::InvalidLayout(format!(
                                "annotation {} {} R is not on page {}",
                                object.number,
                                object.generation,
                                entry.page.number()
                            ))
                        })?;
                    kept.insert(id, annot);
                }
            }
        }
        // Only a page read from the file can have lost annotations (see `AnnotSet::is_loaded`).
        let removed: HashSet<ObjectId> = if annots.is_loaded(entry.key) {
            loaded
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| !kept.contains_key(id))
                .collect()
        } else {
            HashSet::new()
        };
        // Replies must stay on their parent's page (ISO 32000-2 §12.5.6.2): delete threads whole.
        if let Some(orphan) = kept.values().find_map(|a| {
            let ReplyTo::File(parent) = a.reply_to? else {
                return None;
            };
            let parent = revision.object_id(source, (parent.number, parent.generation))?;
            removed.contains(&parent).then_some(parent)
        }) {
            return Err(AnnotError::NotAllowed(format!(
                "a reply to the deleted annotation {} {} R would be left behind; delete the \
                 whole thread",
                orphan.0, orphan.1
            )));
        }
        let changed: Vec<(ObjectId, &Annotation, &Annotation)> = loaded
            .iter()
            .filter_map(|(id, original)| {
                let current = kept.get(id)?;
                let same = current.props == original.props && current.kind == original.kind;
                (!same).then_some((*id, *current, original))
            })
            .collect();
        if added.is_empty() && removed.is_empty() && changed.is_empty() {
            continue;
        }
        if source == 0
            && let Permission::Forbidden(why) = annotation_permission(doc)
        {
            return Err(AnnotError::NotAllowed(why));
        }
        if pdfa.part == Some(1)
            && added
                .iter()
                .map(|(_, a)| *a)
                .chain(changed.iter().map(|(_, a, _)| *a))
                .any(|a| {
                    // Highlights blend with Multiply, which PDF/A-1 forbids (ISO 19005-1 §6.4).
                    a.props.opacity < 1.0
                        || matches!(
                            a.kind,
                            AnnotKind::Markup {
                                style: ferrix_core::MarkupStyle::Highlight,
                                ..
                            }
                        )
                })
        {
            return Err(AnnotError::NotAllowed(
                "this is a PDF/A-1 archival file, which allows no transparency or highlights"
                    .into(),
            ));
        }

        // Gather what we need from the source before mutating the revision.
        let page = doc.get_dictionary(page_id)?;
        let items: Vec<Object> = page
            .get_deref(b"Annots", doc)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default();
        let popups = popups_of(doc, &items, &removed);
        if source == 0
            && let Some(tagger) = tagger.as_mut()
        {
            for id in &removed {
                if let Some(key) = doc
                    .get_dictionary(*id)
                    .ok()
                    .and_then(|d| d.get(b"StructParent").ok()?.as_i64().ok())
                {
                    tagger.untag(doc, key);
                }
            }
        }
        let originals: Vec<(ObjectId, Dictionary, &Annotation, bool)> = changed
            .iter()
            .map(|(id, current, original)| {
                let dict = doc.get_dictionary(*id).cloned()?;
                Ok((
                    *id,
                    dict,
                    *current,
                    current.props.contents != original.props.contents,
                ))
            })
            .collect::<Result<_, lopdf::Error>>()?;

        for (id, mut dict, annot, contents_changed) in originals {
            let ap = appearance(annot).ok_or_else(|| {
                AnnotError::NotAllowed(format!(
                    "{} annotations can't be edited",
                    annot.kind.subtype()
                ))
            })?;
            let ap_id = if kept_appearance(annot) {
                None
            } else {
                Some(annotation_stream(revision, annot, &ap, pdfa)?)
            };
            apply_model(&mut dict, annot, &ap, ap_id, now, contents_changed);
            revision.replace_object(id, Object::Dictionary(dict));
            report.changed += 1;
        }

        let mut out: Vec<Object> = items
            .into_iter()
            .filter(|item| {
                item.as_reference()
                    .map_or(true, |id| !removed.contains(&id) && !popups.contains(&id))
            })
            .collect();
        // Ids first, so a reply's /IRT can name a parent added in this same save.
        let reserved: HashMap<AnnotId, ObjectId> = added
            .iter()
            .map(|(annot_id, _)| (*annot_id, revision.reserve_id()))
            .collect();
        for (annot_id, annot) in added {
            if kept_appearance(annot) {
                return Err(AnnotError::NotAllowed(
                    "a stamp copied from a file can't be added as new".into(),
                ));
            }
            let ap = appearance(annot).ok_or_else(|| {
                AnnotError::NotAllowed(format!(
                    "{} annotations can't be created",
                    annot.kind.subtype()
                ))
            })?;
            let ap_id = annotation_stream(revision, annot, &ap, pdfa)?;
            let mut dict = Dictionary::new();
            dict.set("P", Object::Reference(page_id));
            dict.set("CreationDate", text(now));
            if annot.props.name.is_none() {
                dict.set("NM", text(&unique_name()));
            }
            apply_model(&mut dict, annot, &ap, Some(ap_id), now, true);
            if let Some(parent) = annot.reply_to {
                let parent = match parent {
                    ReplyTo::New(id) => reserved.get(&id).copied(),
                    ReplyTo::File(object) => {
                        revision.object_id(source, (object.number, object.generation))
                    }
                }
                .ok_or_else(|| {
                    AnnotError::NotAllowed("a reply's annotation is no longer on its page".into())
                })?;
                dict.set("IRT", Object::Reference(parent));
            }
            let object_id = reserved[&annot_id];
            if source == 0
                && let Some(tagger) = tagger.as_mut()
            {
                let element = revision.reserve_id();
                let alt = crate::tag::alt_text(annot);
                tagger.tag(
                    revision.source(0)?,
                    (page_id, entry.page.0),
                    object_id,
                    element,
                    &mut dict,
                    &alt,
                );
            } else if source == 0 && untaggable {
                report.untagged += 1;
            }
            revision.replace_object(object_id, Object::Dictionary(dict));
            out.push(Object::Reference(object_id));
            report.added += 1;
        }
        report.removed += removed.len();
        revision.set_page_annots(source, entry.page.0, out)?;
    }
    if let Some(tagger) = tagger {
        let (objects, pages) = tagger.finish(revision.source(0)?);
        for (id, dict) in objects {
            revision.replace_object(id, Object::Dictionary(dict));
        }
        for page in pages {
            revision.set_page_entry(0, page, "Tabs", Object::Name(b"S".to_vec()))?;
        }
    }
    Ok(report)
}

/// Popups that belong to removed annotations: referenced by their `/Popup`, or naming one as
/// `/Parent`.
fn popups_of(doc: &Document, items: &[Object], removed: &HashSet<ObjectId>) -> HashSet<ObjectId> {
    let mut popups = HashSet::new();
    for id in items.iter().filter_map(|o| o.as_reference().ok()) {
        let Ok(dict) = doc.get_dictionary(id) else {
            continue;
        };
        if removed.contains(&id) {
            if let Ok(popup) = dict.get(b"Popup").and_then(Object::as_reference) {
                popups.insert(popup);
            }
        } else if dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_ok_and(|s| s == b"Popup")
            && dict
                .get(b"Parent")
                .and_then(Object::as_reference)
                .is_ok_and(|parent| removed.contains(&parent))
        {
            popups.insert(id);
        }
    }
    popups
}

/// The appearance of `annot`: vector shapes, or for text boxes, text set by `ferrix-compose`.
fn annotation_stream(
    revision: &mut Revision,
    annot: &Annotation,
    ap: &Appearance,
    pdfa: PdfaRules,
) -> Result<ObjectId, AnnotError> {
    let r = ap.bbox;
    let (w, h) = (r.width().max(1.0), r.height().max(1.0));
    // In PDF/A files, ICC-based colour is valid whatever the output intent.
    let icc = pdfa.part.is_some();
    // krilla draws in its own [0 0 pw ph] page; the outer form places it (`cm`) in the box.
    let (page, pw, ph, cm) = match &annot.kind {
        AnnotKind::FreeText { size, color } => {
            let text = annot.props.contents.as_deref().unwrap_or("");
            let page = crate::free_text::render(text, *size, *color, w, h, icc)?;
            (page, w, h, [1.0, 1.0, r.left, r.bottom])
        }
        AnnotKind::Stamp {
            content: StampContent::Standard(name),
        } => {
            let (page, lw, lh) = crate::stamp::render_label(name, icc)?;
            let (nw, nh) = crate::stamp::standard_size(name);
            let (sx, sy) = (w / nw, h / nh);
            let (px, py) = crate::stamp::inset();
            (page, lw, lh, [sx, sy, r.left + px * sx, r.bottom + py * sy])
        }
        AnnotKind::Stamp {
            content: StampContent::Image(image),
        } => {
            let page = crate::stamp::render_image(image, icc)?;
            let (pw, ph) = (image.width as f32, image.height as f32);
            (page, pw, ph, [w / pw, h / ph, r.left, r.bottom])
        }
        _ => return Ok(appearance_stream(revision, ap, pdfa.cmyk)),
    };
    let inner = revision.import_page_as_form(&page, 0, [0.0, 0.0, pw, ph])?;
    let mut xobjects = Dictionary::new();
    xobjects.set("Tx", Object::Reference(inner));
    let mut resources = Dictionary::new();
    resources.set("XObject", Object::Dictionary(xobjects));
    let mut content = String::from("q ");
    if ap.opacity < 1.0 {
        let mut gs = Dictionary::new();
        gs.set("Type", Object::Name(b"ExtGState".to_vec()));
        gs.set("CA", Object::Real(ap.opacity));
        gs.set("ca", Object::Real(ap.opacity));
        let mut states = Dictionary::new();
        states.set("GS0", Object::Dictionary(gs));
        resources.set("ExtGState", Object::Dictionary(states));
        content.push_str("/GS0 gs ");
    }
    // A stamp's border is drawn under its label.
    content.push_str(&String::from_utf8_lossy(&ap.content(pdfa.cmyk)));
    content.push_str(&format!(
        " q {} 0 0 {} {} {} cm /Tx Do Q Q\n",
        cm[0], cm[1], cm[2], cm[3]
    ));
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set("BBox", rect(r));
    dict.set("Resources", Object::Dictionary(resources));
    Ok(revision.add_object(Object::Stream(Stream::new(dict, content.into_bytes()))))
}

/// Adds the form XObject for `/AP /N` and returns its id. Coordinates are in user space, so
/// `/BBox` equals `/Rect` and the matrix is the identity (ISO 32000-2 §12.5.5).
///
/// Below full opacity the shapes are drawn opaque in an inner form marked as a transparency
/// group, which the outer form paints through `/GS0`. Opacity then applies to the drawing as a
/// whole: a border over its fill, crossing ink strokes and arrowheads don't get darker where
/// they overlap, in any reader (§11.4.7).
fn appearance_stream(revision: &mut Revision, ap: &Appearance, cmyk: bool) -> ObjectId {
    let form = |content: Vec<u8>, resources: Dictionary, group: bool| {
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set("BBox", rect(ap.bbox));
        dict.set("Resources", Object::Dictionary(resources));
        if group {
            let mut g = Dictionary::new();
            g.set("Type", Object::Name(b"Group".to_vec()));
            g.set("S", Object::Name(b"Transparency".to_vec()));
            dict.set("Group", Object::Dictionary(g));
        }
        let mut stream = Stream::new(dict, content);
        // Compression only fails on filters we didn't ask for; an uncompressed stream is valid.
        let _ = stream.compress();
        Object::Stream(stream)
    };
    let shapes = ap.content(cmyk);
    if ap.multiply {
        // Multiply must blend with the page itself, so no isolating group here: the shapes
        // are drawn directly through a graphics state with the blend mode and opacity.
        let mut gs = Dictionary::new();
        gs.set("Type", Object::Name(b"ExtGState".to_vec()));
        gs.set("BM", Object::Name(b"Multiply".to_vec()));
        gs.set("CA", Object::Real(ap.opacity));
        gs.set("ca", Object::Real(ap.opacity));
        let mut states = Dictionary::new();
        states.set("GS0", Object::Dictionary(gs));
        let mut resources = Dictionary::new();
        resources.set("ExtGState", Object::Dictionary(states));
        let mut content = b"/GS0 gs\n".to_vec();
        content.extend(shapes);
        return revision.add_object(form(content, resources, false));
    }
    if !ap.needs_group() {
        return revision.add_object(form(shapes, Dictionary::new(), false));
    }
    let inner = revision.add_object(form(shapes, Dictionary::new(), true));
    let mut gs = Dictionary::new();
    gs.set("Type", Object::Name(b"ExtGState".to_vec()));
    gs.set("CA", Object::Real(ap.opacity));
    gs.set("ca", Object::Real(ap.opacity));
    let mut states = Dictionary::new();
    states.set("GS0", Object::Dictionary(gs));
    let mut xobjects = Dictionary::new();
    xobjects.set("Fm0", Object::Reference(inner));
    let mut resources = Dictionary::new();
    resources.set("ExtGState", Object::Dictionary(states));
    resources.set("XObject", Object::Dictionary(xobjects));
    revision.add_object(form(b"q /GS0 gs /Fm0 Do Q\n".to_vec(), resources, false))
}

/// Writes the modelled keys of `annot` into `dict`, leaving every other key as it was.
fn apply_model(
    dict: &mut Dictionary,
    annot: &Annotation,
    ap: &Appearance,
    ap_id: Option<ObjectId>,
    now: &str,
    contents_changed: bool,
) {
    let p = &annot.props;
    dict.set("Type", Object::Name(b"Annot".to_vec()));
    dict.set(
        "Subtype",
        Object::Name(annot.kind.subtype().as_bytes().to_vec()),
    );
    dict.set("Rect", rect(ap.bbox));
    set_or_remove(dict, "C", p.color.map(|c| color(c.r, c.g, c.b)));
    set_or_remove(dict, "IC", p.interior.map(|c| color(c.r, c.g, c.b)));
    let opacity = (p.opacity < 1.0).then_some(Object::Real(p.opacity));
    set_or_remove(dict, "CA", opacity.clone());
    // PDF 2.0 adds a separate fill opacity and blend mode (Table 166); stale values would
    // contradict our appearance for readers that rebuild it.
    set_or_remove(dict, "ca", opacity);
    dict.remove(b"BM");
    let mut bs = Dictionary::new();
    bs.set("Type", Object::Name(b"Border".to_vec()));
    bs.set("W", Object::Real(p.border_width));
    if p.dash.is_empty() {
        bs.set("S", Object::Name(b"S".to_vec()));
    } else {
        bs.set("S", Object::Name(b"D".to_vec()));
        bs.set(
            "D",
            Object::Array(p.dash.iter().map(|d| Object::Real(*d)).collect()),
        );
    }
    // Notes, replies and text markup have no border style (Tables 175 and 182 define none).
    if !matches!(
        annot.kind,
        AnnotKind::Note { .. }
            | AnnotKind::Reply { .. }
            | AnnotKind::Stamp { .. }
            | AnnotKind::Markup { .. }
    ) {
        dict.set("BS", Object::Dictionary(bs));
        // `/BS` takes precedence, but a stale `/Border` would mislead readers that only know it.
        dict.remove(b"Border");
    }
    set_or_remove(dict, "T", p.author.as_deref().map(text));
    set_or_remove(dict, "Contents", p.contents.as_deref().map(text));
    if let Some(name) = &p.name {
        dict.set("NM", text(name));
    }
    dict.set("M", text(now));
    dict.set("F", Object::Integer(i64::from(p.flags.0)));
    if contents_changed {
        // Rich text must never disagree with /Contents (ADR-0009 §4). Rebuilding Acrobat's
        // XFA-flavoured XHTML faithfully isn't worth it; without `/RC` readers show `/Contents`.
        dict.remove(b"RC");
    }

    match &annot.kind {
        AnnotKind::Note { icon, open } => {
            dict.set("Name", Object::Name(icon.name().as_bytes().to_vec()));
            dict.set("Open", Object::Boolean(*open));
        }
        AnnotKind::Stamp { content } => match content {
            // `#` marks our own look so Acrobat keeps our appearance rather than its own
            // artwork; /Subj is what its comment list shows.
            StampContent::Standard(name) => {
                dict.set("Name", Object::Name(format!("#{name}").into_bytes()));
                if let Some((label, _)) = crate::stamp::standard(name) {
                    dict.set("Subj", Object::string_literal(title_case(label)));
                }
                dict.remove(b"IT");
            }
            // Name, /IT and /Subj stay as the file has them.
            StampContent::Kept => {}
            StampContent::Image(_) => {
                dict.remove(b"Name");
                dict.set("IT", Object::Name(b"StampImage".to_vec()));
                dict.set("Subj", Object::string_literal("Image"));
            }
        },
        AnnotKind::Ink { strokes } => {
            let list = strokes
                .iter()
                .map(|s| {
                    Object::Array(
                        s.iter()
                            .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                            .collect(),
                    )
                })
                .collect();
            dict.set("InkList", Object::Array(list));
        }
        AnnotKind::Line { from, to, ends } => {
            dict.set(
                "L",
                Object::Array(vec![
                    Object::Real(from.x),
                    Object::Real(from.y),
                    Object::Real(to.x),
                    Object::Real(to.y),
                ]),
            );
            dict.set(
                "LE",
                Object::Array(
                    ends.iter()
                        .map(|e| Object::Name(e.name().as_bytes().to_vec()))
                        .collect(),
                ),
            );
        }
        AnnotKind::FreeText { size, color } => {
            let hex = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            // For readers that rebuild the look: Helvetica has Liberation Sans's metrics.
            dict.set(
                "DA",
                text(&format!(
                    "/Helv {size} Tf {} {} {} rg",
                    num(color.r),
                    num(color.g),
                    num(color.b)
                )),
            );
            dict.set(
                "DS",
                text(&format!(
                    "font: Helvetica,sans-serif {size}pt; text-align:left; color:#{:02X}{:02X}{:02X}",
                    hex(color.r),
                    hex(color.g),
                    hex(color.b)
                )),
            );
            dict.set("Q", Object::Integer(0));
            // A stale rich-text body would bring back old text or style when Acrobat edits it.
            dict.remove(b"RC");
            dict.remove(b"IC");
        }
        AnnotKind::Markup { quads, .. } => {
            dict.set(
                "QuadPoints",
                Object::Array(
                    quads
                        .iter()
                        .flatten()
                        .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                        .collect(),
                ),
            );
        }
        AnnotKind::Square
        | AnnotKind::Circle
        | AnnotKind::Reply { .. }
        | AnnotKind::Other { .. } => {}
    }
    // A kept appearance stays, with the file's /AP; ours replaces any earlier one, and these
    // keys describe the old geometry or states.
    let Some(ap_id) = ap_id else {
        return;
    };
    for key in [b"RD".as_slice(), b"AS"] {
        dict.remove(key);
    }
    let mut ap_dict = Dictionary::new();
    ap_dict.set("N", Object::Reference(ap_id));
    dict.set("AP", Object::Dictionary(ap_dict));
}

/// Annotations whose appearance comes from the file unchanged.
fn kept_appearance(annot: &Annotation) -> bool {
    matches!(
        annot.kind,
        AnnotKind::Stamp {
            content: StampContent::Kept
        }
    )
}

fn set_or_remove(dict: &mut Dictionary, key: &str, value: Option<Object>) {
    match value {
        Some(value) => dict.set(key, value),
        None => {
            dict.remove(key.as_bytes());
        }
    }
}

fn rect(b: PdfBox) -> Object {
    Object::Array(vec![
        Object::Real(b.left),
        Object::Real(b.bottom),
        Object::Real(b.right),
        Object::Real(b.top),
    ])
}

fn color(r: f32, g: f32, b: f32) -> Object {
    Object::Array(vec![Object::Real(r), Object::Real(g), Object::Real(b)])
}

/// A number as PDF syntax: at most three decimals, no exponent.
fn num(v: f32) -> String {
    let s = format!("{:.3}", if v.is_finite() { v } else { 0.0 });
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A PDF text string: PDFDocEncoding when ASCII, otherwise UTF-16BE with a byte-order mark.
fn text(s: &str) -> Object {
    lopdf::text_string(s)
}

/// Re-reads written `bytes` and checks that every page holds the annotations the model says,
/// by subtype (ADR-0009 §6). Order is not compared: new annotations are appended to `/Annots`
/// whatever their position in the model. Pages whose annotations were never loaded are skipped.
pub fn verify(
    bytes: &[u8],
    password: Option<&str>,
    pages: &PageList,
    annots: &AnnotSet,
) -> Result<(), AnnotError> {
    let doc = ferrix_pdf::load(ferrix_pdf::Source { bytes, password })?;
    let written: Vec<ObjectId> = doc.get_pages().into_values().collect();
    if written.len() != pages.len() {
        return Err(AnnotError::Verify(format!(
            "{} pages written, {} expected",
            written.len(),
            pages.len()
        )));
    }
    for (n, (entry, page_id)) in pages.entries().iter().zip(written).enumerate() {
        if !annots.is_loaded(entry.key) {
            continue;
        }
        let mut found: Vec<String> = read_page(&doc, page_id, entry.source)?
            .into_iter()
            .map(|(_, a)| a.kind.subtype().to_owned())
            .collect();
        let mut expected: Vec<String> = annots
            .page(entry.key)
            .iter()
            .map(|(_, a)| a.kind.subtype().to_owned())
            .collect();
        found.sort();
        expected.sort();
        if found != expected {
            return Err(AnnotError::Verify(format!(
                "page {} has {found:?}, expected {expected:?}",
                n + 1
            )));
        }
    }
    Ok(())
}

/// "NOT APPROVED" → "Not Approved".
fn title_case(label: &str) -> String {
    label
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| {
                f.to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}
