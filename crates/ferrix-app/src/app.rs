//! The M0 application skeleton: open a PDF, page through it, zoom.
//!
//! Renders one whole page per request. The tiled, continuously scrolling canvas arrives with
//! `ferrix-render` in M1; this exists to prove the iced + engine wiring on every platform.

use std::path::PathBuf;

use ferrix_engine::{DocInfo, DocSource, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{PageIndex, PixelRect, Rotation};
use iced::{
    Alignment, Element, Event, Length, Subscription, Task, event, keyboard,
    keyboard::key::Named,
    widget::{button, column, container, image, row, scrollable, text},
    window,
};

use crate::worker::EngineWorker;

/// Logical pixels per PDF point at 100% zoom (96 dpi screens, 72 points per inch).
const LOGICAL_PX_PER_POINT: f32 = 96.0 / 72.0;
const ZOOM_STEPS: [f32; 16] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0,
];
const DEFAULT_ZOOM: usize = 7;
/// Whole-page images are capped at a size every GPU accepts as one texture.
const MAX_TEXTURE_EDGE: f32 = 8192.0;
const PAGE_MARGIN: f32 = 16.0;

type Worker = EngineWorker<PdfiumEngine>;

#[derive(Debug)]
pub(crate) struct App {
    engine: Result<Worker, String>,
    doc: Option<OpenDoc>,
    zoom_step: usize,
    scale_factor: f32,
    viewport: iced::Size,
    page_image: Option<PageImage>,
    /// Incremented per render request; results for older generations are dropped.
    generation: u64,
    status: Option<String>,
}

#[derive(Debug)]
struct OpenDoc {
    path: PathBuf,
    info: DocInfo,
    page: PageIndex,
}

#[derive(Debug)]
struct PageImage {
    handle: image::Handle,
    logical: iced::Size,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    OpenDialog,
    Open(PathBuf),
    DialogClosed,
    Opened(PathBuf, Result<DocInfo, String>),
    Next,
    Previous,
    First,
    Last,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Rendered(u64, Result<Rendered, String>),
    ScaleFactor(f32),
    Resized(iced::Size),
}

#[derive(Debug, Clone)]
pub(crate) struct Rendered {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// Display size in logical pixels; may be smaller than `width`×`height` on HiDPI screens,
    /// or larger when the render was capped at [`MAX_TEXTURE_EDGE`].
    logical: iced::Size,
}

impl App {
    pub(crate) fn new(
        engine: Result<Worker, String>,
        initial: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        let status = engine.as_ref().err().cloned();
        let app = Self {
            engine,
            doc: None,
            zoom_step: DEFAULT_ZOOM,
            scale_factor: 1.0,
            viewport: iced::Size::new(1024.0, 768.0),
            page_image: None,
            generation: 0,
            status,
        };
        let window_info = window::oldest().then(|id| match id {
            Some(id) => Task::batch([
                window::scale_factor(id).map(Message::ScaleFactor),
                window::size(id).map(Message::Resized),
            ]),
            None => Task::none(),
        });
        let open = initial.map_or_else(Task::none, |path| Task::done(Message::Open(path)));
        (app, Task::batch([window_info, open]))
    }

    pub(crate) fn title(&self) -> String {
        match &self.doc {
            Some(doc) => {
                let name = doc.info.metadata.title.clone().unwrap_or_else(|| {
                    doc.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                format!("{name} — Ferrix")
            }
            None => "Ferrix".into(),
        }
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenDialog => {
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .add_filter("PDF documents", &["pdf"])
                        .pick_file(),
                    |file| {
                        file.map_or(Message::DialogClosed, |f| {
                            Message::Open(f.path().to_path_buf())
                        })
                    },
                );
            }
            Message::DialogClosed => {}
            Message::Open(path) => return self.open(path),
            Message::Opened(path, Ok(info)) => {
                let close = self.close_document();
                self.status = (info.page_count == 0).then(|| "This document has no pages".into());
                self.doc = Some(OpenDoc {
                    path,
                    info,
                    page: PageIndex(0),
                });
                return Task::batch([close, self.render()]);
            }
            Message::Opened(path, Err(e)) => {
                self.status = Some(format!("Cannot open {}: {e}", path.display()));
            }
            Message::Next => return self.go_to(|page, count| (page + 1).min(count - 1)),
            Message::Previous => return self.go_to(|page, _| page.saturating_sub(1)),
            Message::First => return self.go_to(|_, _| 0),
            Message::Last => return self.go_to(|_, count| count - 1),
            Message::ZoomIn => {
                return self.set_zoom((self.zoom_step + 1).min(ZOOM_STEPS.len() - 1));
            }
            Message::ZoomOut => return self.set_zoom(self.zoom_step.saturating_sub(1)),
            Message::ZoomReset => return self.set_zoom(DEFAULT_ZOOM),
            Message::Rendered(generation, result) if generation == self.generation => {
                match result {
                    Ok(r) => {
                        tracing::debug!(width = r.width, height = r.height, "page rendered");
                        self.page_image = Some(PageImage {
                            logical: r.logical,
                            handle: image::Handle::from_rgba(r.width, r.height, r.pixels),
                        });
                    }
                    Err(e) => self.status = Some(format!("Cannot render page: {e}")),
                }
            }
            Message::Rendered(..) => {} // stale
            Message::ScaleFactor(factor) => {
                if factor.is_finite() && factor > 0.0 && factor != self.scale_factor {
                    self.scale_factor = factor;
                    return self.render();
                }
            }
            Message::Resized(size) => self.viewport = size,
        }
        Task::none()
    }

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let has_doc = self.doc.as_ref().is_some_and(|d| d.info.page_count > 0);
        let (page, count) = self
            .doc
            .as_ref()
            .map_or((0, 0), |d| (d.page.0, d.info.page_count));

        let toolbar = row![
            button("Open…").on_press(Message::OpenDialog),
            button("◀").on_press_maybe((has_doc && page > 0).then_some(Message::Previous)),
            text(if has_doc {
                format!("{} / {count}", page + 1)
            } else {
                "– / –".into()
            }),
            button("▶").on_press_maybe((has_doc && page + 1 < count).then_some(Message::Next)),
            button("−").on_press_maybe((has_doc && self.zoom_step > 0).then_some(Message::ZoomOut)),
            button(text(format!("{:.0}%", self.zoom() * 100.0)))
                .on_press_maybe(has_doc.then_some(Message::ZoomReset)),
            button("+").on_press_maybe(
                (has_doc && self.zoom_step + 1 < ZOOM_STEPS.len()).then_some(Message::ZoomIn),
            ),
        ]
        .spacing(8)
        .padding(8)
        .align_y(Alignment::Center);

        let body: Element<'_, Message> = match &self.page_image {
            Some(page) => self.page_view(page),
            None if self.doc.is_none() => container(
                text("Open a PDF with Ctrl/⌘+O, or drop a file onto this window").size(16),
            )
            .center(Length::Fill)
            .into(),
            None => container(text("Rendering…")).center(Length::Fill).into(),
        };

        let mut layout = column![toolbar, body];
        if let Some(status) = &self.status {
            layout = layout.push(container(text(status).size(13)).padding([4, 8]));
        }
        layout.into()
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        event::listen_with(|event, status, _window| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::Open(path)),
            Event::Window(window::Event::Rescaled(factor)) => Some(Message::ScaleFactor(factor)),
            Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if status == event::Status::Ignored =>
            {
                shortcut(&key, modifiers)
            }
            _ => None,
        })
    }

    fn page_view<'a>(&self, page: &'a PageImage) -> Element<'a, Message> {
        let content = container(
            image(page.handle.clone())
                .width(page.logical.width)
                .height(page.logical.height),
        )
        .padding(PAGE_MARGIN);

        // Center the page when it fits; scroll horizontally only when it doesn't.
        if page.logical.width + 2.0 * PAGE_MARGIN <= self.viewport.width {
            scrollable(content.center_x(Length::Fill))
                .height(Length::Fill)
                .into()
        } else {
            scrollable(content)
                .direction(scrollable::Direction::Both {
                    vertical: scrollable::Scrollbar::default(),
                    horizontal: scrollable::Scrollbar::default(),
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    fn zoom(&self) -> f32 {
        ZOOM_STEPS[self.zoom_step]
    }

    fn set_zoom(&mut self, step: usize) -> Task<Message> {
        if step == self.zoom_step || self.doc.is_none() {
            return Task::none();
        }
        self.zoom_step = step;
        self.render()
    }

    fn go_to(&mut self, target: impl FnOnce(u32, u32) -> u32) -> Task<Message> {
        let Some(doc) = self.doc.as_mut().filter(|d| d.info.page_count > 0) else {
            return Task::none();
        };
        let page = PageIndex(target(doc.page.0, doc.info.page_count));
        if page == doc.page {
            return Task::none();
        }
        doc.page = page;
        self.render()
    }

    fn open(&mut self, path: PathBuf) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        self.status = None;
        Task::perform(
            async move {
                let source = DocSource::Path(path.clone());
                let result = engine.call(move |e| e.open(source, None)).await;
                (path, flatten(result))
            },
            |(path, result)| Message::Opened(path, result),
        )
    }

    /// Requests the current page at the current zoom and display scale.
    fn render(&mut self) -> Task<Message> {
        let (Ok(engine), Some(doc)) = (self.engine.clone(), &self.doc) else {
            return Task::none();
        };
        if doc.info.page_count == 0 {
            return Task::none();
        }
        self.generation += 1;
        let generation = self.generation;
        let (id, page) = (doc.info.id, doc.page);
        let logical_scale = self.zoom() * LOGICAL_PX_PER_POINT;
        let wanted_scale = logical_scale * self.scale_factor;

        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        let size = e.page_info(id, page)?.size;
                        // Cap the render so the image fits in one GPU texture; the tiled
                        // canvas in M1 removes this limit.
                        let longest = size.width.max(size.height) * wanted_scale;
                        let scale = wanted_scale * (MAX_TEXTURE_EDGE / longest).min(1.0);
                        let (width, height) = size.to_pixels(scale);
                        let tile = e.render_tile(&TileRequest {
                            doc: id,
                            page,
                            scale,
                            rotation: Rotation::R0,
                            region: PixelRect::new(0, 0, width, height),
                            flags: RenderFlags::default(),
                        })?;
                        Ok(Rendered {
                            width: tile.region.width,
                            height: tile.region.height,
                            pixels: tile.pixels,
                            logical: iced::Size::new(
                                size.width * logical_scale,
                                size.height * logical_scale,
                            ),
                        })
                    })
                    .await;
                flatten(result)
            },
            move |result| Message::Rendered(generation, result),
        )
    }

    /// Closes the current document on the engine thread, without waiting for it.
    fn close_document(&mut self) -> Task<Message> {
        self.page_image = None;
        let (Some(doc), Ok(engine)) = (self.doc.take(), self.engine.clone()) else {
            return Task::none();
        };
        let id = doc.info.id;
        Task::future(async move { engine.call(move |e| e.close(id)).await }).discard()
    }
}

fn shortcut(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    use keyboard::Key;
    match key.as_ref() {
        Key::Named(Named::ArrowRight | Named::PageDown) => Some(Message::Next),
        Key::Named(Named::ArrowLeft | Named::PageUp) => Some(Message::Previous),
        Key::Named(Named::Home) => Some(Message::First),
        Key::Named(Named::End) => Some(Message::Last),
        Key::Character(c) if modifiers.command() => match c {
            "o" => Some(Message::OpenDialog),
            "=" | "+" => Some(Message::ZoomIn),
            "-" => Some(Message::ZoomOut),
            "0" => Some(Message::ZoomReset),
            _ => None,
        },
        _ => None,
    }
}

fn flatten<T>(
    result: Result<Result<T, ferrix_engine::EngineError>, crate::worker::EngineUnavailable>,
) -> Result<T, String> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}
