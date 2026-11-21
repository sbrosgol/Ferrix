//! Annotations being edited (ADR-0009).
//!
//! The model holds what Ferrix understands about each annotation. Annotations that came from the
//! file remember their object ([`AnnotOrigin::Existing`]); when saving, the writer starts from the
//! original dictionary and overwrites only the keys modelled here, so unknown data survives.
//!
//! All geometry is in the source page's **PDF user space**, unrotated, with the file's own origin.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use ferrix_types::{PdfBox, PdfPoint};
use serde::{Deserialize, Serialize};

use crate::{EditError, PageKey, SourceId};

/// Most points in one ink stroke, and most strokes in one ink annotation. Bounds memory for
/// hostile files and runaway input; real handwriting stays far below both.
pub const MAX_STROKE_POINTS: usize = 100_000;
pub const MAX_STROKES: usize = 10_000;
/// Most points in one ink annotation altogether (about 8 MB of coordinates).
pub const MAX_INK_POINTS: usize = 1_000_000;
/// Longest text kept in an author or contents field, in bytes.
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// Largest coordinate accepted, in points. PDF readers are only required to handle ±32767;
/// this is generous while keeping widths and heights far from overflowing `f32`.
pub const MAX_COORD: f32 = 1.0e6;

/// An annotation's identity while editing; stable across edits, unique within an [`AnnotSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnnotId(pub u64);

/// An indirect object in a source document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectRef {
    pub number: u32,
    pub generation: u16,
}

/// Where an annotation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotOrigin {
    /// Made in this session; written as a new object.
    New,
    /// Read from `object` in `source`; written back over the same object if changed.
    Existing { source: SourceId, object: ObjectRef },
}

/// An RGB colour with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    fn is_valid(self) -> bool {
        [self.r, self.g, self.b]
            .iter()
            .all(|c| (0.0..=1.0).contains(c))
    }
}

/// Annotation flags (`/F`, ISO 32000-2 §12.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct AnnotFlags(pub u32);

impl AnnotFlags {
    pub const INVISIBLE: u32 = 1 << 0;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_ZOOM: u32 = 1 << 3;
    pub const NO_ROTATE: u32 = 1 << 4;
    pub const NO_VIEW: u32 = 1 << 5;
    pub const READ_ONLY: u32 = 1 << 6;
    pub const LOCKED: u32 = 1 << 7;
    pub const TOGGLE_NO_VIEW: u32 = 1 << 8;
    pub const LOCKED_CONTENTS: u32 = 1 << 9;

    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Properties every annotation type shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotProps {
    /// `/Rect`. For notes, squares and circles this is the shape's box; for ink and lines the
    /// writer recomputes it from the geometry.
    pub rect: PdfBox,
    /// `/C`: stroke colour (or icon colour for notes). `None` means transparent.
    pub color: Option<Rgb>,
    /// `/IC`: fill colour of squares, circles and closed line ends.
    pub interior: Option<Rgb>,
    /// `/CA`, `0.0..=1.0`.
    pub opacity: f32,
    /// `/BS /W` in points; 0 draws no border.
    pub border_width: f32,
    /// `/BS /D` dash pattern; empty for solid.
    pub dash: Vec<f32>,
    /// `/T`.
    pub author: Option<String>,
    /// `/Contents`: the comment text, also used as alternative text.
    pub contents: Option<String>,
    /// `/NM`, unique within the page.
    pub name: Option<String>,
    /// `/M` as written in the file (a PDF date string, or free text in older files).
    pub modified: Option<String>,
    pub flags: AnnotFlags,
}

impl Default for AnnotProps {
    fn default() -> Self {
        Self {
            rect: PdfBox::default(),
            color: Some(Rgb::BLACK),
            interior: None,
            opacity: 1.0,
            border_width: 1.0,
            dash: Vec::new(),
            author: None,
            contents: None,
            name: None,
            modified: None,
            flags: AnnotFlags(AnnotFlags::PRINT),
        }
    }
}

/// The icon of a note (`/Name` of a `Text` annotation).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NoteIcon {
    #[default]
    Comment,
    Note,
    Help,
    Insert,
    Key,
    NewParagraph,
    Paragraph,
    /// A name we don't draw specially; kept as written.
    Other(String),
}

impl NoteIcon {
    /// Names that aren't valid PDF names (empty, too long, delimiters) read as `Note`, the
    /// specification's default.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Comment" => Self::Comment,
            "Note" => Self::Note,
            "Help" => Self::Help,
            "Insert" => Self::Insert,
            "Key" => Self::Key,
            "NewParagraph" => Self::NewParagraph,
            "Paragraph" => Self::Paragraph,
            other if name_ok(other) => Self::Other(other.to_owned()),
            _ => Self::Note,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Comment => "Comment",
            Self::Note => "Note",
            Self::Help => "Help",
            Self::Insert => "Insert",
            Self::Key => "Key",
            Self::NewParagraph => "NewParagraph",
            Self::Paragraph => "Paragraph",
            Self::Other(name) => name,
        }
    }
}

/// A line ending style (`/LE`, ISO 32000-2 Table 179).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum LineEnd {
    #[default]
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

impl LineEnd {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Square,
        Self::Circle,
        Self::Diamond,
        Self::OpenArrow,
        Self::ClosedArrow,
        Self::Butt,
        Self::ROpenArrow,
        Self::RClosedArrow,
        Self::Slash,
    ];

    /// Unknown names read as `None`, as the specification requires.
    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|end| end.name() == name)
            .unwrap_or_default()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Diamond => "Diamond",
            Self::OpenArrow => "OpenArrow",
            Self::ClosedArrow => "ClosedArrow",
            Self::Butt => "Butt",
            Self::ROpenArrow => "ROpenArrow",
            Self::RClosedArrow => "RClosedArrow",
            Self::Slash => "Slash",
        }
    }
}

/// What an annotation is, with its type-specific geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotKind {
    /// A sticky note (`Text`).
    Note { icon: NoteIcon, open: bool },
    /// Freehand drawing; each stroke is a centre line (`/InkList`).
    Ink { strokes: Vec<Vec<PdfPoint>> },
    /// A rectangle drawn inside `rect`.
    Square,
    /// An ellipse drawn inside `rect`.
    Circle,
    /// A straight line or arrow (`/L`, `/LE`).
    Line {
        from: PdfPoint,
        to: PdfPoint,
        ends: [LineEnd; 2],
    },
    /// Highlight, underline, strike-out or squiggly underline over text: one quadrilateral per
    /// line run (`/QuadPoints`), each as top-left, top-right, bottom-left, bottom-right (the
    /// order Acrobat writes and readers expect).
    Markup {
        style: MarkupStyle,
        quads: Vec<[PdfPoint; 4]>,
    },
    /// Any other subtype (free text, stamps, attachments…). Shown, listed and deletable, never
    /// rewritten.
    Other { subtype: String },
}

/// Which text-markup annotation (ISO 32000-2 §12.5.6.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkupStyle {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl MarkupStyle {
    pub const ALL: [Self; 4] = [
        Self::Highlight,
        Self::Underline,
        Self::StrikeOut,
        Self::Squiggly,
    ];

    pub fn subtype(self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "StrikeOut",
            Self::Squiggly => "Squiggly",
        }
    }

    pub fn from_subtype(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.subtype() == name)
    }
}

/// Most quadrilaterals in one text-markup annotation.
pub const MAX_QUADS: usize = 10_000;

impl AnnotKind {
    /// The PDF `/Subtype` name.
    pub fn subtype(&self) -> &str {
        match self {
            Self::Note { .. } => "Text",
            Self::Ink { .. } => "Ink",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Line { .. } => "Line",
            Self::Markup { style, .. } => style.subtype(),
            Self::Other { subtype } => subtype,
        }
    }
}

/// One annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub origin: AnnotOrigin,
    pub props: AnnotProps,
    pub kind: AnnotKind,
    /// `/IRT`: the annotation (in the same source) this one replies to. Replies must stay on
    /// their parent's page, so deleting a parent deletes its [`AnnotSet::thread`].
    #[serde(default)]
    pub reply_to: Option<ObjectRef>,
}

impl Annotation {
    /// A new annotation of `kind` with default properties and `rect`. Notes keep their size and
    /// orientation when zoomed or rotated, as in other readers.
    pub fn new(kind: AnnotKind, rect: PdfBox) -> Self {
        let mut flags = AnnotFlags::PRINT;
        if matches!(kind, AnnotKind::Note { .. }) {
            flags |= AnnotFlags::NO_ZOOM | AnnotFlags::NO_ROTATE;
        }
        Self {
            origin: AnnotOrigin::New,
            props: AnnotProps {
                rect,
                flags: AnnotFlags(flags),
                ..AnnotProps::default()
            },
            kind,
            reply_to: None,
        }
    }

    /// Whether Ferrix may change this annotation's look or geometry. Deleting is allowed
    /// whenever [`Self::is_deletable`] is.
    pub fn is_editable(&self) -> bool {
        !matches!(self.kind, AnnotKind::Other { .. }) && self.is_deletable()
    }

    pub fn is_deletable(&self) -> bool {
        self.lock_violation(None).is_none()
    }

    /// This annotation with every invalid value replaced, so it passes validation. Used for
    /// annotations read from files: whatever can't be kept faithfully becomes [`AnnotKind::Other`].
    pub fn sanitized(mut self) -> Self {
        if self.validate().is_ok() {
            return self;
        }
        let p = &mut self.props;
        let r = p.rect;
        if !([r.left, r.bottom, r.right, r.top]
            .iter()
            .all(|v| coord_ok(*v))
            && r.left <= r.right
            && r.bottom <= r.top)
        {
            p.rect = PdfBox::default();
        }
        p.opacity = if p.opacity.is_finite() {
            p.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        p.border_width = if p.border_width.is_finite() {
            p.border_width.clamp(0.0, 1000.0)
        } else {
            1.0
        };
        if !dash_ok(&p.dash) {
            p.dash.clear();
        }
        for c in [&mut p.color, &mut p.interior] {
            if c.is_some_and(|c| !c.is_valid()) {
                *c = None;
            }
        }
        for t in [&mut p.author, &mut p.contents, &mut p.name, &mut p.modified] {
            if t.as_ref().is_some_and(|t| t.len() > MAX_TEXT_BYTES) {
                *t = None;
            }
        }
        if self.validate().is_err() {
            let subtype = match self.kind.subtype() {
                "" => "Unknown".to_owned(),
                s => s.to_owned(),
            };
            self.kind = AnnotKind::Other { subtype };
        }
        self
    }

    /// Rejects values a PDF cannot hold or that would make the writer misbehave.
    fn validate(&self) -> Result<(), EditError> {
        let invalid = |what: &'static str| Err(EditError::InvalidAnnotation(what));
        let p = &self.props;
        let r = p.rect;
        if ![r.left, r.bottom, r.right, r.top]
            .iter()
            .all(|v| coord_ok(*v))
            || r.left > r.right
            || r.bottom > r.top
        {
            return invalid("rectangle");
        }
        if !(0.0..=1.0).contains(&p.opacity) {
            return invalid("opacity");
        }
        if !(p.border_width.is_finite() && (0.0..=1000.0).contains(&p.border_width)) {
            return invalid("border width");
        }
        if !dash_ok(&p.dash) {
            return invalid("dash pattern");
        }
        if p.color.is_some_and(|c| !c.is_valid()) || p.interior.is_some_and(|c| !c.is_valid()) {
            return invalid("colour");
        }
        let texts = [&p.author, &p.contents, &p.name, &p.modified];
        if texts
            .iter()
            .any(|t| t.as_ref().is_some_and(|t| t.len() > MAX_TEXT_BYTES))
        {
            return invalid("text too long");
        }
        let finite = |pt: &PdfPoint| coord_ok(pt.x) && coord_ok(pt.y);
        match &self.kind {
            AnnotKind::Ink { strokes } => {
                if strokes.is_empty()
                    || strokes.len() > MAX_STROKES
                    || strokes
                        .iter()
                        .any(|s| s.is_empty() || s.len() > MAX_STROKE_POINTS)
                    || strokes.iter().map(Vec::len).sum::<usize>() > MAX_INK_POINTS
                {
                    return invalid("ink strokes");
                }
                if !strokes.iter().flatten().all(finite) {
                    return invalid("ink point");
                }
            }
            AnnotKind::Line { from, to, .. } if !(finite(from) && finite(to)) => {
                return invalid("line end point");
            }
            AnnotKind::Note {
                icon: NoteIcon::Other(name),
                ..
            } if !name_ok(name) => return invalid("note icon"),
            AnnotKind::Markup { quads, .. } => {
                if quads.is_empty() || quads.len() > MAX_QUADS {
                    return invalid("text markup");
                }
                if !quads.iter().flatten().all(finite) {
                    return invalid("text markup point");
                }
            }
            AnnotKind::Other { subtype } if subtype.is_empty() => return invalid("subtype"),
            _ => {}
        }
        Ok(())
    }

    /// For `Existing` annotations, the file's lock flags (ISO 32000-2 Table 167) are honoured:
    /// `Locked` forbids deleting or changing it, `LockedContents` forbids changing its text.
    fn lock_violation(&self, new: Option<&Self>) -> Option<&'static str> {
        let flags = self.props.flags;
        if let Some(new) = new {
            let lock_bits = AnnotFlags::LOCKED | AnnotFlags::LOCKED_CONTENTS;
            if flags.0 & lock_bits != new.props.flags.0 & lock_bits {
                // Changing a lock would make the inverse edit itself forbidden.
                return Some("lock flags cannot change");
            }
        }
        if matches!(self.origin, AnnotOrigin::New) {
            return None;
        }
        if flags.has(AnnotFlags::LOCKED) {
            return Some("locked");
        }
        match new {
            Some(new)
                if flags.has(AnnotFlags::LOCKED_CONTENTS)
                    && new.props.contents != self.props.contents =>
            {
                Some("contents locked")
            }
            _ => None,
        }
    }
}

fn coord_ok(v: f32) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

/// At most 16 finite, non-negative lengths, not all zero (ISO 32000-2 §8.4.3.6).
fn dash_ok(dash: &[f32]) -> bool {
    dash.len() <= 16
        && dash
            .iter()
            .all(|d| d.is_finite() && (0.0..=1000.0).contains(d))
        && (dash.is_empty() || dash.iter().any(|d| *d > 0.0))
}

/// A PDF name Ferrix will write: 1–127 bytes of printable ASCII without delimiters.
fn name_ok(name: &str) -> bool {
    (1..=127).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"()<>[]{}/%#".contains(&b))
}

/// A change to the annotations. Applying one returns the edit that undoes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotEdit {
    /// Adds `annot` as `id` at `index` in the page's annotation order (its z-order).
    Add {
        page: PageKey,
        index: usize,
        id: AnnotId,
        annot: Box<Annotation>,
    },
    Remove {
        id: AnnotId,
    },
    /// Replaces the whole annotation; the origin must stay the same.
    Set {
        id: AnnotId,
        annot: Box<Annotation>,
    },
}

/// Every annotation of the document being edited, grouped by page in z-order (back to front).
///
/// Pages are identified by [`PageKey`], so annotations follow their page when it moves, and stay
/// attached to it when it is removed (undoing the removal brings them back).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AnnotSet {
    pages: BTreeMap<PageKey, Vec<(AnnotId, Annotation)>>,
    pub(crate) next_id: u64,
    /// Pages whose annotations were read from the file. Only on these can a missing annotation
    /// mean "deleted"; elsewhere the writer leaves the file's annotations alone.
    #[serde(default)]
    loaded: BTreeSet<PageKey>,
}

impl AnnotSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds annotations read from the file to `page`, after any it already has. Not an edit:
    /// this builds the starting state. Invalid values are replaced ([`Annotation::sanitized`]),
    /// so undoing a deletion can always put the annotation back.
    pub fn insert_loaded(
        &mut self,
        page: PageKey,
        annots: impl IntoIterator<Item = Annotation>,
    ) -> Vec<AnnotId> {
        self.loaded.insert(page);
        let mut ids = Vec::new();
        for annot in annots {
            let id = self.new_id();
            self.pages
                .entry(page)
                .or_default()
                .push((id, annot.sanitized()));
            ids.push(id);
        }
        ids
    }

    /// Checks a set that came from outside, such as a recovery journal: every annotation
    /// valid, ids unique and below the id counter, and no file object listed twice.
    pub fn check(&self) -> Result<(), EditError> {
        let mut ids = HashSet::new();
        let mut objects = HashSet::new();
        for (id, annot) in self.pages.values().flatten() {
            if !ids.insert(*id) {
                return Err(EditError::DuplicateAnnotation(*id));
            }
            if id.0 >= self.next_id {
                return Err(EditError::InvalidAnnotation("id was not reserved"));
            }
            if let AnnotOrigin::Existing { source, object } = annot.origin
                && !objects.insert((source, object))
            {
                return Err(EditError::InvalidAnnotation("object already present"));
            }
            annot.validate()?;
        }
        Ok(())
    }

    /// Whether any page's annotations were read from the file. A set can be empty yet still
    /// mean something: "every annotation of these pages was deleted".
    pub fn has_loaded(&self) -> bool {
        !self.loaded.is_empty()
    }

    /// Whether `page`'s annotations were read from the file (see [`Self::insert_loaded`]).
    pub fn is_loaded(&self, page: PageKey) -> bool {
        self.loaded.contains(&page)
    }

    /// `id` followed by every reply to it, directly or through other replies, on its page.
    /// Delete these together: a reply whose parent is gone breaks the file (ISO 32000-2 §12.5.6.2).
    pub fn thread(&self, id: AnnotId) -> Vec<AnnotId> {
        let mut thread = vec![id];
        let Some((page, _)) = self.get(id) else {
            return thread;
        };
        let list = self.page(page);
        let mut i = 0;
        while i < thread.len() {
            if let Some((_, a)) = list.iter().find(|(x, _)| *x == thread[i])
                && let AnnotOrigin::Existing { object, .. } = a.origin
            {
                for (reply, r) in list {
                    if r.reply_to == Some(object) && !thread.contains(reply) {
                        thread.push(*reply);
                    }
                }
            }
            i += 1;
        }
        thread
    }

    /// Reserves a fresh id for an annotation about to be added.
    ///
    /// Ids are allocated outside the history, like page keys: edits never change the counter,
    /// which keeps every inverse exact. An `Add` must use an id reserved here.
    pub fn new_id(&mut self) -> AnnotId {
        let id = AnnotId(self.next_id);
        // 2^64 allocations can't happen; saturating keeps this panic-free regardless.
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// The annotations of `page`, back to front.
    pub fn page(&self, page: PageKey) -> &[(AnnotId, Annotation)] {
        self.pages.get(&page).map_or(&[], Vec::as_slice)
    }

    /// Pages that have at least one annotation.
    pub fn pages(&self) -> impl Iterator<Item = (PageKey, &[(AnnotId, Annotation)])> {
        self.pages
            .iter()
            .filter(|(_, list)| !list.is_empty())
            .map(|(key, list)| (*key, list.as_slice()))
    }

    /// The annotation `id` and the page it is on.
    pub fn get(&self, id: AnnotId) -> Option<(PageKey, &Annotation)> {
        let (page, index) = self.find(id)?;
        let (_, annot) = self.pages.get(&page)?.get(index)?;
        Some((page, annot))
    }

    pub fn len(&self) -> usize {
        self.pages.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The existing objects of `source` that are no longer in the set, i.e. were deleted.
    pub fn existing_objects(&self, source: SourceId) -> HashSet<ObjectRef> {
        self.pages
            .values()
            .flatten()
            .filter_map(|(_, a)| match a.origin {
                AnnotOrigin::Existing { source: s, object } if s == source => Some(object),
                _ => None,
            })
            .collect()
    }

    fn find(&self, id: AnnotId) -> Option<(PageKey, usize)> {
        self.pages.iter().find_map(|(page, list)| {
            list.iter()
                .position(|(i, _)| *i == id)
                .map(|index| (*page, index))
        })
    }

    /// Applies `edit`, returning its inverse. On error the set is left unchanged.
    ///
    /// Page existence is checked by [`crate::DocState`], which knows the page list.
    pub fn apply(&mut self, edit: &AnnotEdit) -> Result<AnnotEdit, EditError> {
        match edit {
            AnnotEdit::Add {
                page,
                index,
                id,
                annot,
            } => {
                if self.find(*id).is_some() {
                    return Err(EditError::DuplicateAnnotation(*id));
                }
                if let AnnotOrigin::Existing { source, object } = annot.origin
                    && self.existing_objects(source).contains(&object)
                {
                    // Two entries writing over the same object would make saving ambiguous.
                    return Err(EditError::InvalidAnnotation("object already present"));
                }
                if annot.origin == AnnotOrigin::New && matches!(annot.kind, AnnotKind::Other { .. })
                {
                    return Err(EditError::InvalidAnnotation("unsupported type"));
                }
                if id.0 >= self.next_id {
                    return Err(EditError::InvalidAnnotation("id was not reserved"));
                }
                annot.validate()?;
                if *index > self.page(*page).len() {
                    return Err(EditError::InvalidPosition(*index));
                }
                self.pages
                    .entry(*page)
                    .or_default()
                    .insert(*index, (*id, (**annot).clone()));
                Ok(AnnotEdit::Remove { id: *id })
            }
            AnnotEdit::Remove { id } => {
                let (page, index) = self.find(*id).ok_or(EditError::UnknownAnnotation(*id))?;
                if let Some((_, annot)) = self.page(page).get(index)
                    && let Some(why) = annot.lock_violation(None)
                {
                    return Err(EditError::InvalidAnnotation(why));
                }
                let list = self.pages.entry(page).or_default();
                let (_, annot) = list.remove(index);
                if list.is_empty() {
                    self.pages.remove(&page);
                }
                Ok(AnnotEdit::Add {
                    page,
                    index,
                    id: *id,
                    annot: Box::new(annot),
                })
            }
            AnnotEdit::Set { id, annot } => {
                let (page, index) = self.find(*id).ok_or(EditError::UnknownAnnotation(*id))?;
                annot.validate()?;
                let slot = &mut self.pages.entry(page).or_default()[index].1;
                if slot.origin != annot.origin || slot.reply_to != annot.reply_to {
                    return Err(EditError::InvalidAnnotation("origin cannot change"));
                }
                if matches!(slot.kind, AnnotKind::Other { .. }) {
                    // Ferrix doesn't model these, so it must never rewrite them (ADR-0009 §4).
                    return Err(EditError::InvalidAnnotation("this type cannot be edited"));
                }
                if std::mem::discriminant(&slot.kind) != std::mem::discriminant(&annot.kind) {
                    // Also keeps a Set from turning an annotation into `Other`, whose inverse
                    // would then be refused.
                    return Err(EditError::InvalidAnnotation("type cannot change"));
                }
                if let Some(why) = slot.lock_violation(Some(annot)) {
                    return Err(EditError::InvalidAnnotation(why));
                }
                let previous = std::mem::replace(slot, (**annot).clone());
                Ok(AnnotEdit::Set {
                    id: *id,
                    annot: Box::new(previous),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_rejects_tampered_sets() {
        let mut set = AnnotSet::new();
        let page = crate::PageList::identity(1).entries()[0].key;
        set.insert_loaded(
            page,
            [Annotation::new(AnnotKind::Square, PdfBox::default())],
        );
        assert_eq!(set.check(), Ok(()));
        set.next_id = 0;
        assert!(set.check().is_err(), "ids must be below the counter");
        set.next_id = 1;
        if let Some(list) = set.pages.get_mut(&page) {
            list[0].1.props.opacity = 7.0;
        }
        assert_eq!(set.check(), Err(EditError::InvalidAnnotation("opacity")));
    }
}
