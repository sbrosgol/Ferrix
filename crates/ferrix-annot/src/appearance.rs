//! What an annotation looks like, as a display list in PDF user space.
//!
//! The same [`Appearance`] is drawn by the editor while the user works and written as the
//! annotation's `/AP /N` form XObject, so Ferrix and every other reader show the same thing
//! (ADR-0009 §5).

use std::f32::consts::PI;
use std::fmt::Write as _;

use ferrix_core::{AnnotKind, Annotation, LineEnd, MarkupStyle, NoteIcon, Rgb};
use ferrix_types::{PdfBox, PdfPoint};

/// Bézier control-point distance for a quarter circle of radius 1.
const KAPPA: f32 = 0.552_284_8;
/// Size of a note icon in points (Acrobat uses about the same).
const NOTE_SIZE: f32 = 20.0;

/// One segment of a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathEl {
    MoveTo(PdfPoint),
    LineTo(PdfPoint),
    CurveTo(PdfPoint, PdfPoint, PdfPoint),
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub color: Rgb,
    pub width: f32,
    pub dash: Vec<f32>,
    /// Round caps and joins (ink); otherwise miter joins and butt caps.
    pub round: bool,
}

/// A path, filled and/or stroked.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub path: Vec<PathEl>,
    pub fill: Option<Rgb>,
    pub stroke: Option<Stroke>,
}

/// The look of one annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    /// Everything drawn lies inside this box; written as the form's `/BBox` and as `/Rect`.
    pub bbox: PdfBox,
    /// Applied to the whole appearance.
    pub opacity: f32,
    /// Blend with the page by multiplying (highlights: the text under them stays dark).
    pub multiply: bool,
    pub items: Vec<Item>,
}

/// The rectangle of a note icon whose top-left corner is at `at`.
pub fn note_rect(at: PdfPoint) -> PdfBox {
    PdfBox {
        left: at.x,
        bottom: at.y - NOTE_SIZE,
        right: at.x + NOTE_SIZE,
        top: at.y,
    }
}

/// The appearance of `annot`, or `None` for types Ferrix does not draw ([`AnnotKind::Other`]).
pub fn appearance(annot: &Annotation) -> Option<Appearance> {
    let props = &annot.props;
    let width = props.border_width;
    let stroke = |round: bool| {
        props.color.filter(|_| width > 0.0).map(|color| Stroke {
            color,
            width,
            dash: props.dash.clone(),
            round,
        })
    };
    let (items, bbox) = match &annot.kind {
        AnnotKind::Other { .. } => return None,
        AnnotKind::Note { icon, .. } => {
            let rect = note_rect(PdfPoint::new(props.rect.left, props.rect.top));
            (note_icon(rect, props.color, icon), rect)
        }
        AnnotKind::Square => {
            let r = inset(props.rect, width / 2.0);
            let path = vec![
                PathEl::MoveTo(PdfPoint::new(r.left, r.bottom)),
                PathEl::LineTo(PdfPoint::new(r.right, r.bottom)),
                PathEl::LineTo(PdfPoint::new(r.right, r.top)),
                PathEl::LineTo(PdfPoint::new(r.left, r.top)),
                PathEl::Close,
            ];
            let item = Item {
                path,
                fill: props.interior,
                stroke: stroke(false),
            };
            (vec![item], props.rect)
        }
        AnnotKind::Circle => {
            let r = inset(props.rect, width / 2.0);
            let item = Item {
                path: ellipse(r),
                fill: props.interior,
                stroke: stroke(false),
            };
            (vec![item], props.rect)
        }
        AnnotKind::Ink { strokes } => {
            let items: Vec<Item> = strokes
                .iter()
                .map(|points| Item {
                    path: polyline(points),
                    fill: None,
                    stroke: stroke(true),
                })
                .collect();
            let bbox = PdfBox::bounding(strokes.iter().flatten().copied())
                .unwrap_or(props.rect)
                .outset(width / 2.0 + 1.0);
            (items, bbox)
        }
        AnnotKind::Markup { style, quads } => {
            let color = props.color.unwrap_or(Rgb::new(1.0, 0.82, 0.2));
            let items = markup_items(*style, quads, color);
            let bbox = PdfBox::bounding(quads.iter().flatten().copied())
                .unwrap_or(props.rect)
                .outset(1.0);
            (items, bbox)
        }
        AnnotKind::Line { from, to, ends } => {
            let mut items = vec![Item {
                path: vec![PathEl::MoveTo(*from), PathEl::LineTo(*to)],
                fill: None,
                stroke: stroke(false),
            }];
            let mut bbox = PdfBox::from_corners(*from, *to);
            // ends[0] sits at `from` pointing away from `to`; ends[1] at `to`.
            for (end, tip, tail) in [(ends[0], *from, *to), (ends[1], *to, *from)] {
                if let Some(item) = line_end(end, tip, tail, width, props.color, props.interior) {
                    bbox = item_points(&item).fold(bbox, PdfBox::union_point);
                    items.push(item);
                }
            }
            (items, bbox.outset(width + 1.0))
        }
    };
    Some(Appearance {
        bbox,
        opacity: props.opacity,
        multiply: matches!(
            annot.kind,
            AnnotKind::Markup {
                style: MarkupStyle::Highlight,
                ..
            }
        ),
        items,
    })
}

/// Text markup over `quads` (each top-left, top-right, bottom-left, bottom-right).
fn markup_items(style: MarkupStyle, quads: &[[PdfPoint; 4]], color: Rgb) -> Vec<Item> {
    quads
        .iter()
        .filter_map(|&[tl, tr, bl, br]| {
            let (bx, by) = (br.x - bl.x, br.y - bl.y);
            let len = bx.hypot(by);
            let (hx, hy) = (tl.x - bl.x, tl.y - bl.y);
            let height = hx.hypot(hy);
            if len <= f32::EPSILON || height <= f32::EPSILON {
                return None;
            }
            // Along the text (u) and towards its top (n), so rotated text works too.
            let (ux, uy) = (bx / len, by / len);
            let (nx, ny) = (hx / height, hy / height);
            let at = |along: f32, up: f32| {
                PdfPoint::new(bl.x + ux * along + nx * up, bl.y + uy * along + ny * up)
            };
            let line = |up: f32, width: f32| Item {
                path: vec![PathEl::MoveTo(at(0.0, up)), PathEl::LineTo(at(len, up))],
                fill: None,
                stroke: Some(Stroke {
                    color,
                    width,
                    dash: Vec::new(),
                    round: false,
                }),
            };
            let thin = (height / 14.0).max(0.5);
            Some(match style {
                MarkupStyle::Highlight => Item {
                    path: vec![
                        PathEl::MoveTo(tl),
                        PathEl::LineTo(tr),
                        PathEl::LineTo(br),
                        PathEl::LineTo(bl),
                        PathEl::Close,
                    ],
                    fill: Some(color),
                    stroke: None,
                },
                MarkupStyle::Underline => line(height * 0.06, thin),
                MarkupStyle::StrikeOut => line(height * 0.45, thin),
                MarkupStyle::Squiggly => {
                    // A zigzag under the text: period a quarter of the line height.
                    let period = (height / 4.0).max(1.0);
                    let amplitude = height / 16.0;
                    let mut path = vec![PathEl::MoveTo(at(0.0, amplitude))];
                    let mut x = period / 2.0;
                    let mut up = false;
                    while x < len {
                        path.push(PathEl::LineTo(at(
                            x,
                            if up { amplitude * 2.0 } else { 0.0 },
                        )));
                        up = !up;
                        x += period / 2.0;
                    }
                    path.push(PathEl::LineTo(at(len, amplitude)));
                    Item {
                        path,
                        fill: None,
                        stroke: Some(Stroke {
                            color,
                            width: thin,
                            dash: Vec::new(),
                            round: true,
                        }),
                    }
                }
            })
        })
        .collect()
}

impl Appearance {
    /// The content stream drawing this appearance's shapes, fully opaque. Opacity applies to
    /// the whole drawing at once (see [`Self::needs_group`]), so overlapping parts don't darken.
    /// With `cmyk`, colours are written as DeviceCMYK, for PDF/A files whose output intent is
    /// CMYK (ISO 19005-2 §6.2.4.3).
    pub fn content(&self, cmyk: bool) -> Vec<u8> {
        let color = |c: Rgb, stroke: bool| -> String {
            if cmyk {
                let k = 1.0 - c.r.max(c.g).max(c.b);
                let part = |v: f32| {
                    if k < 1.0 {
                        (1.0 - v - k) / (1.0 - k)
                    } else {
                        0.0
                    }
                };
                format!(
                    "{} {} {} {} {}",
                    num(part(c.r)),
                    num(part(c.g)),
                    num(part(c.b)),
                    num(k),
                    if stroke { "K" } else { "k" }
                )
            } else {
                format!(
                    "{} {} {} {}",
                    num(c.r),
                    num(c.g),
                    num(c.b),
                    if stroke { "RG" } else { "rg" }
                )
            }
        };
        let mut out = String::new();
        out.push_str("q\n");
        for item in &self.items {
            if item.fill.is_none() && item.stroke.is_none() {
                continue;
            }
            out.push_str("q\n");
            if let Some(c) = item.fill {
                let _ = writeln!(out, "{}", color(c, false));
            }
            if let Some(s) = &item.stroke {
                let _ = writeln!(out, "{} {} w", color(s.color, true), num(s.width));
                if s.round {
                    out.push_str("1 J 1 j\n");
                }
                if !s.dash.is_empty() {
                    let dash: Vec<String> = s.dash.iter().map(|d| num(*d)).collect();
                    let _ = writeln!(out, "[{}] 0 d", dash.join(" "));
                }
            }
            for el in &item.path {
                let _ = match el {
                    PathEl::MoveTo(p) => writeln!(out, "{} {} m", num(p.x), num(p.y)),
                    PathEl::LineTo(p) => writeln!(out, "{} {} l", num(p.x), num(p.y)),
                    PathEl::CurveTo(a, b, c) => writeln!(
                        out,
                        "{} {} {} {} {} {} c",
                        num(a.x),
                        num(a.y),
                        num(b.x),
                        num(b.y),
                        num(c.x),
                        num(c.y)
                    ),
                    PathEl::Close => writeln!(out, "h"),
                };
            }
            out.push_str(match (item.fill.is_some(), item.stroke.is_some()) {
                (true, true) => "B\n",
                (true, false) => "f\n",
                _ => "S\n",
            });
            out.push_str("Q\n");
        }
        out.push_str("Q\n");
        out.into_bytes()
    }

    /// Whether the drawing must be painted as a transparency group at [`Self::opacity`].
    pub fn needs_group(&self) -> bool {
        self.opacity < 1.0
    }
}

/// A number as PDF content syntax: at most three decimals, no exponent, no trailing zeros.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn inset(r: PdfBox, by: f32) -> PdfBox {
    let by = by.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    r.outset(-by)
}

fn ellipse(r: PdfBox) -> Vec<PathEl> {
    let (cx, cy) = ((r.left + r.right) / 2.0, (r.bottom + r.top) / 2.0);
    let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let p = PdfPoint::new;
    vec![
        PathEl::MoveTo(p(cx + rx, cy)),
        PathEl::CurveTo(p(cx + rx, cy + ky), p(cx + kx, cy + ry), p(cx, cy + ry)),
        PathEl::CurveTo(p(cx - kx, cy + ry), p(cx - rx, cy + ky), p(cx - rx, cy)),
        PathEl::CurveTo(p(cx - rx, cy - ky), p(cx - kx, cy - ry), p(cx, cy - ry)),
        PathEl::CurveTo(p(cx + kx, cy - ry), p(cx + rx, cy - ky), p(cx + rx, cy)),
        PathEl::Close,
    ]
}

fn polyline(points: &[PdfPoint]) -> Vec<PathEl> {
    let mut path = Vec::with_capacity(points.len() + 1);
    if let Some((first, rest)) = points.split_first() {
        path.push(PathEl::MoveTo(*first));
        if rest.is_empty() {
            // A single tap still leaves a dot: a zero-length line drawn with a round cap.
            path.push(PathEl::LineTo(*first));
        }
        path.extend(rest.iter().map(|p| PathEl::LineTo(*p)));
    }
    path
}

fn item_points(item: &Item) -> impl Iterator<Item = PdfPoint> + '_ {
    item.path.iter().flat_map(|el| match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
        PathEl::CurveTo(a, b, c) => vec![a, b, c],
        PathEl::Close => vec![],
    })
}

/// The decoration at `tip` of a line coming from `tail` (ISO 32000-2 Table 179).
fn line_end(
    end: LineEnd,
    tip: PdfPoint,
    tail: PdfPoint,
    width: f32,
    color: Option<Rgb>,
    interior: Option<Rgb>,
) -> Option<Item> {
    let color = color?;
    let (dx, dy) = (tip.x - tail.x, tip.y - tail.y);
    let len = (dx * dx + dy * dy).sqrt();
    // Direction from the tail towards the tip; any direction will do for a zero-length line.
    let (ux, uy) = if len > f32::EPSILON {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let size = (width * 3.0).max(6.0);
    let at = |along: f32, across: f32| {
        PdfPoint::new(
            tip.x + ux * along - uy * across,
            tip.y + uy * along + ux * across,
        )
    };
    let stroke = Some(Stroke {
        color,
        width: width.max(0.5),
        dash: Vec::new(),
        round: false,
    });
    // Arrow wings sit 30° off the line.
    let back = size * (PI / 6.0).cos();
    let side = size * (PI / 6.0).sin();
    let item = match end {
        LineEnd::None => return None,
        LineEnd::OpenArrow | LineEnd::ClosedArrow => {
            let closed = end == LineEnd::ClosedArrow;
            let mut path = vec![
                PathEl::MoveTo(at(-back, side)),
                PathEl::LineTo(tip),
                PathEl::LineTo(at(-back, -side)),
            ];
            if closed {
                path.push(PathEl::Close);
            }
            Item {
                path,
                fill: if closed { interior } else { None },
                stroke,
            }
        }
        LineEnd::ROpenArrow | LineEnd::RClosedArrow => {
            let closed = end == LineEnd::RClosedArrow;
            let mut path = vec![
                PathEl::MoveTo(at(back, side)),
                PathEl::LineTo(tip),
                PathEl::LineTo(at(back, -side)),
            ];
            if closed {
                path.push(PathEl::Close);
            }
            Item {
                path,
                fill: if closed { interior } else { None },
                stroke,
            }
        }
        LineEnd::Square => {
            let h = size / 2.0;
            Item {
                path: vec![
                    PathEl::MoveTo(at(-h, -h)),
                    PathEl::LineTo(at(h, -h)),
                    PathEl::LineTo(at(h, h)),
                    PathEl::LineTo(at(-h, h)),
                    PathEl::Close,
                ],
                fill: interior,
                stroke,
            }
        }
        LineEnd::Diamond => {
            let h = size / 2.0;
            Item {
                path: vec![
                    PathEl::MoveTo(at(-h, 0.0)),
                    PathEl::LineTo(at(0.0, -h)),
                    PathEl::LineTo(at(h, 0.0)),
                    PathEl::LineTo(at(0.0, h)),
                    PathEl::Close,
                ],
                fill: interior,
                stroke,
            }
        }
        LineEnd::Circle => {
            let h = size / 2.0;
            Item {
                path: ellipse(PdfBox {
                    left: tip.x - h,
                    bottom: tip.y - h,
                    right: tip.x + h,
                    top: tip.y + h,
                }),
                fill: interior,
                stroke,
            }
        }
        LineEnd::Butt => {
            let h = size / 2.0;
            Item {
                path: vec![PathEl::MoveTo(at(0.0, h)), PathEl::LineTo(at(0.0, -h))],
                fill: None,
                stroke,
            }
        }
        LineEnd::Slash => {
            let h = size / 2.0;
            // 30° clockwise from perpendicular to the line.
            let (s, c) = (PI / 6.0).sin_cos();
            Item {
                path: vec![
                    PathEl::MoveTo(at(h * s, h * c)),
                    PathEl::LineTo(at(-h * s, -h * c)),
                ],
                fill: None,
                stroke,
            }
        }
    };
    Some(item)
}

/// A speech-bubble icon filling `r`. Every icon name draws the same bubble for now; the name
/// is still written, so readers with their own icons use them.
fn note_icon(r: PdfBox, color: Option<Rgb>, _icon: &NoteIcon) -> Vec<Item> {
    let fill = color.unwrap_or(Rgb::new(1.0, 0.82, 0.2));
    let outline = Stroke {
        color: Rgb::new(0.25, 0.25, 0.25),
        width: 0.8,
        dash: Vec::new(),
        round: true,
    };
    let p = |fx: f32, fy: f32| PdfPoint::new(r.left + fx * r.width(), r.bottom + fy * r.height());
    let bubble = vec![
        PathEl::MoveTo(p(0.1, 0.35)),
        PathEl::LineTo(p(0.1, 0.9)),
        PathEl::LineTo(p(0.9, 0.9)),
        PathEl::LineTo(p(0.9, 0.35)),
        PathEl::LineTo(p(0.45, 0.35)),
        PathEl::LineTo(p(0.25, 0.1)),
        PathEl::LineTo(p(0.28, 0.35)),
        PathEl::Close,
    ];
    let mut items = vec![Item {
        path: bubble,
        fill: Some(fill),
        stroke: Some(outline.clone()),
    }];
    for y in [0.73, 0.6, 0.47] {
        items.push(Item {
            path: vec![PathEl::MoveTo(p(0.22, y)), PathEl::LineTo(p(0.78, y))],
            fill: None,
            stroke: Some(Stroke {
                width: 0.7,
                ..outline.clone()
            }),
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use ferrix_core::{AnnotKind, Annotation};

    use super::*;

    #[test]
    fn numbers_are_compact_and_never_use_exponents() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(1e-9), "0");
        assert_eq!(num(1234.5), "1234.5");
        assert_eq!(num(0.123_45), "0.123");
        assert_eq!(num(f32::NAN), "0");
    }

    #[test]
    fn square_border_stays_inside_the_rectangle() {
        let mut a = Annotation::new(
            AnnotKind::Square,
            PdfBox::from_corners(PdfPoint::new(100.0, 100.0), PdfPoint::new(200.0, 150.0)),
        );
        a.props.border_width = 4.0;
        let ap = appearance(&a).expect("drawn");
        assert_eq!(ap.bbox, a.props.rect);
        let content = String::from_utf8(ap.content(false)).expect("ascii");
        assert!(content.contains("102 102 m"), "{content}");
        assert!(content.contains("4 w"));
        assert!(!ap.needs_group(), "opaque needs no transparency group");
        let cmyk = String::from_utf8(ap.content(true)).expect("ascii");
        assert!(cmyk.contains("0 0 0 1 K"), "black as CMYK: {cmyk}");
    }

    #[test]
    fn line_bounds_include_arrow_heads() {
        let a = Annotation::new(
            AnnotKind::Line {
                from: PdfPoint::new(100.0, 100.0),
                to: PdfPoint::new(200.0, 100.0),
                ends: [LineEnd::None, LineEnd::ClosedArrow],
            },
            PdfBox::default(),
        );
        let ap = appearance(&a).expect("drawn");
        assert_eq!(ap.items.len(), 2);
        assert!(ap.bbox.right >= 200.0 && ap.bbox.top > 100.0 + 2.0);
        for item in &ap.items {
            for p in item_points(item) {
                assert!(ap.bbox.contains(p), "{p:?} outside {:?}", ap.bbox);
            }
        }
    }

    #[test]
    fn ink_bounds_cover_the_stroke_width() {
        let mut a = Annotation::new(
            AnnotKind::Ink {
                strokes: vec![vec![PdfPoint::new(10.0, 10.0), PdfPoint::new(50.0, 80.0)]],
            },
            PdfBox::default(),
        );
        a.props.border_width = 6.0;
        a.props.opacity = 0.5;
        let ap = appearance(&a).expect("drawn");
        assert!(ap.bbox.left <= 7.0 && ap.bbox.top >= 83.0, "{:?}", ap.bbox);
        let content = String::from_utf8(ap.content(false)).expect("ascii");
        assert!(ap.needs_group() && content.contains("1 J 1 j"));
    }

    #[test]
    fn other_types_have_no_ferrix_appearance() {
        let a = Annotation::new(
            AnnotKind::Other {
                subtype: "Stamp".into(),
            },
            PdfBox::default(),
        );
        assert!(appearance(&a).is_none());
    }
}
