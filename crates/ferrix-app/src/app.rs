//! Application state, update logic and view.
//!
//! Renders one whole page per request. The tiled, continuously scrolling canvas arrives with
//! `ferrix-render` in M1.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ferrix_engine::{DocInfo, DocSource, EngineError, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PixelRect, Rotation, Size};
use iced::{
    Alignment, Element, Event, Font, Length, Subscription, Task, Theme, event, font, keyboard,
    keyboard::key::Named,
    system,
    widget::{
        Space, button, column, container, image, mouse_area, operation, pick_list, row, scrollable,
        svg, text, text_input, tooltip,
    },
    window,
};

use crate::{
    icon::Icon,
    theme::{self, Tokens},
    worker::{EngineUnavailable, EngineWorker},
};

/// Logical pixels per PDF point at 100% zoom (96 dpi screens, 72 points per inch).
const LOGICAL_PX_PER_POINT: f32 = 96.0 / 72.0;
const ZOOM_STEPS: [f32; 15] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0,
];
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 4.0;
/// "Automatic" zoom fits the width but never enlarges beyond this (as pdf.js does).
const MAX_AUTO_ZOOM: f32 = 1.25;
/// Caps the longest edge of a rendered page (~200 MB of pixels for Letter). The M1 canvas
/// renders only visible tiles and removes this limit.
const MAX_RENDER_EDGE: f32 = 8192.0;
/// Pages are shown as a grid of tiles this size. Each stays under iced's 2 MiB synchronous
/// upload limit, so a new page appears in the same frame instead of flashing blank.
const TILE_EDGE: u32 = 512;

const TOOLBAR_HEIGHT: f32 = 46.0;
const STATUS_HEIGHT: f32 = 26.0;
const SIDEBAR_WIDTH: f32 = 188.0;
const THUMB_WIDTH: f32 = 112.0;
/// Thumbnails beyond this are not rendered until the virtualized sidebar lands in M1.
const MAX_THUMBNAILS: u32 = 500;
const PAGE_MARGIN: f32 = 28.0;
const SCROLLBAR_ALLOWANCE: f32 = 12.0;
/// Space for the macOS traffic lights, which sit over our toolbar.
const TRAFFIC_LIGHTS: f32 = if cfg!(target_os = "macos") { 72.0 } else { 0.0 };

const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

type Worker = EngineWorker<PdfiumEngine>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    Auto,
    FitWidth,
    FitPage,
    Fixed(f32),
}

impl Zoom {
    /// The zoom factor (1.0 = 100%) for a page of `page` points in a viewport of `canvas`
    /// logical pixels.
    fn resolve(self, page: Size, canvas: iced::Size) -> f32 {
        let fit_width = canvas.width / (page.width * LOGICAL_PX_PER_POINT);
        let fit_page = fit_width.min(canvas.height / (page.height * LOGICAL_PX_PER_POINT));
        let zoom = match self {
            Self::Auto => fit_width.min(MAX_AUTO_ZOOM),
            Self::FitWidth => fit_width,
            Self::FitPage => fit_page,
            Self::Fixed(zoom) => zoom,
        };
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    }
}

/// Entries of the zoom menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomChoice {
    FitWidth,
    FitPage,
    Percent(u16),
}

const ZOOM_CHOICES: [ZoomChoice; 10] = [
    ZoomChoice::FitWidth,
    ZoomChoice::FitPage,
    ZoomChoice::Percent(50),
    ZoomChoice::Percent(75),
    ZoomChoice::Percent(100),
    ZoomChoice::Percent(125),
    ZoomChoice::Percent(150),
    ZoomChoice::Percent(200),
    ZoomChoice::Percent(300),
    ZoomChoice::Percent(400),
];

impl fmt::Display for ZoomChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FitWidth => f.write_str("Fit width"),
            Self::FitPage => f.write_str("Fit page"),
            Self::Percent(p) => write!(f, "{p}%"),
        }
    }
}

#[derive(Debug)]
pub struct App {
    engine: Result<Worker, String>,
    /// Generation of the newest page render; the engine thread skips older queued requests.
    latest_render: Arc<AtomicU64>,
    window: Option<window::Id>,
    window_size: iced::Size,
    scale_factor: f32,
    appearance: Appearance,
    system_dark: bool,
    sidebar: bool,
    zoom: Zoom,
    doc: Option<OpenDoc>,
    page_input: String,
    page_image: Option<PageImage>,
    status: Option<String>,
}

#[derive(Debug)]
struct OpenDoc {
    path: PathBuf,
    info: DocInfo,
    page: PageIndex,
    /// Size and zoom of the page currently on screen.
    shown: Option<(Size, f32)>,
    thumbnails: Vec<Option<PageImage>>,
}

/// A rendered page as rows of tiles, each with its display size in logical pixels.
#[derive(Debug)]
struct PageImage {
    rows: Vec<Vec<(image::Handle, iced::Size)>>,
    logical: iced::Size,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenDialog,
    Open(PathBuf),
    DialogClosed,
    Opened(PathBuf, Result<DocInfo, String>),
    GoTo(u32),
    Next,
    Previous,
    First,
    Last,
    PageInput(String),
    PageSubmit,
    ZoomIn,
    ZoomOut,
    ZoomPicked(ZoomChoice),
    FitWidth,
    FitPage,
    ToggleSidebar,
    ToggleTheme,
    SystemTheme(iced::theme::Mode),
    Rendered(Result<Option<Rendered>, String>),
    Thumbnail(DocId, u32, Result<Rendered, String>),
    WindowOpened(Option<window::Id>),
    ScaleFactor(f32),
    Resized(iced::Size),
    DragWindow,
    DismissStatus,
}

#[derive(Debug, Clone)]
pub struct Rendered {
    /// Device-pixel width of the whole page.
    width: u32,
    rows: Vec<Vec<RawTile>>,
    logical: iced::Size,
    page_size: Size,
    zoom: f32,
}

#[derive(Debug, Clone)]
struct RawTile {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Rendered {
    fn into_image(self) -> PageImage {
        let px_per_logical = self.width as f32 / self.logical.width;
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|t| {
                        let size = iced::Size::new(
                            t.width as f32 / px_per_logical,
                            t.height as f32 / px_per_logical,
                        );
                        (image::Handle::from_rgba(t.width, t.height, t.pixels), size)
                    })
                    .collect()
            })
            .collect();
        PageImage {
            rows,
            logical: self.logical,
        }
    }
}

fn page_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("page")
}

fn sidebar_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("sidebar")
}

impl App {
    pub(crate) fn new(
        engine: Result<Worker, String>,
        initial: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        let status = engine.as_ref().err().cloned();
        let app = Self {
            engine,
            latest_render: Arc::new(AtomicU64::new(0)),
            window: None,
            window_size: iced::Size::new(1180.0, 860.0),
            scale_factor: 1.0,
            appearance: Appearance::System,
            system_dark: false,
            sidebar: true,
            zoom: Zoom::Auto,
            doc: None,
            page_input: String::new(),
            page_image: None,
            status,
        };
        let open = initial.map_or_else(Task::none, |path| Task::done(Message::Open(path)));
        let boot = Task::batch([
            window::oldest().map(Message::WindowOpened),
            system::theme().map(Message::SystemTheme),
        ]);
        (app, boot.chain(open))
    }

    pub(crate) fn title(&self) -> String {
        match &self.doc {
            Some(doc) => format!("{} — Ferrix", doc_name(doc)),
            None => "Ferrix".into(),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        let dark = match self.appearance {
            Appearance::System => self.system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        };
        if dark { theme::dark() } else { theme::light() }
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
                let thumbs = info.page_count.min(MAX_THUMBNAILS) as usize;
                let id = info.id;
                self.doc = Some(OpenDoc {
                    path,
                    info,
                    page: PageIndex(0),
                    shown: None,
                    thumbnails: (0..thumbs).map(|_| None).collect(),
                });
                self.sync_page_input();
                return Task::batch([close, self.render(), self.render_thumbnail(id, 0)]);
            }
            Message::Opened(path, Err(e)) => {
                self.status = Some(format!("Couldn’t open “{}”: {e}", file_name(&path)));
            }
            Message::GoTo(page) => return self.go_to(|_, _| page),
            Message::Next => return self.go_to(|page, count| (page + 1).min(count - 1)),
            Message::Previous => return self.go_to(|page, _| page.saturating_sub(1)),
            Message::First => return self.go_to(|_, _| 0),
            Message::Last => return self.go_to(|_, count| count - 1),
            Message::PageInput(value) => {
                self.page_input = value.chars().filter(char::is_ascii_digit).take(6).collect();
            }
            Message::PageSubmit => {
                let target = self.page_input.parse::<u32>().ok();
                let task = match target {
                    Some(n) if n >= 1 => self.go_to(|_, count| (n - 1).min(count - 1)),
                    _ => Task::none(),
                };
                self.sync_page_input();
                return task;
            }
            Message::ZoomIn => return self.step_zoom(true),
            Message::ZoomOut => return self.step_zoom(false),
            Message::ZoomPicked(choice) => {
                return self.set_zoom(match choice {
                    ZoomChoice::FitWidth => Zoom::FitWidth,
                    ZoomChoice::FitPage => Zoom::FitPage,
                    ZoomChoice::Percent(p) => Zoom::Fixed(f32::from(p) / 100.0),
                });
            }
            Message::FitWidth => return self.set_zoom(Zoom::FitWidth),
            Message::FitPage => return self.set_zoom(Zoom::FitPage),
            Message::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                return self.rerender_if_fitting();
            }
            Message::ToggleTheme => {
                let dark = self.theme().extended_palette().is_dark;
                self.appearance = if dark {
                    Appearance::Light
                } else {
                    Appearance::Dark
                };
            }
            Message::SystemTheme(mode) => self.system_dark = mode == iced::theme::Mode::Dark,
            Message::Rendered(Ok(None)) => {} // superseded by a newer request
            Message::Rendered(Ok(Some(r))) => {
                if let Some(doc) = &mut self.doc {
                    doc.shown = Some((r.page_size, r.zoom));
                }
                self.page_image = Some(r.into_image());
            }
            Message::Rendered(Err(e)) => {
                self.status = Some(format!("Couldn’t render this page: {e}"))
            }
            Message::Thumbnail(id, page, result) => {
                let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == id) else {
                    return Task::none(); // document was closed meanwhile
                };
                if let (Ok(r), Some(slot)) = (result, doc.thumbnails.get_mut(page as usize)) {
                    *slot = Some(r.into_image());
                }
                let next = page + 1;
                if (next as usize) < doc.thumbnails.len() {
                    return self.render_thumbnail(id, next);
                }
            }
            Message::WindowOpened(Some(id)) => {
                self.window = Some(id);
                return Task::batch([
                    window::scale_factor(id).map(Message::ScaleFactor),
                    window::size(id).map(Message::Resized),
                ]);
            }
            Message::WindowOpened(None) => {}
            Message::ScaleFactor(factor) => {
                if factor.is_finite() && factor > 0.0 && factor != self.scale_factor {
                    self.scale_factor = factor;
                    return self.render();
                }
            }
            Message::Resized(size) => {
                self.window_size = size;
                return self.rerender_if_fitting();
            }
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::DismissStatus => self.status = None,
        }
        Task::none()
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            system::theme_changes().map(Message::SystemTheme),
            event::listen_with(|event, status, _window| match event {
                Event::Window(window::Event::FileDropped(path)) => Some(Message::Open(path)),
                Event::Window(window::Event::Rescaled(factor)) => {
                    Some(Message::ScaleFactor(factor))
                }
                Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored =>
                {
                    shortcut(&key, modifiers)
                }
                _ => None,
            }),
        ])
    }

    // ── View ──────────────────────────────────────────────────────────────────

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let main: Element<'_, Message> = match (&self.doc, &self.page_image) {
            (None, _) => self.empty_state(),
            (Some(_), Some(page)) => self.page_view(page),
            (Some(_), None) => container(muted("Loading…", 13.0))
                .center(Length::Fill)
                .style(theme::canvas)
                .into(),
        };

        let body: Element<'_, Message> = match &self.doc {
            Some(doc) if self.sidebar => {
                row![self.sidebar_view(doc), vertical_divider(), main].into()
            }
            _ => main,
        };

        column![
            self.toolbar(),
            horizontal_divider(),
            container(body).height(Length::Fill),
            horizontal_divider(),
            self.status_bar(),
        ]
        .into()
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let doc = self.doc.as_ref().filter(|d| d.info.page_count > 0);
        let (page, count) = doc.map_or((0, 0), |d| (d.page.0, d.info.page_count));
        let has_doc = doc.is_some();

        let title: Element<'_, Message> = match &self.doc {
            Some(doc) => text(doc_name(doc))
                .size(13)
                .font(SEMIBOLD)
                .wrapping(text::Wrapping::None)
                .into(),
            None => text("Ferrix").size(13).font(SEMIBOLD).into(),
        };
        let left = row![
            Space::new().width(TRAFFIC_LIGHTS),
            icon_button(
                Icon::PanelLeft,
                "Toggle sidebar",
                has_doc.then_some(Message::ToggleSidebar),
                self.sidebar && has_doc
            ),
            icon_button(
                Icon::FolderOpen,
                "Open… (⌘O)",
                Some(Message::OpenDialog),
                false
            ),
            Space::new().width(6),
            title,
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let page_field = text_input("", if has_doc { &self.page_input } else { "" })
            .on_input_maybe(has_doc.then_some(Message::PageInput))
            .on_submit(Message::PageSubmit)
            .size(13)
            .width(46)
            .padding([4, 6])
            .align_x(Alignment::Center)
            .style(theme::input);
        let center = row![
            icon_button(
                Icon::ChevronLeft,
                "Previous page",
                (has_doc && page > 0).then_some(Message::Previous),
                false
            ),
            page_field,
            muted(
                if has_doc {
                    format!("of {count}")
                } else {
                    "of –".into()
                },
                13.0
            ),
            icon_button(
                Icon::ChevronRight,
                "Next page",
                (has_doc && page + 1 < count).then_some(Message::Next),
                false
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let zoom = doc.and_then(|d| d.shown).map(|(_, z)| z);
        let zoom_label = zoom.map(|z| ZoomChoice::Percent((z * 100.0).round() as u16));
        let theme_icon = if self.theme().extended_palette().is_dark {
            Icon::Sun
        } else {
            Icon::Moon
        };
        let right = row![
            icon_button(
                Icon::ZoomOut,
                "Zoom out (⌘−)",
                (has_doc && zoom.is_some_and(|z| z > MIN_ZOOM + 0.001)).then_some(Message::ZoomOut),
                false
            ),
            pick_list(ZOOM_CHOICES, zoom_label, Message::ZoomPicked)
                .placeholder("–")
                .text_size(13)
                .padding([4, 8])
                .width(92)
                .style(theme::select)
                .menu_style(theme::select_menu),
            icon_button(
                Icon::ZoomIn,
                "Zoom in (⌘+)",
                (has_doc && zoom.is_some_and(|z| z < MAX_ZOOM - 0.001)).then_some(Message::ZoomIn),
                false
            ),
            toolbar_separator(),
            icon_button(
                Icon::FitWidth,
                "Fit width",
                has_doc.then_some(Message::FitWidth),
                has_doc && self.zoom == Zoom::FitWidth
            ),
            icon_button(
                Icon::FitPage,
                "Fit page",
                has_doc.then_some(Message::FitPage),
                has_doc && self.zoom == Zoom::FitPage
            ),
            toolbar_separator(),
            icon_button(
                theme_icon,
                "Toggle light/dark",
                Some(Message::ToggleTheme),
                false
            ),
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let bar = container(
            row![
                container(left).width(Length::Fill),
                center,
                container(right).width(Length::Fill).align_x(Alignment::End),
            ]
            .align_y(Alignment::Center),
        )
        .height(TOOLBAR_HEIGHT)
        .padding([0, 8])
        .center_y(TOOLBAR_HEIGHT)
        .style(theme::chrome);

        // With the macOS title bar merged into the toolbar, the toolbar must move the window.
        if cfg!(target_os = "macos") {
            mouse_area(bar).on_press(Message::DragWindow).into()
        } else {
            bar.into()
        }
    }

    fn sidebar_view<'a>(&'a self, doc: &'a OpenDoc) -> Element<'a, Message> {
        let thumbs = doc.thumbnails.iter().enumerate().map(|(i, thumb)| {
            let selected = i as u32 == doc.page.0;
            let paper: Element<'_, Message> = match thumb {
                Some(t) => tiles(t),
                None => Space::new()
                    .width(THUMB_WIDTH)
                    .height(THUMB_WIDTH * 1.3)
                    .into(),
            };
            let label = text((i + 1).to_string())
                .size(12)
                .style(move |theme: &Theme| {
                    let t = theme::tokens(theme);
                    text::Style {
                        color: Some(if selected { t.accent } else { t.text_muted }),
                    }
                });
            let ring = container(container(paper).style(theme::page))
                .padding(4)
                .style(move |theme| theme::thumbnail_ring(theme, selected));
            button(column![ring, label].spacing(6).align_x(Alignment::Center))
                .on_press(Message::GoTo(i as u32))
                .padding(0)
                .style(theme::plain)
                .into()
        });

        let list = column(thumbs)
            .spacing(14)
            .padding([16, 0])
            .width(Length::Fill)
            .align_x(Alignment::Center);
        container(
            scrollable(list)
                .direction(scrollable::Direction::Vertical(thin_scrollbar()))
                .id(sidebar_scroll_id())
                .height(Length::Fill)
                .style(theme::scrollbar),
        )
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .style(theme::chrome)
        .into()
    }

    fn page_view<'a>(&self, page: &'a PageImage) -> Element<'a, Message> {
        let paper = container(tiles(page)).style(theme::page);
        let canvas = self.canvas_size();
        let fits_x = page.logical.width <= canvas.width + SCROLLBAR_ALLOWANCE;
        let fits_y = page.logical.height <= canvas.height;

        let view: Element<'a, Message> = if fits_x && fits_y {
            container(paper).center(Length::Fill).into()
        } else {
            let content = container(paper).padding(PAGE_MARGIN);
            let scroll = if fits_x {
                scrollable(content.center_x(Length::Fill))
                    .direction(scrollable::Direction::Vertical(thin_scrollbar()))
            } else {
                scrollable(content).direction(scrollable::Direction::Both {
                    vertical: thin_scrollbar(),
                    horizontal: thin_scrollbar(),
                })
            };
            scroll
                .id(page_scroll_id())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::scrollbar)
                .into()
        };
        container(view).style(theme::canvas).into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let failed = self.engine.is_err();
        let (icon, heading, body) = if failed {
            (
                Icon::Alert,
                "The PDF engine couldn’t start",
                "Ferrix needs the PDFium library. In a development checkout, run \
                 `cargo xtask fetch-pdfium`.",
            )
        } else {
            (
                Icon::FileText,
                "No document open",
                "Open a PDF to start reading, or drop a file anywhere in this window.",
            )
        };
        let badge = container(
            svg(icon.handle())
                .width(34)
                .height(34)
                .style(theme::icon_colored(if failed {
                    |t: &Tokens| t.danger
                } else {
                    |t: &Tokens| t.accent
                })),
        )
        .padding(21)
        .style(theme::badge);

        let open = button(
            row![
                svg(Icon::FolderOpen.handle())
                    .width(16)
                    .height(16)
                    .style(theme::icon_colored(|t| t.on_accent)),
                text("Open PDF…").size(14).font(SEMIBOLD),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([9, 18])
        .style(theme::primary)
        .on_press_maybe((!failed).then_some(Message::OpenDialog));

        let shortcut_hint = row![
            kbd(if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Ctrl"
            }),
            kbd("O"),
        ]
        .spacing(4);

        let content = column![
            badge,
            Space::new().height(4),
            text(heading).size(20).font(SEMIBOLD),
            container(muted(body, 14.0).align_x(Alignment::Center)).max_width(380),
            Space::new().height(6),
            open,
            shortcut_hint,
        ]
        .spacing(10)
        .align_x(Alignment::Center);

        container(content)
            .center(Length::Fill)
            .style(theme::canvas)
            .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let left: Element<'_, Message> = match (&self.status, &self.doc) {
            (Some(status), _) => button(
                row![
                    svg(Icon::Alert.handle())
                        .width(14)
                        .height(14)
                        .style(theme::icon_colored(|t| t.danger)),
                    text(status).size(12).style(|theme: &Theme| text::Style {
                        color: Some(theme::tokens(theme).danger),
                    }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(0)
            .style(theme::plain)
            .on_press(Message::DismissStatus)
            .into(),
            (None, Some(doc)) => {
                let mut parts = vec![format!(
                    "{} page{}",
                    doc.info.page_count,
                    if doc.info.page_count == 1 { "" } else { "s" }
                )];
                if let Some(version) = &doc.info.pdf_version {
                    parts.push(format!("PDF {version}"));
                }
                if let Some((size, _)) = doc.shown {
                    parts.push(format!("{:.0} × {:.0} pt", size.width, size.height));
                }
                muted(parts.join("  ·  "), 12.0).into()
            }
            (None, None) => muted("Ready", 12.0).into(),
        };
        let right = match &self.doc {
            Some(doc) if doc.info.page_count > 0 => {
                format!("Page {} of {}", doc.page.number(), doc.info.page_count)
            }
            _ => String::new(),
        };
        container(
            row![container(left).width(Length::Fill), muted(right, 12.0)]
                .align_y(Alignment::Center),
        )
        .height(STATUS_HEIGHT)
        .padding([0, 12])
        .center_y(STATUS_HEIGHT)
        .style(theme::chrome)
        .into()
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    /// Logical size available to a page inside the canvas.
    fn canvas_size(&self) -> iced::Size {
        let sidebar = if self.sidebar && self.doc.is_some() {
            SIDEBAR_WIDTH + 1.0
        } else {
            0.0
        };
        iced::Size::new(
            (self.window_size.width - sidebar - 2.0 * PAGE_MARGIN - SCROLLBAR_ALLOWANCE).max(64.0),
            (self.window_size.height - TOOLBAR_HEIGHT - STATUS_HEIGHT - 2.0 - 2.0 * PAGE_MARGIN)
                .max(64.0),
        )
    }

    fn sync_page_input(&mut self) {
        self.page_input = self
            .doc
            .as_ref()
            .map(|d| d.page.number().to_string())
            .unwrap_or_default();
    }

    fn set_zoom(&mut self, zoom: Zoom) -> Task<Message> {
        if self.doc.is_none() {
            return Task::none();
        }
        self.zoom = zoom;
        self.render()
    }

    fn step_zoom(&mut self, up: bool) -> Task<Message> {
        let Some((_, current)) = self.doc.as_ref().and_then(|d| d.shown) else {
            return Task::none();
        };
        let next = if up {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|&z| z > current * 1.01)
                .unwrap_or(MAX_ZOOM)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|&z| z < current * 0.99)
                .unwrap_or(MIN_ZOOM)
        };
        self.set_zoom(Zoom::Fixed(next))
    }

    fn rerender_if_fitting(&mut self) -> Task<Message> {
        match self.zoom {
            Zoom::Fixed(_) => Task::none(),
            Zoom::Auto | Zoom::FitWidth | Zoom::FitPage => self.render(),
        }
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
        let follow = if doc.info.page_count > 1 {
            page.0 as f32 / (doc.info.page_count - 1) as f32
        } else {
            0.0
        };
        self.sync_page_input();
        Task::batch([
            self.render(),
            operation::snap_to(page_scroll_id(), scrollable::RelativeOffset::START),
            operation::snap_to(
                sidebar_scroll_id(),
                scrollable::RelativeOffset { x: 0.0, y: follow },
            ),
        ])
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

    /// Closes the current document on the engine thread, without waiting for it.
    fn close_document(&mut self) -> Task<Message> {
        self.page_image = None;
        let (Some(doc), Ok(engine)) = (self.doc.take(), self.engine.clone()) else {
            return Task::none();
        };
        let id = doc.info.id;
        Task::future(async move { engine.call(move |e| e.close(id)).await }).discard()
    }

    /// Requests the current page at the current zoom and display scale.
    fn render(&mut self) -> Task<Message> {
        let (Ok(engine), Some(doc)) = (self.engine.clone(), &self.doc) else {
            return Task::none();
        };
        if doc.info.page_count == 0 {
            return Task::none();
        }
        let generation = self.latest_render.fetch_add(1, Ordering::Relaxed) + 1;
        let latest = Arc::clone(&self.latest_render);
        let (id, page, zoom_mode) = (doc.info.id, doc.page, self.zoom);
        let (canvas, scale_factor) = (self.canvas_size(), self.scale_factor);

        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        if latest.load(Ordering::Relaxed) != generation {
                            return Ok(None);
                        }
                        let size = e.page_info(id, page)?.size;
                        let zoom = zoom_mode.resolve(size, canvas);
                        let logical_scale = zoom * LOGICAL_PX_PER_POINT;
                        render_page(e, id, page, size, logical_scale, scale_factor, zoom).map(Some)
                    })
                    .await;
                flatten(result)
            },
            Message::Rendered,
        )
    }

    fn render_thumbnail(&self, id: DocId, page: u32) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        let scale_factor = self.scale_factor.max(2.0);
        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        let size = e.page_info(id, PageIndex(page))?.size;
                        let logical_scale = THUMB_WIDTH / size.width;
                        render_page(
                            e,
                            id,
                            PageIndex(page),
                            size,
                            logical_scale,
                            scale_factor,
                            0.0,
                        )
                    })
                    .await;
                flatten(result)
            },
            move |result| Message::Thumbnail(id, page, result),
        )
    }
}

/// Renders a whole page at `logical_scale` logical px per point on a `scale_factor` display.
fn render_page(
    engine: &mut PdfiumEngine,
    doc: DocId,
    page: PageIndex,
    size: Size,
    logical_scale: f32,
    scale_factor: f32,
    zoom: f32,
) -> Result<Rendered, EngineError> {
    let wanted = logical_scale * scale_factor;
    let longest = size.width.max(size.height) * wanted;
    let scale = wanted * (MAX_RENDER_EDGE / longest).min(1.0);
    let (width, height) = size.to_pixels(scale);
    // One engine call for the whole page (PDFium re-interprets the page per call), then slice.
    let page_pixels = engine.render_tile(&TileRequest {
        doc,
        page,
        scale,
        rotation: Rotation::R0,
        region: PixelRect::new(0, 0, width, height),
        flags: RenderFlags::default(),
    })?;
    let (width, height) = (page_pixels.region.width, page_pixels.region.height);
    Ok(Rendered {
        width,
        rows: slice_tiles(&page_pixels.pixels, width, height),
        logical: iced::Size::new(size.width * logical_scale, size.height * logical_scale),
        page_size: size,
        zoom,
    })
}

/// Splits a tightly packed RGBA image into rows of tiles of at most [`TILE_EDGE`] pixels.
fn slice_tiles(pixels: &[u8], width: u32, height: u32) -> Vec<Vec<RawTile>> {
    let stride = width as usize * 4;
    (0..height)
        .step_by(TILE_EDGE as usize)
        .map(|y| {
            let tile_h = TILE_EDGE.min(height - y);
            (0..width)
                .step_by(TILE_EDGE as usize)
                .map(|x| {
                    let tile_w = TILE_EDGE.min(width - x);
                    let mut tile = Vec::with_capacity(tile_w as usize * tile_h as usize * 4);
                    for row in y..y + tile_h {
                        let start = row as usize * stride + x as usize * 4;
                        tile.extend_from_slice(&pixels[start..start + tile_w as usize * 4]);
                    }
                    RawTile {
                        width: tile_w,
                        height: tile_h,
                        pixels: tile,
                    }
                })
                .collect()
        })
        .collect()
}

// ── Small widgets ─────────────────────────────────────────────────────────────

fn thin_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(8)
        .scroller_width(6)
        .margin(2)
}

/// Lays out a page's tiles edge to edge.
fn tiles(page: &PageImage) -> Element<'_, Message> {
    column(page.rows.iter().map(|tiles| {
        row(tiles.iter().map(|(handle, size)| {
            image(handle.clone())
                .width(size.width)
                .height(size.height)
                .into()
        }))
        .into()
    }))
    .width(page.logical.width)
    .height(page.logical.height)
    .into()
}

fn icon_button<'a>(
    icon: Icon,
    label: &'a str,
    on_press: Option<Message>,
    active: bool,
) -> Element<'a, Message> {
    let status = if on_press.is_some() {
        button::Status::Active
    } else {
        button::Status::Disabled
    };
    let glyph = svg(icon.handle())
        .width(17)
        .height(17)
        .style(theme::icon_for(status, active));
    let button = button(container(glyph).center(30))
        .padding(0)
        .on_press_maybe(on_press)
        .style(if active {
            theme::ghost_active
        } else {
            theme::ghost
        });
    tooltip(
        button,
        container(text(label).size(12))
            .padding([4, 8])
            .style(theme::tooltip),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

fn muted<'a>(content: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(content).size(size).style(|theme: &Theme| text::Style {
        color: Some(theme::tokens(theme).text_muted),
    })
}

fn kbd(key: &str) -> Element<'_, Message> {
    container(text(key).size(11))
        .padding([2, 6])
        .style(theme::kbd)
        .into()
}

fn horizontal_divider<'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .style(theme::divider)
        .into()
}

fn vertical_divider<'a>() -> Element<'a, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(theme::divider)
        .into()
}

fn toolbar_separator<'a>() -> Element<'a, Message> {
    container(container(Space::new().width(1).height(18)).style(theme::divider))
        .padding([0, 6])
        .into()
}

fn doc_name(doc: &OpenDoc) -> String {
    doc.info
        .metadata
        .title
        .clone()
        .unwrap_or_else(|| file_name(&doc.path))
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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
            "0" => Some(Message::ZoomPicked(ZoomChoice::Percent(100))),
            "1" => Some(Message::FitPage),
            "2" => Some(Message::FitWidth),
            "\\" => Some(Message::ToggleSidebar),
            _ => None,
        },
        _ => None,
    }
}

fn flatten<T>(result: Result<Result<T, EngineError>, EngineUnavailable>) -> Result<T, String> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Size = Size::new(612.0, 792.0);

    #[test]
    fn fit_modes_fill_the_canvas() {
        let canvas = iced::Size::new(816.0, 600.0); // exactly letter width at 100%
        assert!((Zoom::FitWidth.resolve(LETTER, canvas) - 1.0).abs() < 1e-4);
        let fit_page = Zoom::FitPage.resolve(LETTER, canvas);
        assert!((fit_page * 792.0 * LOGICAL_PX_PER_POINT - 600.0).abs() < 0.01);
    }

    #[test]
    fn tiles_cover_the_image_exactly() {
        let (w, h) = (1100, 600);
        let pixels: Vec<u8> = (0..w * h).flat_map(u32::to_le_bytes).collect();
        let rows = slice_tiles(&pixels, w, h);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].iter().map(|t| t.width).collect::<Vec<_>>(),
            [512, 512, 76]
        );
        assert_eq!(rows[1][0].height, 88);
        // The first pixel of tile (1, 2) is image pixel (1024, 512).
        let first = u32::from_le_bytes(rows[1][2].pixels[..4].try_into().expect("4 bytes"));
        assert_eq!(first, 512 * w + 1024);
    }

    #[test]
    fn auto_zoom_never_enlarges_past_the_cap() {
        let huge = iced::Size::new(4000.0, 4000.0);
        assert_eq!(Zoom::Auto.resolve(LETTER, huge), MAX_AUTO_ZOOM);
        assert_eq!(Zoom::Fixed(9.0).resolve(LETTER, huge), MAX_ZOOM);
    }
}
