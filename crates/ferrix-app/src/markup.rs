//! Markup mode (ADR-0009 phase 2, `docs/design-system.md` › Annotations): tools, their
//! styles, and the geometry of drawing, hit-testing, moving and resizing annotations.
//!
//! Everything here is plain data and pure functions in PDF user space; the page overlay turns
//! pointer input into calls and draws the result.

use std::collections::HashMap;

use ferrix_core::{
    AnnotFlags, AnnotId, AnnotKind, Annotation, LineEnd, NoteIcon, Rgb, StampContent,
};
use ferrix_types::{PdfBox, PdfPoint};

/// The paper palette (design system › Annotations): names and colours.
pub(crate) const COLORS: [(&str, Rgb); 7] = [
    ("Red", rgb(0xE0_31_31)),
    ("Orange", rgb(0xE8_59_0C)),
    ("Yellow", rgb(0xF2_C2_00)),
    ("Green", rgb(0x2F_9E_44)),
    ("Blue", rgb(0x1C_7E_D6)),
    ("Purple", rgb(0x70_48_E8)),
    ("Black", rgb(0x1B_1D_21)),
];
pub(crate) const WIDTHS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
pub(crate) const OPACITIES: [u8; 4] = [100, 75, 50, 25];
/// Strokes drawn within this long of each other join one ink annotation.
pub(crate) const JOIN_STROKES_MS: u128 = 1_000;

const fn rgb(hex: u32) -> Rgb {
    Rgb::new(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    TextBox,
    Note,
    Stamp,
    Pen,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
}

impl Tool {
    /// Tools that work on text, in their own pill.
    pub(crate) const TEXT: [Self; 5] = [
        Self::Select,
        Self::Highlight,
        Self::Underline,
        Self::StrikeOut,
        Self::Squiggly,
    ];
    /// Tools that add things.
    pub(crate) const ADD: [Self; 8] = [
        Self::TextBox,
        Self::Note,
        Self::Stamp,
        Self::Pen,
        Self::Rectangle,
        Self::Ellipse,
        Self::Line,
        Self::Arrow,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Select => "Select (V)",
            Self::Highlight => "Highlight (H)",
            Self::Underline => "Underline (U)",
            Self::StrikeOut => "Strikethrough (S)",
            Self::Squiggly => "Squiggly underline (W)",
            Self::TextBox => "Text box (T)",
            Self::Note => "Note (N)",
            Self::Stamp => "Stamp (M)",
            Self::Pen => "Pen (P)",
            Self::Rectangle => "Rectangle (R)",
            Self::Ellipse => "Ellipse (O)",
            Self::Line => "Line (L)",
            Self::Arrow => "Arrow (A)",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "v" => Self::Select,
            "h" => Self::Highlight,
            "u" => Self::Underline,
            "s" => Self::StrikeOut,
            "w" => Self::Squiggly,
            "t" => Self::TextBox,
            "n" => Self::Note,
            "m" => Self::Stamp,
            "p" => Self::Pen,
            "r" => Self::Rectangle,
            "o" => Self::Ellipse,
            "l" => Self::Line,
            "a" => Self::Arrow,
            _ => return None,
        })
    }

    /// Tools that stay active after use; the others return to Select (as in Preview).
    pub(crate) fn sticky(self) -> bool {
        matches!(self, Self::Select | Self::Pen) || self.marks_text()
    }

    /// Tools that draw shapes with a crosshair (not Select or the text tools).
    pub(crate) fn draws(self) -> bool {
        self != Self::Select && !self.marks_text()
    }

    pub(crate) fn marks_text(self) -> bool {
        matches!(
            self,
            Self::Highlight | Self::Underline | Self::StrikeOut | Self::Squiggly
        )
    }
}

/// Colour, width and opacity for new annotations of one tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Style {
    pub(crate) color: Rgb,
    pub(crate) width: f32,
    pub(crate) opacity: f32,
}

/// One annotation gesture in progress.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Gesture {
    /// Drawing a new shape or stroke from `start`.
    Draw {
        start: PdfPoint,
        current: PdfPoint,
        points: Vec<PdfPoint>,
    },
    /// Dragging the selected annotation from `start`.
    Move { start: PdfPoint, current: PdfPoint },
    /// Dragging one of the selection's handles.
    Resize { handle: Handle, current: PdfPoint },
}

/// A selection handle: box corners and edges, or a line's ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handle {
    Box(i8, i8),
    LineFrom,
    LineTo,
}

/// Markup mode state, while it is on.
#[derive(Debug, Clone)]
pub(crate) struct Markup {
    pub(crate) tool: Tool,
    styles: HashMap<Tool, Style>,
    pub(crate) selected: Option<AnnotId>,
    /// The page of the selection or gesture, and the gesture itself.
    pub(crate) gesture: Option<(ferrix_core::PageKey, Gesture)>,
    /// The ink annotation the next pen stroke may join, and when it was drawn.
    pub(crate) last_ink: Option<(AnnotId, std::time::Instant)>,
    /// What the Stamp tool places.
    pub(crate) stamp: StampContent,
}

impl Default for Markup {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            styles: HashMap::new(),
            selected: None,
            gesture: None,
            last_ink: None,
            stamp: StampContent::Standard("Approved".into()),
        }
    }
}

impl Markup {
    /// The style new annotations of `tool` get: notes yellow, everything else red, 2 pt.
    pub(crate) fn style(&self, tool: Tool) -> Style {
        self.styles.get(&tool).copied().unwrap_or(Style {
            // Notes and highlights yellow, text black, everything else red.
            color: match tool {
                Tool::Note | Tool::Highlight => COLORS[2].1,
                Tool::TextBox => COLORS[6].1,
                _ => COLORS[0].1,
            },
            width: 2.0,
            opacity: 1.0,
        })
    }

    pub(crate) fn set_style(&mut self, tool: Tool, style: Style) {
        self.styles.insert(tool, style);
    }
}

/// A finished (or in-progress) drag on a page, in PDF user space.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drag<'a> {
    pub(crate) start: PdfPoint,
    pub(crate) end: PdfPoint,
    /// Every point passed, for the pen.
    pub(crate) points: &'a [PdfPoint],
    /// Constrain: squares, circles, 45° lines.
    pub(crate) shift: bool,
    /// Draw boxes from the centre.
    pub(crate) alt: bool,
    /// The smallest meaningful drag, in points.
    pub(crate) min: f32,
}

/// The annotation a drag (or click, for notes) creates; `None` if too small to mean anything.
pub(crate) fn create(tool: Tool, style: Style, drag: &Drag<'_>) -> Option<Annotation> {
    let Drag {
        start,
        end,
        points,
        shift,
        alt,
        min,
    } = *drag;
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let moved = dx.abs().max(dy.abs()) >= min;
    let kind_rect = match tool {
        Tool::Select | Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => {
            return None;
        }
        Tool::TextBox => {
            let (w, h) = ferrix_annot::free_text::measure("", 12.0, None);
            (
                AnnotKind::FreeText {
                    size: 12.0,
                    color: COLORS[6].1,
                },
                PdfBox::from_corners(start, PdfPoint::new(start.x + w, start.y - h)),
            )
        }
        Tool::Stamp => return None,
        Tool::Note => (
            AnnotKind::Note {
                icon: NoteIcon::Comment,
                open: false,
            },
            ferrix_annot::note_rect(start),
        ),
        Tool::Pen => {
            let stroke = simplify(points, min / 8.0);
            if stroke.is_empty() {
                return None;
            }
            let rect = PdfBox::bounding(stroke.iter().copied())?;
            (
                AnnotKind::Ink {
                    strokes: vec![stroke],
                },
                rect,
            )
        }
        Tool::Rectangle | Tool::Ellipse => {
            if !moved {
                return None;
            }
            let (mut w, mut h) = (dx, dy);
            if shift {
                let side = w.abs().max(h.abs());
                w = side.copysign(w);
                h = side.copysign(h);
            }
            let rect = if alt {
                PdfBox::from_corners(
                    PdfPoint::new(start.x - w, start.y - h),
                    PdfPoint::new(start.x + w, start.y + h),
                )
            } else {
                PdfBox::from_corners(start, PdfPoint::new(start.x + w, start.y + h))
            };
            let kind = if tool == Tool::Rectangle {
                AnnotKind::Square
            } else {
                AnnotKind::Circle
            };
            (kind, rect)
        }
        Tool::Line | Tool::Arrow => {
            if !moved {
                return None;
            }
            let to = if shift { snap_45(start, end) } else { end };
            let ends = if tool == Tool::Arrow {
                [LineEnd::None, LineEnd::OpenArrow]
            } else {
                [LineEnd::None, LineEnd::None]
            };
            (
                AnnotKind::Line {
                    from: start,
                    to,
                    ends,
                },
                PdfBox::from_corners(start, to),
            )
        }
    };
    let (kind, rect) = kind_rect;
    let is_text = matches!(kind, AnnotKind::FreeText { .. });
    let mut annot = Annotation::new(kind, rect);
    annot.props.color = Some(style.color);
    annot.props.border_width = style.width;
    if is_text {
        // The text's colour is in the kind; a text box has no border.
        annot.props.color = None;
        annot.props.border_width = 0.0;
        if let AnnotKind::FreeText { color, .. } = &mut annot.kind {
            *color = style.color;
        }
    }
    annot.props.opacity = style.opacity;
    annot.props.name = Some(ferrix_annot::unique_name());
    Some(annot)
}

/// Drops points closer than `tolerance` to the previous kept one.
fn simplify(points: &[PdfPoint], tolerance: f32) -> Vec<PdfPoint> {
    let mut out: Vec<PdfPoint> = Vec::with_capacity(points.len());
    for p in points {
        if out
            .last()
            .is_none_or(|q| (p.x - q.x).hypot(p.y - q.y) >= tolerance)
        {
            out.push(*p);
        }
    }
    out
}

fn snap_45(from: PdfPoint, to: PdfPoint) -> PdfPoint {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = dx.hypot(dy);
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    PdfPoint::new(from.x + len * angle.cos(), from.y + len * angle.sin())
}

/// The box a selection outline is drawn around.
pub(crate) fn bounds(annot: &Annotation) -> PdfBox {
    ferrix_annot::appearance(annot).map_or(annot.props.rect, |ap| ap.bbox)
}

/// The topmost annotation at `p`, within `tolerance` points of strokes and lines.
pub(crate) fn hit(
    annots: &[(AnnotId, Annotation)],
    p: PdfPoint,
    tolerance: f32,
) -> Option<AnnotId> {
    annots.iter().rev().find_map(|(id, a)| {
        // Replies share their parent's /Rect but aren't drawn; the parent is what's clicked.
        if matches!(a.kind, AnnotKind::Reply { .. }) {
            return None;
        }
        let near = tolerance + a.props.border_width / 2.0;
        let inside = match &a.kind {
            AnnotKind::Ink { strokes } => strokes.iter().any(|s| polyline_distance(s, p) <= near),
            AnnotKind::Line { from, to, .. } => segment_distance(*from, *to, p) <= near,
            _ => bounds(a).outset(tolerance).contains(p),
        };
        inside.then_some(*id)
    })
}

fn segment_distance(a: PdfPoint, b: PdfPoint, p: PdfPoint) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

fn polyline_distance(points: &[PdfPoint], p: PdfPoint) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => (p.x - only.x).hypot(p.y - only.y),
        _ => points
            .windows(2)
            .map(|w| segment_distance(w[0], w[1], p))
            .fold(f32::INFINITY, f32::min),
    }
}

/// `annot` moved by `(dx, dy)` points.
pub(crate) fn moved(annot: &Annotation, dx: f32, dy: f32) -> Annotation {
    map_points(annot, |p| PdfPoint::new(p.x + dx, p.y + dy))
}

/// The handles of `annot`'s selection; none for notes, which keep their size.
pub(crate) fn handles(annot: &Annotation) -> Vec<(Handle, PdfPoint)> {
    match &annot.kind {
        // Notes keep their size, text boxes fit their text, text markup follows its text.
        AnnotKind::Note { .. }
        | AnnotKind::Reply { .. }
        | AnnotKind::FreeText { .. }
        | AnnotKind::Markup { .. }
        | AnnotKind::Other { .. } => Vec::new(),
        AnnotKind::Line { from, to, .. } => vec![(Handle::LineFrom, *from), (Handle::LineTo, *to)],
        _ => {
            let r = shape_box(annot);
            let mut out = Vec::with_capacity(8);
            for sx in -1..=1_i8 {
                for sy in -1..=1_i8 {
                    if sx == 0 && sy == 0 {
                        continue;
                    }
                    out.push((Handle::Box(sx, sy), box_point(r, sx, sy)));
                }
            }
            out
        }
    }
}

/// `annot` with `handle` dragged to `to`; `keep_ratio` (Shift) keeps the proportions.
pub(crate) fn resized(
    annot: &Annotation,
    handle: Handle,
    to: PdfPoint,
    keep_ratio: bool,
) -> Annotation {
    let mut out = annot.clone();
    match (handle, &mut out.kind) {
        (Handle::LineFrom, AnnotKind::Line { from, .. }) => *from = to,
        (Handle::LineTo, AnnotKind::Line { to: end, .. }) => *end = to,
        (Handle::Box(sx, sy), kind) => {
            // Stamps keep their proportions: labels and pictures would distort.
            let keep_ratio = keep_ratio || matches!(kind, AnnotKind::Stamp { .. });
            let old = shape_box(annot);
            let mut r = old;
            if sx < 0 {
                r.left = to.x.min(old.right - 1.0);
            } else if sx > 0 {
                r.right = to.x.max(old.left + 1.0);
            }
            if sy < 0 {
                r.bottom = to.y.min(old.top - 1.0);
            } else if sy > 0 {
                r.top = to.y.max(old.bottom + 1.0);
            }
            if keep_ratio && sx != 0 && sy != 0 && old.width() > 0.0 && old.height() > 0.0 {
                let scale = (r.width() / old.width()).max(r.height() / old.height());
                let (w, h) = (old.width() * scale, old.height() * scale);
                if sx < 0 {
                    r.left = r.right - w;
                } else {
                    r.right = r.left + w;
                }
                if sy < 0 {
                    r.bottom = r.top - h;
                } else {
                    r.top = r.bottom + h;
                }
            }
            out = scale_into(annot, old, r);
        }
        _ => {}
    }
    out
}

/// The box a shape's geometry spans (the rect for squares and circles, the points' bounds for
/// ink), which handles resize.
fn shape_box(annot: &Annotation) -> PdfBox {
    match &annot.kind {
        AnnotKind::Ink { strokes } => {
            PdfBox::bounding(strokes.iter().flatten().copied()).unwrap_or(annot.props.rect)
        }
        _ => annot.props.rect,
    }
}

fn box_point(r: PdfBox, sx: i8, sy: i8) -> PdfPoint {
    let x = match sx {
        -1 => r.left,
        1 => r.right,
        _ => (r.left + r.right) / 2.0,
    };
    let y = match sy {
        -1 => r.bottom,
        1 => r.top,
        _ => (r.bottom + r.top) / 2.0,
    };
    PdfPoint::new(x, y)
}

fn scale_into(annot: &Annotation, from: PdfBox, to: PdfBox) -> Annotation {
    let sx = if from.width() > 0.0 {
        to.width() / from.width()
    } else {
        1.0
    };
    let sy = if from.height() > 0.0 {
        to.height() / from.height()
    } else {
        1.0
    };
    let mut out = map_points(annot, |p| {
        PdfPoint::new(
            to.left + (p.x - from.left) * sx,
            to.bottom + (p.y - from.bottom) * sy,
        )
    });
    if matches!(annot.kind, AnnotKind::Square | AnnotKind::Circle) {
        out.props.rect = to;
    }
    out
}

fn map_points(annot: &Annotation, f: impl Fn(PdfPoint) -> PdfPoint) -> Annotation {
    let mut out = annot.clone();
    let r = annot.props.rect;
    out.props.rect = PdfBox::from_corners(
        f(PdfPoint::new(r.left, r.bottom)),
        f(PdfPoint::new(r.right, r.top)),
    );
    match &mut out.kind {
        AnnotKind::Ink { strokes } => {
            for p in strokes.iter_mut().flatten() {
                *p = f(*p);
            }
        }
        AnnotKind::Line { from, to, .. } => {
            *from = f(*from);
            *to = f(*to);
        }
        AnnotKind::Markup { quads, .. } => {
            for p in quads.iter_mut().flatten() {
                *p = f(*p);
            }
        }
        _ => {}
    }
    out
}

/// Whether Ferrix may change `annot` at all (as opposed to only deleting it).
pub(crate) fn editable(annot: &Annotation) -> bool {
    // Review states are other people's decisions: shown, never changed.
    let review_state = matches!(annot.kind, AnnotKind::Reply { state: Some(_) });
    annot.is_editable() && !annot.props.flags.has(AnnotFlags::READ_ONLY) && !review_state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style {
            color: COLORS[0].1,
            width: 2.0,
            opacity: 1.0,
        }
    }

    fn pt(x: f32, y: f32) -> PdfPoint {
        PdfPoint::new(x, y)
    }

    #[test]
    fn rectangles_constrain_and_grow_from_the_centre() {
        let r = create(
            Tool::Rectangle,
            style(),
            &Drag {
                start: pt(100.0, 100.0),
                end: pt(150.0, 120.0),
                points: &[],
                shift: true,
                alt: false,
                min: 3.0,
            },
        )
        .expect("drawn");
        assert_eq!(
            r.props.rect,
            PdfBox::from_corners(pt(100.0, 100.0), pt(150.0, 150.0))
        );
        let r = create(
            Tool::Ellipse,
            style(),
            &Drag {
                start: pt(100.0, 100.0),
                end: pt(110.0, 120.0),
                points: &[],
                shift: false,
                alt: true,
                min: 3.0,
            },
        )
        .expect("drawn");
        assert_eq!(
            r.props.rect,
            PdfBox::from_corners(pt(90.0, 80.0), pt(110.0, 120.0))
        );
        assert!(
            create(
                Tool::Rectangle,
                style(),
                &Drag {
                    start: pt(0.0, 0.0),
                    end: pt(1.0, 1.0),
                    points: &[],
                    shift: false,
                    alt: false,
                    min: 3.0
                }
            )
            .is_none(),
            "a click is not a rectangle"
        );
    }

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let a = create(
            Tool::Arrow,
            style(),
            &Drag {
                start: pt(0.0, 0.0),
                end: pt(100.0, 8.0),
                points: &[],
                shift: true,
                alt: false,
                min: 3.0,
            },
        )
        .expect("drawn");
        let AnnotKind::Line { to, ends, .. } = a.kind else {
            panic!("not a line");
        };
        assert!(to.y.abs() < 0.001 && (to.x - 100.32).abs() < 0.01, "{to:?}");
        assert_eq!(ends[1], LineEnd::OpenArrow);
    }

    #[test]
    fn notes_are_placed_by_their_top_left_corner() {
        let n = create(
            Tool::Note,
            style(),
            &Drag {
                start: pt(50.0, 700.0),
                end: pt(50.0, 700.0),
                points: &[],
                shift: false,
                alt: false,
                min: 3.0,
            },
        )
        .expect("placed");
        assert_eq!(
            n.props.rect,
            PdfBox::from_corners(pt(50.0, 680.0), pt(70.0, 700.0))
        );
        assert!(n.props.flags.has(AnnotFlags::NO_ZOOM));
    }

    #[test]
    fn hit_testing_follows_strokes_and_prefers_the_topmost() {
        let pen = create(
            Tool::Pen,
            style(),
            &Drag {
                start: pt(0.0, 0.0),
                end: pt(100.0, 0.0),
                points: &[pt(0.0, 0.0), pt(100.0, 0.0)],
                shift: false,
                alt: false,
                min: 3.0,
            },
        )
        .expect("stroke");
        let square = create(
            Tool::Rectangle,
            style(),
            &Drag {
                start: pt(40.0, -10.0),
                end: pt(60.0, 10.0),
                points: &[],
                shift: false,
                alt: false,
                min: 3.0,
            },
        )
        .expect("square");
        let list = vec![(AnnotId(1), pen), (AnnotId(2), square)];
        assert_eq!(
            hit(&list, pt(50.0, 0.0), 2.0),
            Some(AnnotId(2)),
            "topmost wins"
        );
        assert_eq!(
            hit(&list, pt(20.0, 2.0), 2.0),
            Some(AnnotId(1)),
            "near the stroke"
        );
        assert_eq!(hit(&list, pt(20.0, 20.0), 2.0), None);
    }

    #[test]
    fn moving_and_resizing_keep_geometry_consistent() {
        let square = create(
            Tool::Rectangle,
            style(),
            &Drag {
                start: pt(0.0, 0.0),
                end: pt(100.0, 50.0),
                points: &[],
                shift: false,
                alt: false,
                min: 3.0,
            },
        )
        .expect("square");
        let m = moved(&square, 10.0, -5.0);
        assert_eq!(
            m.props.rect,
            PdfBox::from_corners(pt(10.0, -5.0), pt(110.0, 45.0))
        );
        let r = resized(&square, Handle::Box(1, 1), pt(200.0, 60.0), true);
        assert_eq!(
            r.props.rect,
            PdfBox::from_corners(pt(0.0, 0.0), pt(200.0, 100.0))
        );
        let ink = create(
            Tool::Pen,
            style(),
            &Drag {
                start: pt(0.0, 0.0),
                end: pt(10.0, 10.0),
                points: &[pt(0.0, 0.0), pt(10.0, 10.0)],
                shift: false,
                alt: false,
                min: 3.0,
            },
        )
        .expect("ink");
        let big = resized(&ink, Handle::Box(1, 1), pt(20.0, 20.0), false);
        let AnnotKind::Ink { strokes } = big.kind else {
            panic!("not ink");
        };
        assert_eq!(strokes[0], vec![pt(0.0, 0.0), pt(20.0, 20.0)]);
        assert!(handles(&square).len() == 8 && handles(&m).len() == 8);
    }
}

/// Seconds since the Unix epoch for a PDF date (`D:YYYYMMDDHHmmSS` with an optional
/// `Z`/`+HH'mm'` offset, ISO 32000-2 §7.9.4); `None` if it isn't one.
pub(crate) fn pdf_date_seconds(date: &str) -> Option<i64> {
    let s = date.trim().strip_prefix("D:").unwrap_or(date.trim());
    let digits = |from: usize, len: usize| -> Option<i64> {
        s.get(from..from + len)
            .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?
            .parse()
            .ok()
    };
    let year = digits(0, 4)?;
    let month = digits(4, 2).unwrap_or(1).clamp(1, 12);
    let day = digits(6, 2).unwrap_or(1).clamp(1, 31);
    let hour = digits(8, 2).unwrap_or(0).min(23);
    let minute = digits(10, 2).unwrap_or(0).min(59);
    let second = digits(12, 2).unwrap_or(0).min(59);
    let offset = match s.get(14..15) {
        Some(sign @ ("+" | "-")) => {
            let h = digits(15, 2).unwrap_or(0);
            let m = s
                .get(18..20)
                .and_then(|m| m.parse::<i64>().ok())
                .unwrap_or(0);
            let total = (h * 60 + m) * 60;
            if sign == "+" { total } else { -total }
        }
        _ => 0,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// "Just now", "5 min ago", "3 hours ago", "Yesterday", "4 days ago", or the date
/// ("21 Nov 2026"), relative to `now` (seconds since the epoch). Dates are in UTC: Ferrix
/// has no time-zone database yet.
pub(crate) fn relative_date(date: &str, now: i64) -> String {
    let Some(then) = pdf_date_seconds(date) else {
        return String::new();
    };
    let ago = now - then;
    match ago {
        i64::MIN..=59 => "Just now".into(),
        60..=3_599 => format!("{} min ago", ago / 60),
        3_600..=86_399 => {
            let h = ago / 3600;
            if h == 1 {
                "1 hour ago".into()
            } else {
                format!("{h} hours ago")
            }
        }
        86_400..=172_799 => "Yesterday".into(),
        172_800..=604_799 => format!("{} days ago", ago / 86_400),
        _ => {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let s = date.trim().strip_prefix("D:").unwrap_or(date.trim());
            let month: usize = s.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(1);
            let day: u32 = s.get(6..8).and_then(|d| d.parse().ok()).unwrap_or(1);
            let year = s.get(0..4).unwrap_or("");
            format!("{day} {} {year}", MONTHS[month.clamp(1, 12) - 1])
        }
    }
}

#[cfg(test)]
mod date_tests {
    use super::*;

    #[test]
    fn parses_pdf_dates_with_offsets() {
        assert_eq!(pdf_date_seconds("D:19700101000000Z"), Some(0));
        assert_eq!(pdf_date_seconds("D:19700101010000+01'00'"), Some(0));
        assert_eq!(
            pdf_date_seconds("D:2026"),
            pdf_date_seconds("D:20260101000000")
        );
        assert_eq!(pdf_date_seconds("yesterday"), None);
    }

    #[test]
    fn describes_dates_relative_to_now() {
        let now = pdf_date_seconds("D:20261121120000Z").expect("date");
        assert_eq!(relative_date("D:20261121115930Z", now), "Just now");
        assert_eq!(relative_date("D:20261121114000Z", now), "20 min ago");
        assert_eq!(relative_date("D:20261121090000Z", now), "3 hours ago");
        assert_eq!(relative_date("D:20261120100000Z", now), "Yesterday");
        assert_eq!(relative_date("D:20261001000000Z", now), "1 Oct 2026");
    }
}

/// A stamp of `content` centred at `at`: standard stamps at their natural size, pictures at
/// 72 dpi but no more than 200 pt on a side.
pub(crate) fn stamp(content: &StampContent, at: PdfPoint) -> Annotation {
    let (w, h) = match content {
        StampContent::Standard(name) => ferrix_annot::stamp::standard_size(name),
        // The tool never holds a kept stamp; a plain square if it ever did.
        StampContent::Kept => (60.0, 60.0),
        StampContent::Image(image) => {
            let (w, h) = (image.width as f32, image.height as f32);
            let scale = (200.0 / w.max(h)).min(1.0);
            ((w * scale).max(8.0), (h * scale).max(8.0))
        }
    };
    let rect = PdfBox::from_corners(
        PdfPoint::new(at.x - w / 2.0, at.y - h / 2.0),
        PdfPoint::new(at.x + w / 2.0, at.y + h / 2.0),
    );
    let mut annot = Annotation::new(
        AnnotKind::Stamp {
            content: content.clone(),
        },
        rect,
    );
    annot.props.name = Some(ferrix_annot::unique_name());
    annot.props.flags = AnnotFlags(AnnotFlags::PRINT);
    annot
}
