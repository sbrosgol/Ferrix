//! The layer over each page in Markup mode: turns pointer input into PDF-space messages and
//! draws what PDFium can't show yet: the shape being drawn, a moved or resized annotation, and
//! the selection with its handles. Committed annotations are rendered by PDFium from the
//! would-be-saved revision (ADR-0009 §7).

use ferrix_annot::{Appearance, PathEl};
use ferrix_core::{AnnotId, Annotation, PageKey, Rgb};
use ferrix_types::{PageTransform, PdfBox, PdfPoint, Point};
use iced::{
    Color, Rectangle, Renderer, Theme, mouse,
    widget::canvas::{self, Event, Frame, Geometry, LineDash, Path, Stroke, path::Builder},
};

use crate::{
    annotate_ui::AnnotMsg,
    app::Message,
    markup::{self, Gesture, Style, Tool},
    theme,
};

/// Pointer input on a page, in PDF user space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    Press(PdfPoint),
    Drag(PdfPoint),
    Release(PdfPoint),
}

/// One page's overlay.
#[derive(Debug)]
pub(crate) struct Overlay<'a> {
    pub(crate) page: PageKey,
    /// User space → page space (points), the page's extra rotation included.
    pub(crate) transform: PageTransform,
    /// Logical pixels per point.
    pub(crate) scale: f32,
    pub(crate) annots: &'a [(AnnotId, Annotation)],
    pub(crate) selected: Option<AnnotId>,
    pub(crate) gesture: Option<&'a Gesture>,
    pub(crate) tool: Tool,
    pub(crate) style: Style,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
}

#[derive(Debug, Default)]
pub(crate) struct State {
    pressed: bool,
}

impl Overlay<'_> {
    fn to_pdf(&self, bounds: Rectangle, position: iced::Point) -> PdfPoint {
        let local = Point::new(
            (position.x - bounds.x) / self.scale,
            (position.y - bounds.y) / self.scale,
        );
        self.transform.to_pdf(local)
    }

    fn to_px(&self, p: PdfPoint) -> iced::Point {
        let page = self.transform.to_page(p.x, p.y);
        iced::Point::new(page.x * self.scale, page.y * self.scale)
    }

    fn selection(&self) -> Option<&Annotation> {
        let id = self.selected?;
        self.annots.iter().find(|(i, _)| *i == id).map(|(_, a)| a)
    }

    /// What the gesture would produce if released now.
    fn draft(&self) -> Option<Annotation> {
        match self.gesture? {
            Gesture::Draw {
                start,
                current,
                points,
            } => markup::create(
                self.tool,
                self.style,
                &markup::Drag {
                    start: *start,
                    end: *current,
                    points,
                    shift: self.shift,
                    alt: self.alt,
                    min: 0.0,
                },
            ),
            Gesture::Move { start, current } => self
                .selection()
                .map(|a| markup::moved(a, current.x - start.x, current.y - start.y)),
            Gesture::Resize { handle, current } => self
                .selection()
                .map(|a| markup::resized(a, *handle, *current, self.shift)),
        }
    }

    fn draw_appearance(&self, frame: &mut Frame, ap: &Appearance) {
        for item in &ap.items {
            let mut builder = Builder::new();
            for el in &item.path {
                match *el {
                    PathEl::MoveTo(p) => builder.move_to(self.to_px(p)),
                    PathEl::LineTo(p) => builder.line_to(self.to_px(p)),
                    PathEl::CurveTo(a, b, c) => {
                        builder.bezier_curve_to(self.to_px(a), self.to_px(b), self.to_px(c));
                    }
                    PathEl::Close => builder.close(),
                }
            }
            let path = builder.build();
            if let Some(fill) = item.fill {
                frame.fill(&path, color(fill, ap.opacity));
            }
            if let Some(stroke) = &item.stroke {
                let cap = if stroke.round {
                    canvas::LineCap::Round
                } else {
                    canvas::LineCap::Butt
                };
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_color(color(stroke.color, ap.opacity))
                        .with_width((stroke.width * self.scale).max(1.0))
                        .with_line_cap(cap)
                        .with_line_join(canvas::LineJoin::Round),
                );
            }
        }
    }

    fn outline(&self, frame: &mut Frame, b: PdfBox, color: Color, dashed: bool) {
        let corners = [
            PdfPoint::new(b.left, b.bottom),
            PdfPoint::new(b.right, b.bottom),
            PdfPoint::new(b.right, b.top),
            PdfPoint::new(b.left, b.top),
        ];
        let path = Path::new(|p| {
            p.move_to(self.to_px(corners[0]));
            for c in &corners[1..] {
                p.line_to(self.to_px(*c));
            }
            p.close();
        });
        let dash = [4.0, 3.0];
        frame.stroke(
            &path,
            Stroke {
                line_dash: if dashed {
                    LineDash {
                        segments: &dash,
                        offset: 0,
                    }
                } else {
                    LineDash::default()
                },
                ..Stroke::default().with_color(color).with_width(1.0)
            },
        );
    }
}

fn color(c: Rgb, opacity: f32) -> Color {
    Color::from_rgba(c.r, c.g, c.b, opacity)
}

/// Handle size on screen (design system › Annotations: 8 px drawn, 16 px to grab).
const HANDLE: f32 = 8.0;

impl canvas::Program<Message> for Overlay<'_> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let page = self.page;
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor
                    .position_in(bounds)
                    .map(|p| iced::Point::new(p.x + bounds.x, p.y + bounds.y))?;
                state.pressed = true;
                let at = self.to_pdf(bounds, position);
                Some(
                    canvas::Action::publish(Message::Annot(AnnotMsg::Pointer(
                        page,
                        Pointer::Press(at),
                    )))
                    .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.pressed => {
                let at = self.to_pdf(bounds, *position);
                Some(
                    canvas::Action::publish(Message::Annot(AnnotMsg::Pointer(
                        page,
                        Pointer::Drag(at),
                    )))
                    .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.pressed => {
                state.pressed = false;
                let position = cursor.position()?;
                let at = self.to_pdf(bounds, position);
                Some(
                    canvas::Action::publish(Message::Annot(AnnotMsg::Pointer(
                        page,
                        Pointer::Release(at),
                    )))
                    .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let t = theme::tokens(theme);
        let draft = self.draft();
        if let Some(ap) = draft.as_ref().and_then(ferrix_annot::appearance) {
            self.draw_appearance(&mut frame, &ap);
        }
        // The selection: at its new place while moving or resizing.
        let selected = match self.gesture {
            Some(Gesture::Move { .. } | Gesture::Resize { .. }) => draft.clone(),
            _ => self.selection().cloned(),
        };
        if let Some(annot) = selected {
            let editable = markup::editable(&annot);
            let outline = if editable { t.accent } else { t.text_muted };
            self.outline(&mut frame, markup::bounds(&annot), outline, !editable);
            if editable {
                for (_, p) in markup::handles(&annot) {
                    let c = self.to_px(p);
                    let r = Path::rectangle(
                        iced::Point::new(c.x - HANDLE / 2.0, c.y - HANDLE / 2.0),
                        iced::Size::new(HANDLE, HANDLE),
                    );
                    frame.fill(&r, Color::WHITE);
                    frame.stroke(&r, Stroke::default().with_color(t.accent).with_width(1.0));
                }
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let Some(position) = cursor.position_in(bounds) else {
            return mouse::Interaction::default();
        };
        if self.tool.draws() {
            return mouse::Interaction::Crosshair;
        }
        let at = self.to_pdf(
            bounds,
            iced::Point::new(position.x + bounds.x, position.y + bounds.y),
        );
        match markup::hit(self.annots, at, 4.0 / self.scale) {
            Some(_) if matches!(self.gesture, Some(Gesture::Move { .. })) => {
                mouse::Interaction::Grabbing
            }
            Some(_) => mouse::Interaction::Grab,
            None => mouse::Interaction::default(),
        }
    }
}
