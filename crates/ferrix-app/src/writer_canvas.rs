//! The writing surface's overlay: drawn on top of the page images (which are ordinary image
//! widgets underneath), it shows the caret and selection and turns mouse, keyboard and
//! input-method events into [`Action`]s; the app does the editing.

use ferrix_compose::Clicks;
use iced::{
    Color, Element, Event, Length, Point, Rectangle, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget,
        input_method::{self, InputMethod},
        layout, mouse, renderer,
        text::{self, Renderer as _},
        widget::{Tree, tree},
    },
    keyboard, window,
};

use crate::{
    theme,
    writer::{Action, PAGE_GAP, key_command},
};

/// A page's position and content, in logical pixels.
pub(crate) struct Canvas<'a, Message> {
    page: Size,
    pages: usize,
    /// Logical pixels per point.
    zoom: f32,
    selection: Vec<(usize, Rectangle)>,
    caret: Option<(usize, Rectangle)>,
    focused: bool,
    /// Wider than the window: the canvas takes the pages' width and scrolls sideways.
    wide: bool,
    on_action: Box<dyn Fn(Action) -> Message + 'a>,
}

impl<'a, Message> Canvas<'a, Message> {
    pub(crate) fn new(
        page: Size,
        pages: usize,
        zoom: f32,
        on_action: impl Fn(Action) -> Message + 'a,
    ) -> Self {
        Self {
            page,
            pages,
            zoom,
            selection: Vec::new(),
            caret: None,
            focused: true,
            wide: false,
            on_action: Box::new(on_action),
        }
    }

    /// Selection rectangles, in points on their pages.
    pub(crate) fn selection(mut self, rects: Vec<(usize, ferrix_compose::Rect)>) -> Self {
        self.selection = rects
            .into_iter()
            .map(|(p, r)| (p, self.scaled(r)))
            .collect();
        self
    }

    /// The caret, in points on its page.
    pub(crate) fn caret(mut self, caret: Option<(usize, ferrix_compose::Rect)>) -> Self {
        self.caret = caret.map(|(p, r)| (p, self.scaled(r)));
        self
    }

    /// Whether keys go to the document (no dialog is open).
    pub(crate) fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub(crate) fn wide(mut self, wide: bool) -> Self {
        self.wide = wide;
        self
    }

    fn scaled(&self, r: ferrix_compose::Rect) -> Rectangle {
        Rectangle::new(
            Point::new(r.x * self.zoom, r.y * self.zoom),
            Size::new(r.width * self.zoom, r.height * self.zoom),
        )
    }

    fn content_height(&self) -> f32 {
        PAGE_GAP + self.pages as f32 * (self.page.height + PAGE_GAP)
    }

    /// Where page `index` is drawn inside `bounds`.
    fn page_bounds(&self, bounds: Rectangle, index: usize) -> Rectangle {
        let x = bounds.x + ((bounds.width - self.page.width) / 2.0).max(PAGE_GAP);
        let y = bounds.y + PAGE_GAP + index as f32 * (self.page.height + PAGE_GAP);
        Rectangle::new(Point::new(x, y), self.page)
    }

    /// The page nearest to a point, and the point in that page's coordinates (in points).
    fn locate(&self, bounds: Rectangle, point: Point) -> (usize, f32, f32) {
        let step = self.page.height + PAGE_GAP;
        let index = (((point.y - bounds.y - PAGE_GAP / 2.0) / step)
            .floor()
            .max(0.0) as usize)
            .min(self.pages.saturating_sub(1));
        let page = self.page_bounds(bounds, index);
        (
            index,
            (point.x - page.x) / self.zoom,
            (point.y - page.y) / self.zoom,
        )
    }
}

#[derive(Debug, Default)]
struct State {
    last_click: Option<mouse::Click>,
    dragging: bool,
    modifiers: keyboard::Modifiers,
    preedit: Option<input_method::Preedit>,
    window_focused: bool,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Canvas<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            window_focused: true,
            ..State::default()
        })
    }

    fn size(&self) -> Size<Length> {
        let width = if self.wide {
            Length::Fixed(self.page.width + 2.0 * PAGE_GAP)
        } else {
            Length::Fill
        };
        Size::new(width, Length::Fixed(self.content_height()))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let min = self.page.width + 2.0 * PAGE_GAP;
        let width = if self.wide {
            min
        } else {
            limits.max().width.max(min)
        };
        layout::Node::new(Size::new(width, self.content_height()))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Window(window::Event::Focused) => state.window_focused = true,
            Event::Window(window::Event::Unfocused) => state.window_focused = false,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                let click = mouse::Click::new(position, mouse::Button::Left, state.last_click);
                state.last_click = Some(click);
                state.dragging = true;
                let clicks = match click.kind() {
                    mouse::click::Kind::Single => Clicks::Single,
                    mouse::click::Kind::Double => Clicks::Double,
                    mouse::click::Kind::Triple => Clicks::Triple,
                };
                let (page, x, y) = self.locate(bounds, position);
                shell.publish((self.on_action)(Action::Press {
                    page,
                    x,
                    y,
                    clicks,
                    extend: state.modifiers.shift(),
                }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                let (page, x, y) = self.locate(bounds, *position);
                shell.publish((self.on_action)(Action::Drag { page, x, y }));
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                text,
                ..
            }) if self.focused => {
                if let Some(command) = key_command(key, *physical_key, *modifiers, text.as_deref())
                {
                    shell.publish((self.on_action)(Action::Key(command)));
                    shell.capture_event();
                }
            }
            Event::InputMethod(event) if self.focused => match event {
                input_method::Event::Opened => {
                    state.preedit = Some(input_method::Preedit::new());
                }
                input_method::Event::Closed => state.preedit = None,
                input_method::Event::Preedit(content, selection) => {
                    state.preedit = Some(input_method::Preedit {
                        content: content.clone(),
                        selection: selection.clone(),
                        text_size: None,
                    });
                    shell.request_redraw();
                }
                input_method::Event::Commit(content) => {
                    state.preedit = None;
                    shell.publish((self.on_action)(Action::Key(
                        crate::writer::KeyCommand::Insert(content.clone()),
                    )));
                    shell.capture_event();
                }
            },
            Event::Window(window::Event::RedrawRequested(_)) => {
                let method = match self.caret {
                    Some((page, rect)) if self.focused && state.window_focused => {
                        let origin = self.page_bounds(bounds, page).position();
                        InputMethod::Enabled {
                            cursor: Rectangle::new(
                                origin + Vector::new(rect.x, rect.y),
                                Size::new(1.0, rect.height),
                            ),
                            purpose: input_method::Purpose::Normal,
                            preedit: state.preedit.as_ref().map(input_method::Preedit::as_ref),
                        }
                    }
                    _ => InputMethod::Disabled,
                };
                shell.request_input_method(&method);
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let over_page =
            (0..self.pages).any(|i| cursor.is_over(self.page_bounds(layout.bounds(), i)));
        if over_page {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<State>();
        let tokens = theme::tokens(theme);
        let bounds = layout.bounds();

        let highlight = Color {
            a: 0.28,
            ..tokens.accent
        };
        for (index, rect) in &self.selection {
            let page = self.page_bounds(bounds, *index);
            let rect = Rectangle::new(page.position() + Vector::new(rect.x, rect.y), rect.size());
            if rect.intersects(viewport) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        ..renderer::Quad::default()
                    },
                    highlight,
                );
            }
        }

        if let Some((index, rect)) = self
            .caret
            .filter(|_| self.focused && self.selection.is_empty())
        {
            let origin = self.page_bounds(bounds, index).position();
            let caret = Rectangle::new(
                origin + Vector::new(rect.x - 0.75, rect.y),
                Size::new(1.5, rect.height),
            );
            renderer.fill_quad(
                renderer::Quad {
                    bounds: caret,
                    ..renderer::Quad::default()
                },
                Color::from_rgb8(0x1C, 0x1F, 0x24),
            );
            // Text being composed with an input method, shown at the caret until committed.
            if let Some(preedit) = state.preedit.as_ref().filter(|p| !p.content.is_empty()) {
                let size = (rect.height * 0.75).max(10.0);
                let position = caret.position() + Vector::new(2.0, 0.0);
                renderer.fill_text(
                    text::Text {
                        content: preedit.content.clone(),
                        bounds: Size::new(f32::INFINITY, rect.height),
                        size: size.into(),
                        line_height: text::LineHeight::Absolute(rect.height.into()),
                        font: iced::Font::DEFAULT,
                        align_x: text::Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: text::Wrapping::None,
                    },
                    position,
                    Color::from_rgb8(0x1C, 0x1F, 0x24),
                    *viewport,
                );
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle::new(
                            position + Vector::new(0.0, rect.height - 2.0),
                            Size::new(size * 0.6 * preedit.content.chars().count() as f32, 1.0),
                        ),
                        ..renderer::Quad::default()
                    },
                    tokens.accent,
                );
            }
        }
    }
}

impl<'a, Message: 'a> From<Canvas<'a, Message>> for Element<'a, Message> {
    fn from(canvas: Canvas<'a, Message>) -> Self {
        Element::new(canvas)
    }
}
