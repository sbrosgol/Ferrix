//! Reading annotations from a parsed PDF into the Ferrix model.
//!
//! Reading never fails on a malformed annotation: anything Ferrix cannot model faithfully
//! becomes [`AnnotKind::Other`], which is shown but never rewritten.

use ferrix_core::{
    AnnotFlags, AnnotKind, AnnotOrigin, AnnotProps, Annotation, LineEnd, MAX_STROKE_POINTS,
    MAX_STROKES, MAX_TEXT_BYTES, MarkupStyle, NoteIcon, ObjectRef, ReplyTo, Rgb, SourceId,
};
use ferrix_types::{PdfBox, PdfPoint};
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::AnnotError;

/// Annotations past this many on one page are ignored (they stay in the file untouched).
const MAX_PER_PAGE: usize = 10_000;

/// Subtypes that are not annotations to the user: popups belong to their parent, widgets are
/// form fields (forms ADR), links are navigation. They stay in `/Annots` untouched.
const NOT_LOADED: [&[u8]; 3] = [b"Popup", b"Widget", b"Link"];

/// The annotations of every page of `doc`, in page order and z-order.
pub fn read_annotations(doc: &Document, source: SourceId) -> Vec<Vec<Annotation>> {
    doc.get_pages()
        .into_values()
        .map(|page| {
            read_page(doc, page, source)
                .map(|list| list.into_iter().map(|(_, a)| a).collect())
                .unwrap_or_default()
        })
        .collect()
}

/// The annotations Ferrix loads from one page, with their object ids.
pub fn read_page(
    doc: &Document,
    page: ObjectId,
    source: SourceId,
) -> Result<Vec<(ObjectId, Annotation)>, AnnotError> {
    let page = doc.get_dictionary(page)?;
    let Ok(annots) = page.get_deref(b"Annots", doc).and_then(Object::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in annots.iter().take(MAX_PER_PAGE) {
        // Annotations must be indirect objects; a direct one can't be addressed for editing.
        let Ok(id) = item.as_reference() else {
            continue;
        };
        let Ok(dict) = doc.get_dictionary(id) else {
            continue;
        };
        let subtype = dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap_or(b"");
        if NOT_LOADED.contains(&subtype) {
            continue;
        }
        let origin = AnnotOrigin::Existing {
            source,
            object: ObjectRef {
                number: id.0,
                generation: id.1,
            },
        };
        out.push((id, read_dict(doc, dict, origin)));
    }
    Ok(out)
}

/// Reads one annotation dictionary.
pub(crate) fn read_dict(doc: &Document, dict: &Dictionary, origin: AnnotOrigin) -> Annotation {
    let subtype = String::from_utf8_lossy(
        dict.get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap_or(b"Unknown"),
    )
    .into_owned();
    let mut props = read_props(doc, dict);
    let other = || AnnotKind::Other {
        subtype: subtype.clone(),
    };
    let reply_to = dict
        .get(b"IRT")
        .and_then(Object::as_reference)
        .ok()
        .map(|(number, generation)| ReplyTo::File(ObjectRef { number, generation }));
    // Replies are notes with /IRT; group members (/RT /Group) stay display-only, deleted with
    // their primary. Shapes drawn with features our appearance can't reproduce stay
    // display-only too, so editing never silently drops them.
    let grouped = name(doc, dict, b"RT").is_some_and(|rt| rt == "Group");
    let kind = if reply_to.is_some() {
        if subtype == "Text" && !grouped {
            let state = match (name(doc, dict, b"StateModel"), text(doc, dict, b"State")) {
                (model, Some(state)) => Some(ferrix_core::ReviewState {
                    model: model.unwrap_or_else(|| "Marked".into()),
                    state,
                }),
                _ => None,
            };
            AnnotKind::Reply { state }
        } else {
            other()
        }
    } else {
        match subtype.as_str() {
            "Text" => AnnotKind::Note {
                icon: NoteIcon::from_name(
                    &name(doc, dict, b"Name").unwrap_or_else(|| "Note".into()),
                ),
                open: matches!(deref(doc, dict, b"Open"), Some(Object::Boolean(true))),
            },
            "Square" | "Circle" if has_cloudy_border(doc, dict) => other(),
            "Square" | "Circle" => {
                // `/RD` insets the drawn shape from `/Rect`; the model's rect is the shape's box.
                if let Some(rd) = numbers(doc, dict, b"RD").filter(|rd| rd.len() == 4) {
                    let r = props.rect;
                    let inner = PdfBox {
                        left: r.left + rd[0],
                        bottom: r.bottom + rd[1],
                        right: r.right - rd[2],
                        top: r.top - rd[3],
                    };
                    if rd.iter().all(|v| *v >= 0.0)
                        && inner.left <= inner.right
                        && inner.bottom <= inner.top
                    {
                        props.rect = inner;
                    }
                }
                if subtype == "Square" {
                    AnnotKind::Square
                } else {
                    AnnotKind::Circle
                }
            }
            "Ink" => ink_list(doc, dict).map_or_else(other, |strokes| AnnotKind::Ink { strokes }),
            "FreeText" if free_text_is_plain(doc, dict) => {
                let (size, color) = default_appearance(doc, dict);
                AnnotKind::FreeText { size, color }
            }
            name @ ("Highlight" | "Underline" | "StrikeOut" | "Squiggly") => {
                match (MarkupStyle::from_subtype(name), quad_points(doc, dict)) {
                    (Some(style), Some(quads)) => AnnotKind::Markup { style, quads },
                    _ => other(),
                }
            }
            // Ferrix's own standard stamps (`/Name /#Approved`) are redrawn when edited. Any
            // other stamp with one plain appearance keeps it and can be moved and resized;
            // stamps with appearance states stay display-only.
            "Stamp" => {
                let ours = name(doc, dict, b"Name")
                    .and_then(|n| n.strip_prefix('#').map(str::to_owned))
                    .filter(|n| crate::stamp::standard(n).is_some());
                match ours {
                    Some(n) => AnnotKind::Stamp {
                        content: ferrix_core::StampContent::Standard(n),
                    },
                    None if single_appearance(doc, dict) => AnnotKind::Stamp {
                        content: ferrix_core::StampContent::Kept,
                    },
                    None => other(),
                }
            }
            "Line" if has_line_extras(doc, dict) => other(),
            "Line" => line(doc, dict).unwrap_or_else(other),
            _ => other(),
        }
    };
    // The same normalization `AnnotSet::insert_loaded` applies, so the writer's "did this
    // change?" comparison sees identical models for untouched annotations.
    Annotation {
        origin,
        props,
        kind,
        reply_to,
    }
    .sanitized()
}

/// Text boxes Ferrix can redraw faithfully: plain or typewriter boxes without callouts, line
/// ends, rotation, cloudy borders, a fill or styled rich text (the PDF review of phase 3).
fn free_text_is_plain(doc: &Document, dict: &Dictionary) -> bool {
    let intent_ok = name(doc, dict, b"IT").is_none_or(|it| it == "FreeTextTypeWriter");
    let rotated = deref(doc, dict, b"Rotate")
        .and_then(|r| r.as_i64().ok())
        .is_some_and(|r| r % 360 != 0);
    let filled = numbers(doc, dict, b"C").is_some_and(|c| !c.is_empty());
    let styled = text(doc, dict, b"RC").is_some_and(|rc| {
        let rc = rc.to_ascii_lowercase();
        [
            "<b>",
            "<b ",
            "<i>",
            "<i ",
            "<span",
            "font-weight",
            "font-style",
        ]
        .iter()
        .any(|tag| rc.contains(tag))
    });
    intent_ok
        && !dict.has(b"CL")
        && !dict.has(b"LE")
        && !rotated
        && !filled
        && !styled
        && !has_cloudy_border(doc, dict)
}

/// Font size and colour from `/DA` (e.g. `/Helv 12 Tf 0 0 1 rg`); 12 pt black if absent.
fn default_appearance(doc: &Document, dict: &Dictionary) -> (f32, Rgb) {
    let da = text(doc, dict, b"DA").unwrap_or_default();
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut size = 12.0;
    let mut color = Rgb::BLACK;
    for (i, op) in tokens.iter().enumerate() {
        let arg = |back: usize| {
            i.checked_sub(back)
                .and_then(|j| tokens.get(j))
                .and_then(|t| t.parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        match *op {
            "Tf" => {
                if let Some(s) = arg(1).filter(|s| (4.0..=144.0).contains(s)) {
                    size = s;
                }
            }
            "rg" => {
                if let (Some(r), Some(g), Some(b)) = (arg(3), arg(2), arg(1)) {
                    color = Rgb::new(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
                }
            }
            "g" => {
                if let Some(v) = arg(1) {
                    let v = v.clamp(0.0, 1.0);
                    color = Rgb::new(v, v, v);
                }
            }
            "k" => {
                if let (Some(c), Some(m), Some(y), Some(k)) = (arg(4), arg(3), arg(2), arg(1)) {
                    let f = |x: f32| ((1.0 - x) * (1.0 - k)).clamp(0.0, 1.0);
                    color = Rgb::new(f(c), f(m), f(y));
                }
            }
            _ => {}
        }
    }
    (size, color)
}

/// `/QuadPoints` as top-left, top-right, bottom-left, bottom-right quads. Acrobat and most
/// writers use that order; quads in the specification's counter-clockwise order are reordered.
fn quad_points(doc: &Document, dict: &Dictionary) -> Option<Vec<[PdfPoint; 4]>> {
    let n = numbers(doc, dict, b"QuadPoints")?;
    if n.is_empty() || n.len() % 8 != 0 || n.len() / 8 > ferrix_core::MAX_QUADS {
        return None;
    }
    Some(
        n.as_chunks::<8>()
            .0
            .iter()
            .map(|q| {
                let p = |i: usize| PdfPoint::new(q[2 * i], q[2 * i + 1]);
                let (a, b, c, d) = (p(0), p(1), p(2), p(3));
                // In the Z order a→b→c→d draws a bow tie: b–c crosses d–a.
                if segments_cross(b, c, d, a) {
                    [a, b, c, d]
                } else {
                    // Counter-clockwise from bottom-left: a=BL, b=BR, c=TR, d=TL.
                    [d, c, a, b]
                }
            })
            .collect(),
    )
}

fn segments_cross(p1: PdfPoint, p2: PdfPoint, q1: PdfPoint, q2: PdfPoint) -> bool {
    let side = |a: PdfPoint, b: PdfPoint, c: PdfPoint| {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    };
    let d1 = side(q1, q2, p1);
    let d2 = side(q1, q2, p2);
    let d3 = side(p1, p2, q1);
    let d4 = side(p1, p2, q2);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// A cloudy border (`/BE /S /C`, ISO 32000-2 Table 168).
fn has_cloudy_border(doc: &Document, dict: &Dictionary) -> bool {
    deref(doc, dict, b"BE")
        .and_then(|o| o.as_dict().ok())
        .and_then(|be| name(doc, be, b"S"))
        .is_some_and(|s| s == "C")
}

/// Leader lines, a caption, an intent or measurement (ISO 32000-2 Table 175).
fn has_line_extras(doc: &Document, dict: &Dictionary) -> bool {
    let nonzero = |key: &[u8]| {
        deref(doc, dict, key)
            .and_then(number)
            .is_some_and(|v| v != 0.0)
    };
    nonzero(b"LL")
        || nonzero(b"LLE")
        || nonzero(b"LLO")
        || matches!(deref(doc, dict, b"Cap"), Some(Object::Boolean(true)))
        || dict.has(b"IT")
        || dict.has(b"Measure")
}

fn read_props(doc: &Document, dict: &Dictionary) -> AnnotProps {
    let rect = numbers(doc, dict, b"Rect")
        .filter(|n| n.len() == 4)
        .map(|n| PdfBox::from_corners(PdfPoint::new(n[0], n[1]), PdfPoint::new(n[2], n[3])))
        .unwrap_or_default();
    let border = deref(doc, dict, b"BS").and_then(|o| o.as_dict().ok());
    let border_width = border
        .and_then(|bs| deref(doc, bs, b"W"))
        .and_then(number)
        .or_else(|| numbers(doc, dict, b"Border").and_then(|b| b.get(2).copied()))
        .unwrap_or(1.0);
    let dashed = border
        .and_then(|bs| name(doc, bs, b"S"))
        .is_some_and(|s| s == "D");
    let dash = if dashed {
        border
            .and_then(|bs| numbers(doc, bs, b"D"))
            .unwrap_or_else(|| vec![3.0])
    } else {
        Vec::new()
    };
    AnnotProps {
        rect,
        color: color(doc, dict, b"C"),
        interior: color(doc, dict, b"IC"),
        opacity: deref(doc, dict, b"CA")
            .and_then(number)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0),
        border_width: border_width.clamp(0.0, 1000.0),
        dash: dash
            .into_iter()
            .take(16)
            .map(|d| d.clamp(0.0, 1000.0))
            .collect(),
        author: text(doc, dict, b"T"),
        contents: text(doc, dict, b"Contents"),
        name: text(doc, dict, b"NM"),
        modified: text(doc, dict, b"M"),
        flags: AnnotFlags(
            deref(doc, dict, b"F")
                .and_then(|o| o.as_i64().ok())
                .map_or(0, |f| f as u32),
        ),
    }
}

fn deref<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get_deref(key, doc).ok()
}

fn number(object: &Object) -> Option<f32> {
    object.as_float().ok().filter(|v| v.is_finite())
}

fn numbers(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<Vec<f32>> {
    let array = deref(doc, dict, key)?.as_array().ok()?;
    array
        .iter()
        .map(|o| doc.dereference(o).ok().and_then(|(_, o)| number(o)))
        .collect()
}

fn name(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    deref(doc, dict, key)
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
}

fn text(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let object = deref(doc, dict, key)?;
    let mut text = lopdf::decode_text_string(object).ok().or_else(|| {
        object
            .as_str()
            .ok()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    })?;
    if text.len() > MAX_TEXT_BYTES {
        let mut cut = MAX_TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}

/// `/C` or `/IC`: 0 components is transparent, 1 gray, 3 RGB, 4 CMYK (ISO 32000-2 Table 166).
fn color(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<Rgb> {
    let c = numbers(doc, dict, key)?;
    let c: Vec<f32> = c.into_iter().map(|v| v.clamp(0.0, 1.0)).collect();
    match c.as_slice() {
        [g] => Some(Rgb::new(*g, *g, *g)),
        [r, g, b] => Some(Rgb::new(*r, *g, *b)),
        [c, m, y, k] => Some(Rgb::new(
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        )),
        _ => None,
    }
}

fn ink_list(doc: &Document, dict: &Dictionary) -> Option<Vec<Vec<PdfPoint>>> {
    let strokes = deref(doc, dict, b"InkList")?.as_array().ok()?;
    if strokes.is_empty() || strokes.len() > MAX_STROKES {
        return None;
    }
    strokes
        .iter()
        .map(|stroke| {
            let values = doc.dereference(stroke).ok()?.1.as_array().ok()?;
            if values.len() < 2 || values.len() % 2 != 0 || values.len() / 2 > MAX_STROKE_POINTS {
                return None;
            }
            values
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[x, y]| Some(PdfPoint::new(number(x)?, number(y)?)))
                .collect()
        })
        .collect()
}

fn line(doc: &Document, dict: &Dictionary) -> Option<AnnotKind> {
    let l = numbers(doc, dict, b"L").filter(|l| l.len() == 4)?;
    let ends = deref(doc, dict, b"LE")
        .and_then(|o| o.as_array().ok())
        .map(|a| {
            let end = |i: usize| {
                a.get(i)
                    .and_then(|o| o.as_name().ok())
                    .map(|n| LineEnd::from_name(&String::from_utf8_lossy(n)))
                    .unwrap_or_default()
            };
            [end(0), end(1)]
        })
        .unwrap_or_default();
    Some(AnnotKind::Line {
        from: PdfPoint::new(l[0], l[1]),
        to: PdfPoint::new(l[2], l[3]),
        ends,
    })
}

/// Whether `/AP /N` is one form XObject (not a dictionary of states) and `/AS` is absent: the
/// appearance can then be kept as it is while `/Rect` changes.
fn single_appearance(doc: &Document, dict: &Dictionary) -> bool {
    if dict.has(b"AS") {
        return false;
    }
    let Some(ap) = deref(doc, dict, b"AP").and_then(|o| o.as_dict().ok()) else {
        return false;
    };
    let Ok(normal) = ap.get(b"N") else {
        return false;
    };
    let normal = match normal {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    };
    normal
        .and_then(|o| o.as_stream().ok())
        .is_some_and(|s| s.dict.get(b"BBox").is_ok())
}
