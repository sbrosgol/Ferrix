//! Application state and update logic. Layout lives in `view.rs`.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ferrix_convert::{Converter, Kind};
use ferrix_core::{Edit, PageKey};
use ferrix_engine::{DocInfo, DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_io::FileStamp;
use ferrix_pdf::{LayoutPage, PaperSize, Source as PdfSource};
use ferrix_print::{PrintJob, PrintOutcome};
use ferrix_types::{DocId, Rotation, Size};
use iced::{
    Event, Subscription, Task, Theme, event, keyboard,
    keyboard::key::Named,
    system,
    widget::{operation, scrollable},
    window,
};

use ferrix_convert::kind_of;

use crate::{
    document::{Click, Document, Origin, Thumbnail, file_name, pages_label},
    render::{LOGICAL_PX_PER_POINT, PageRef, Rendered, render_page},
    worker::{EngineWorker, blocking, flatten},
};

const ZOOM_STEPS: [f32; 15] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0,
];
pub(crate) const MIN_ZOOM: f32 = 0.1;
pub(crate) const MAX_ZOOM: f32 = 4.0;
/// "Automatic" zoom fits the width but never enlarges beyond this (as pdf.js does).
const MAX_AUTO_ZOOM: f32 = 1.25;
pub(crate) const THUMB_WIDTH: f32 = 112.0;

pub(crate) type Worker = EngineWorker<PdfiumEngine>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Zoom {
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

pub(crate) const ZOOM_CHOICES: [ZoomChoice; 10] = [
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

/// What to do once the user has dealt with unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Proceed {
    OpenDialog,
    Open(PathBuf),
    NewDialog,
    Quit,
}

/// Why a file is being converted to PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Open,
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    Letter,
    A4,
    Legal,
    A5,
}

pub(crate) const PAPERS: [Paper; 4] = [Paper::Letter, Paper::A4, Paper::Legal, Paper::A5];

impl Paper {
    fn size(self) -> PaperSize {
        match self {
            Self::Letter => PaperSize::LETTER,
            Self::A4 => PaperSize::A4,
            Self::Legal => PaperSize::LEGAL,
            Self::A5 => PaperSize::A5,
        }
    }
}

impl fmt::Display for Paper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Letter => "Letter",
            Self::A4 => "A4",
            Self::Legal => "Legal",
            Self::A5 => "A5",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

pub(crate) const ORIENTATIONS: [Orientation; 2] = [Orientation::Portrait, Orientation::Landscape];

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        })
    }
}

/// The New Document dialog's choices.
#[derive(Debug, Clone)]
pub(crate) struct NewForm {
    pub(crate) paper: Paper,
    pub(crate) orientation: Orientation,
    pub(crate) pages: String,
}

impl Default for NewForm {
    fn default() -> Self {
        Self {
            paper: Paper::Letter,
            orientation: Orientation::Portrait,
            pages: "1".into(),
        }
    }
}

const MAX_NEW_PAGES: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug)]
pub struct App {
    pub(crate) engine: Result<Worker, String>,
    /// Generation of the newest page render; the engine thread skips older queued requests.
    latest_render: Arc<AtomicU64>,
    window: Option<window::Id>,
    pub(crate) window_size: iced::Size,
    scale_factor: f32,
    appearance: Appearance,
    system_dark: bool,
    pub(crate) sidebar: bool,
    pub(crate) zoom: Zoom,
    pub(crate) doc: Option<Document>,
    pub(crate) page_input: String,
    pub(crate) page_image: Option<crate::render::PageImage>,
    pub(crate) error: Option<String>,
    pub(crate) notice: Option<String>,
    /// Set while a save or export runs; editing is paused until it finishes.
    pub(crate) busy: Option<&'static str>,
    /// Asking whether to save unsaved changes before doing this.
    pub(crate) confirm: Option<Proceed>,
    after_save: Option<Proceed>,
    /// Page to return to when a document is reopened after saving.
    reopen_at: Option<usize>,
    modifiers: keyboard::Modifiers,
    /// Installed conversion tools, detected in the background at startup.
    pub(crate) converter: Option<Arc<Converter>>,
    /// The New Document dialog, while open.
    pub(crate) new_form: Option<NewForm>,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenDialog,
    Open(PathBuf),
    DialogClosed,
    Opened(Origin, Result<DocInfo, String>),
    GoTo(usize),
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
    Thumbnail(DocId, PageKey, Rotation, Result<Rendered, String>),
    WindowOpened(Option<window::Id>),
    ScaleFactor(f32),
    Resized(iced::Size),
    ModifiersChanged(keyboard::Modifiers),
    DragWindow,
    DismissStatus,
    Print,
    PrintReady(Result<PathBuf, String>),
    Printed(Result<PrintOutcome, String>),
    ThumbnailClicked(PageKey),
    Rotate(Rotation),
    Delete,
    Move { earlier: bool },
    InsertDialog,
    Insert(PathBuf),
    Inserted(Origin, Result<DocInfo, String>),
    Undo,
    Redo,
    Save,
    SaveAs,
    SaveAsPicked(Option<PathBuf>),
    Saved(Result<PathBuf, String>),
    CloseRequested,
    Confirmed(Choice),
    Escape,
    ConverterReady(Arc<Converter>),
    Converted(Purpose, PathBuf, Result<Vec<u8>, String>),
    NewDialog,
    NewPaper(Paper),
    NewOrientation(Orientation),
    NewPages(String),
    CreateBlank,
    CreateFromFiles,
    NewPicked(Vec<PathBuf>),
    CloseNewDialog,
}

pub(crate) fn page_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("page")
}

pub(crate) fn sidebar_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("sidebar")
}

impl App {
    pub(crate) fn new(
        engine: Result<Worker, String>,
        initial: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        let error = engine.as_ref().err().cloned();
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
            error,
            notice: None,
            busy: None,
            confirm: None,
            after_save: None,
            reopen_at: None,
            modifiers: keyboard::Modifiers::default(),
            converter: None,
            new_form: None,
        };
        let open = initial.map_or_else(Task::none, |path| Task::done(Message::Open(path)));
        let boot = Task::batch([
            window::oldest().map(Message::WindowOpened),
            system::theme().map(Message::SystemTheme),
            Task::perform(
                blocking(|| Ok(Converter::detect())),
                |found: Result<Converter, String>| {
                    Message::ConverterReady(Arc::new(found.unwrap_or_default()))
                },
            ),
        ]);
        (app, boot.chain(open))
    }

    pub(crate) fn title(&self) -> String {
        match &self.doc {
            Some(doc) if doc.is_dirty() => format!("{} — Edited — Ferrix", doc.name()),
            Some(doc) => format!("{} — Ferrix", doc.name()),
            None => "Ferrix".into(),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        if self.is_dark() {
            crate::theme::dark()
        } else {
            crate::theme::light()
        }
    }

    pub(crate) fn is_dark(&self) -> bool {
        match self.appearance {
            Appearance::System => self.system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        }
    }

    /// Whether page-editing actions are available right now.
    pub(crate) fn can_edit(&self) -> bool {
        self.doc.is_some()
            && self.busy.is_none()
            && self.confirm.is_none()
            && self.new_form.is_none()
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenDialog => return self.guard(Proceed::OpenDialog),
            Message::Open(path) => return self.guard(Proceed::Open(path)),
            Message::DialogClosed => {}
            Message::Opened(origin, Ok(info)) => return self.opened(origin, info),
            Message::Opened(origin, Err(e)) => {
                self.reopen_at = None;
                self.error = Some(format!("Couldn’t open “{}”: {e}", origin.file_name()));
            }
            Message::GoTo(index) => return self.go_to(index),
            Message::Next => return self.step_page(1),
            Message::Previous => return self.step_page(-1),
            Message::First => return self.go_to(0),
            Message::Last => {
                let last = self.doc.as_ref().map_or(0, |d| d.len().saturating_sub(1));
                return self.go_to(last);
            }
            Message::PageInput(value) => {
                self.page_input = value.chars().filter(char::is_ascii_digit).take(6).collect();
            }
            Message::PageSubmit => {
                let task = match self.page_input.parse::<usize>() {
                    Ok(n) if n >= 1 => self.go_to(n - 1),
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
                self.appearance = if self.is_dark() {
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
                self.error = Some(format!("Couldn’t render this page: {e}"));
            }
            Message::Thumbnail(doc_id, key, rotation, result) => {
                let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
                    return Task::none(); // the document was closed meanwhile
                };
                doc.thumbnail_busy = false;
                if let Ok(r) = result {
                    doc.thumbnails.insert(
                        key,
                        Thumbnail {
                            rotation,
                            image: r.into_image(),
                        },
                    );
                }
                return self.next_thumbnail();
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
            Message::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::DismissStatus => {
                self.error = None;
                self.notice = None;
            }
            Message::Print => return self.print(),
            Message::PrintReady(Ok(path)) => {
                self.busy = None;
                return self.print_file(path);
            }
            Message::PrintReady(Err(e)) => {
                self.busy = None;
                self.error = Some(format!("Couldn’t prepare the document for printing: {e}"));
            }
            Message::Printed(Ok(_)) => {}
            Message::Printed(Err(e)) => self.error = Some(format!("Couldn’t print: {e}")),
            Message::ThumbnailClicked(key) => {
                let click = if self.modifiers.shift() {
                    Click::Extend
                } else if self.modifiers.command() {
                    Click::Toggle
                } else {
                    Click::Replace
                };
                if let Some(doc) = &mut self.doc {
                    doc.click(key, click);
                }
                self.sync_page_input();
                return self.render();
            }
            Message::Rotate(by) => {
                let Some(doc) = self.editable() else {
                    return Task::none();
                };
                let keys = doc.targets();
                let label = pages_label("Rotate", keys.len());
                return self.apply(Edit::Rotate { keys, by }, label);
            }
            Message::Delete => {
                let Some(doc) = self.editable() else {
                    return Task::none();
                };
                let keys = doc.targets();
                let label = pages_label("Delete", keys.len());
                return self.apply(Edit::Remove { keys }, label);
            }
            Message::Move { earlier } => {
                let Some(doc) = self.editable() else {
                    return Task::none();
                };
                let keys = doc.targets();
                let positions: Vec<usize> =
                    keys.iter().filter_map(|k| doc.pages.position(*k)).collect();
                let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else {
                    return Task::none();
                };
                // Positions are counted in the list without the moved pages.
                let to = if earlier {
                    first.checked_sub(1)
                } else {
                    (last + 1 < doc.len()).then(|| last + 2 - keys.len())
                };
                let Some(to) = to else { return Task::none() };
                match doc.pages.move_edit(&keys, to) {
                    Ok(edit) => {
                        let label = pages_label("Move", keys.len());
                        return self.apply(edit, label);
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Message::InsertDialog => {
                if !self.can_edit() {
                    return Task::none();
                }
                return Task::perform(self.file_dialog("Insert Pages From").pick_file(), |file| {
                    file.map_or(Message::DialogClosed, |f| {
                        Message::Insert(f.path().to_path_buf())
                    })
                });
            }
            Message::Insert(path) => {
                if kind_of(&path) == Some(Kind::Pdf) {
                    return self.load(Origin::File(path), Message::Inserted);
                }
                return self.convert(Purpose::Insert, vec![path]);
            }
            Message::Inserted(origin, Ok(info)) => return self.inserted(origin, info),
            Message::Inserted(origin, Err(e)) => {
                self.error = Some(format!("Couldn’t insert “{}”: {e}", origin.file_name()));
            }
            Message::Undo => {
                if self.can_edit()
                    && let Some(doc) = &mut self.doc
                    && let Some(label) = doc.undo()
                {
                    self.notice = Some(format!("Undid {label}"));
                    return self.after_edit();
                }
            }
            Message::Redo => {
                if self.can_edit()
                    && let Some(doc) = &mut self.doc
                    && let Some(label) = doc.redo()
                {
                    self.notice = Some(format!("Redid {label}"));
                    return self.after_edit();
                }
            }
            Message::Save => {
                if let Some(doc) = &self.doc
                    && doc.is_dirty()
                {
                    // Documents that were never saved need a location first.
                    let Some(path) = doc.origin.path().map(PathBuf::from) else {
                        return self.update(Message::SaveAs);
                    };
                    return self.save_to(path);
                }
            }
            Message::SaveAs => {
                let Some(doc) = self.doc.as_ref().filter(|_| self.busy.is_none()) else {
                    return Task::none();
                };
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Save As")
                    .add_filter("PDF documents", &["pdf"])
                    .set_file_name(doc.origin.file_name());
                if let Some(dir) = doc.origin.save_location().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                }
                return Task::perform(dialog.save_file(), |file| {
                    Message::SaveAsPicked(file.map(|f| f.path().to_path_buf()))
                });
            }
            Message::SaveAsPicked(Some(path)) => return self.save_to(path),
            Message::SaveAsPicked(None) => self.after_save = None,
            Message::Saved(Ok(path)) => {
                self.busy = None;
                self.notice = Some(format!("Saved “{}”", file_name(&path)));
                if let Some(next) = self.after_save.take() {
                    return self.proceed(next);
                }
                // Reload what is now on disk; the edit history starts over from it.
                self.reopen_at = self.doc.as_ref().map(|d| d.current);
                return self.load(Origin::File(path), Message::Opened);
            }
            Message::Saved(Err(e)) => {
                self.busy = None;
                self.after_save = None;
                self.error = Some(format!("Couldn’t save: {e}"));
            }
            Message::CloseRequested => return self.guard(Proceed::Quit),
            Message::Confirmed(choice) => {
                let Some(next) = self.confirm.take() else {
                    return Task::none();
                };
                match choice {
                    Choice::Save => {
                        self.after_save = Some(next);
                        return self.update(Message::Save);
                    }
                    Choice::Discard => return self.proceed(next),
                    Choice::Cancel => {}
                }
            }
            Message::Escape => {
                self.confirm = None;
                self.new_form = None;
            }
            Message::ConverterReady(converter) => self.converter = Some(converter),
            Message::Converted(purpose, source, Ok(bytes)) => {
                self.busy = None;
                let stem = source
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled".into());
                let name = format!("{stem}.pdf");
                let origin =
                    Origin::untitled(name.clone(), Some(source.with_file_name(name)), bytes);
                return match purpose {
                    Purpose::Open => self.load(origin, Message::Opened),
                    Purpose::Insert => self.load(origin, Message::Inserted),
                };
            }
            Message::Converted(_, source, Err(e)) => {
                self.busy = None;
                self.error = Some(format!("Couldn’t convert “{}”: {e}", file_name(&source)));
            }
            Message::NewDialog => return self.guard(Proceed::NewDialog),
            Message::NewPaper(paper) => {
                if let Some(form) = &mut self.new_form {
                    form.paper = paper;
                }
            }
            Message::NewOrientation(orientation) => {
                if let Some(form) = &mut self.new_form {
                    form.orientation = orientation;
                }
            }
            Message::NewPages(value) => {
                if let Some(form) = &mut self.new_form {
                    form.pages = value.chars().filter(char::is_ascii_digit).take(4).collect();
                }
            }
            Message::CreateBlank => {
                let Some(form) = self.new_form.take() else {
                    return Task::none();
                };
                let pages = form
                    .pages
                    .parse::<u32>()
                    .unwrap_or(1)
                    .clamp(1, MAX_NEW_PAGES);
                let size = match form.orientation {
                    Orientation::Portrait => form.paper.size(),
                    Orientation::Landscape => form.paper.size().landscape(),
                };
                match ferrix_pdf::blank_document(pages, size) {
                    Ok(bytes) => {
                        let origin = Origin::untitled("Untitled.pdf".into(), None, bytes);
                        return self.load(origin, Message::Opened);
                    }
                    Err(e) => self.error = Some(format!("Couldn’t create the document: {e}")),
                }
            }
            Message::CreateFromFiles => {
                return Task::perform(self.file_dialog("Create PDF From").pick_files(), |files| {
                    Message::NewPicked(
                        files
                            .unwrap_or_default()
                            .iter()
                            .map(|f| f.path().to_path_buf())
                            .collect(),
                    )
                });
            }
            Message::NewPicked(paths) if paths.is_empty() => {}
            Message::NewPicked(paths) => {
                self.new_form = None;
                if let [path] = paths.as_slice()
                    && kind_of(path) == Some(Kind::Pdf)
                {
                    return self.load(Origin::File(path.clone()), Message::Opened);
                }
                return self.convert(Purpose::Open, paths);
            }
            Message::CloseNewDialog => self.new_form = None,
        }
        Task::none()
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            system::theme_changes().map(Message::SystemTheme),
            window::close_requests().map(|_| Message::CloseRequested),
            event::listen_with(|event, status, _window| match event {
                Event::Window(window::Event::FileDropped(path)) => Some(Message::Open(path)),
                Event::Window(window::Event::Rescaled(factor)) => {
                    Some(Message::ScaleFactor(factor))
                }
                Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                    Some(Message::ModifiersChanged(modifiers))
                }
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored =>
                {
                    shortcut(&key, modifiers)
                }
                _ => None,
            }),
        ])
    }

    // ── Documents ─────────────────────────────────────────────────────────────

    /// Runs `next` now, or first asks what to do with unsaved changes.
    fn guard(&mut self, next: Proceed) -> Task<Message> {
        if self.busy.is_some() {
            return Task::none();
        }
        if self.doc.as_ref().is_some_and(Document::is_dirty) {
            self.confirm = Some(next);
            return Task::none();
        }
        self.proceed(next)
    }

    fn proceed(&mut self, next: Proceed) -> Task<Message> {
        match next {
            Proceed::OpenDialog => Task::perform(self.file_dialog("Open").pick_file(), |file| {
                file.map_or(Message::DialogClosed, |f| {
                    Message::Open(f.path().to_path_buf())
                })
            }),
            // Unsaved changes were already dealt with, so open directly.
            Proceed::Open(path) => {
                if kind_of(&path) == Some(Kind::Pdf) {
                    self.load(Origin::File(path), Message::Opened)
                } else {
                    self.convert(Purpose::Open, vec![path])
                }
            }
            Proceed::NewDialog => {
                self.new_form = Some(NewForm::default());
                Task::none()
            }
            Proceed::Quit => iced::exit(),
        }
    }

    /// A file dialog offering every format that can be opened right now.
    fn file_dialog(&self, title: &str) -> rfd::AsyncFileDialog {
        let supported = self
            .converter
            .as_ref()
            .map_or_else(|| vec!["pdf"], |c| c.extensions());
        rfd::AsyncFileDialog::new()
            .set_title(title)
            .add_filter("All supported files", &supported)
            .add_filter("PDF documents", &["pdf"])
            .add_filter("Images", ferrix_convert::IMAGE_EXTENSIONS)
    }

    /// Opens `origin` in the engine, reporting the result through `done`.
    fn load(
        &mut self,
        origin: Origin,
        done: fn(Origin, Result<DocInfo, String>) -> Message,
    ) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        self.error = None;
        Task::perform(
            async move {
                let source = match &origin {
                    Origin::File(path) => DocSource::Path(path.clone()),
                    Origin::Untitled { bytes, .. } => DocSource::Bytes(bytes.as_ref().clone()),
                };
                let result = flatten(engine.call(move |e| e.open(source, None)).await);
                (origin, result)
            },
            move |(origin, result)| done(origin, result),
        )
    }

    /// Converts files to one PDF in the background: images become pages, an office
    /// document is converted by Word or LibreOffice.
    fn convert(&mut self, purpose: Purpose, paths: Vec<PathBuf>) -> Task<Message> {
        let Some(first) = paths.first().cloned() else {
            return Task::none();
        };
        let converter = self.converter.clone().unwrap_or_default();
        let all_images = paths.iter().all(|p| kind_of(p) == Some(Kind::Image));
        if !all_images && paths.len() > 1 {
            self.error = Some("Choose one document, or several images, to convert".into());
            return Task::none();
        }
        self.busy = Some("Converting…");
        self.error = None;
        Task::perform(
            blocking(move || {
                if all_images {
                    ferrix_convert::images_to_pdf(&paths)
                } else {
                    converter.convert(&paths[0])
                }
                .map_err(|e| e.to_string())
            }),
            move |result| Message::Converted(purpose, first.clone(), result),
        )
    }

    fn opened(&mut self, origin: Origin, info: DocInfo) -> Task<Message> {
        let close = self.close_document();
        if info.page_count == 0 {
            self.error = Some("This document has no pages".into());
            return close;
        }
        let mut doc = Document::new(origin, info);
        if let Some(at) = self.reopen_at.take() {
            let at = at.min(doc.len() - 1);
            doc.current = at;
            if let Some(key) = doc.key_at(at) {
                doc.select_only(key);
            }
        } else {
            self.notice = None;
        }
        self.doc = Some(doc);
        self.sync_page_input();
        Task::batch([close, self.render(), self.next_thumbnail()])
    }

    /// Closes the current document and its inserted sources on the engine thread.
    fn close_document(&mut self) -> Task<Message> {
        self.page_image = None;
        let (Some(doc), Ok(engine)) = (self.doc.take(), self.engine.clone()) else {
            return Task::none();
        };
        let ids = doc.engine_docs();
        Task::future(async move {
            engine
                .call(move |e| ids.into_iter().for_each(|id| e.close(id)))
                .await
        })
        .discard()
    }

    // ── Editing ───────────────────────────────────────────────────────────────

    fn editable(&mut self) -> Option<&mut Document> {
        if self.can_edit() {
            self.doc.as_mut()
        } else {
            None
        }
    }

    fn apply(&mut self, edit: Edit, label: String) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.edit(edit, label) {
            Ok(()) => {
                self.notice = None;
                self.after_edit()
            }
            Err(e) => {
                self.error = Some(e.to_string());
                Task::none()
            }
        }
    }

    fn after_edit(&mut self) -> Task<Message> {
        self.sync_page_input();
        Task::batch([self.render(), self.next_thumbnail()])
    }

    fn inserted(&mut self, origin: Origin, info: DocInfo) -> Task<Message> {
        let Some(doc) = self.editable() else {
            return Task::none();
        };
        let source = ferrix_core::SourceId(doc.sources.len() as u32);
        doc.sources.push(crate::document::Source {
            origin,
            doc: info.id,
        });
        let entries = doc
            .pages
            .new_entries(source, (0..info.page_count).map(ferrix_types::PageIndex));
        let at = doc.current + 1;
        let count = entries.len();
        let pages = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| (at + i, e))
            .collect();
        let task = self.apply(Edit::Insert { pages }, pages_label("Insert", count));
        // Show the first inserted page.
        Task::batch([task, self.go_to(at)])
    }

    // ── Saving ────────────────────────────────────────────────────────────────

    fn save_to(&mut self, target: PathBuf) -> Task<Message> {
        let Some(export) = self.export_job() else {
            return Task::none();
        };
        self.busy = Some("Saving…");
        self.error = None;
        Task::perform(
            async move {
                let bytes = export.run().await?;
                let destination = target.clone();
                blocking(move || {
                    ferrix_io::write_atomic(&destination, &bytes).map_err(|e| e.to_string())
                })
                .await?;
                Ok(target)
            },
            Message::Saved,
        )
    }

    fn export_job(&self) -> Option<Export> {
        let (Ok(engine), Some(doc)) = (self.engine.clone(), &self.doc) else {
            return None;
        };
        if self.busy.is_some() {
            return None;
        }
        Some(Export {
            engine,
            base: doc.origin.clone(),
            stamp: doc.stamp,
            imports: doc.imports(),
            layout: doc.layout(),
        })
    }

    // ── Printing ──────────────────────────────────────────────────────────────

    /// Prints the document as shown: unsaved edits are exported to a temporary file first.
    fn print(&mut self) -> Task<Message> {
        let Some(doc) = &self.doc else {
            return Task::none();
        };
        if !doc.is_dirty()
            && let Some(path) = doc.origin.path()
        {
            let path = path.to_path_buf();
            return self.print_file(path);
        }
        let Some(export) = self.export_job() else {
            return Task::none();
        };
        let temp = std::env::temp_dir().join(format!(
            "ferrix-print-{}-{}",
            std::process::id(),
            doc.origin.file_name()
        ));
        self.busy = Some("Preparing to print…");
        Task::perform(
            async move {
                let bytes = export.run().await?;
                let destination = temp.clone();
                blocking(move || std::fs::write(&destination, bytes).map_err(|e| e.to_string()))
                    .await?;
                Ok(temp)
            },
            Message::PrintReady,
        )
    }

    fn print_file(&mut self, path: PathBuf) -> Task<Message> {
        let (Some(doc), Some(window)) = (&self.doc, self.window) else {
            return Task::none();
        };
        let job = PrintJob {
            title: doc.name(),
            path,
            page_count: doc.len() as u32,
        };
        self.error = None;
        self.platform_print(window, job)
    }

    /// macOS: a print sheet on our window, opened on the main thread.
    #[cfg(target_os = "macos")]
    fn platform_print(&self, window: window::Id, job: PrintJob) -> Task<Message> {
        window::run(window, move |w| {
            ferrix_print::print_with_sheet(w, &job).map_err(|e| e.to_string())
        })
        .map(Message::Printed)
    }

    /// Linux/BSD: the desktop portal's print dialog.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn platform_print(&self, window: window::Id, job: PrintJob) -> Task<Message> {
        window::run(window, |w| ferrix_print::Parent::of(w)).then(move |parent| {
            let job = job.clone();
            Task::perform(
                async move {
                    ferrix_print::print_with_portal(parent, &job)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::Printed,
            )
        })
    }

    /// Windows: the common print dialog, with pages rendered by our engine on a spool thread.
    #[cfg(windows)]
    fn platform_print(&self, window: window::Id, job: PrintJob) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        window::run(window, |w| ferrix_print::Parent::of(w)).then(move |parent| {
            let (job, engine) = (job.clone(), engine.clone());
            Task::perform(
                blocking(move || crate::print::print_on_windows(engine, parent, &job)),
                Message::Printed,
            )
        })
    }

    // ── Navigation and rendering ──────────────────────────────────────────────

    /// Logical size available to a page inside the canvas.
    pub(crate) fn canvas_size(&self) -> iced::Size {
        crate::view::canvas_size(self.window_size, self.sidebar && self.doc.is_some())
    }

    fn sync_page_input(&mut self) {
        self.page_input = self
            .doc
            .as_ref()
            .map(|d| (d.current + 1).to_string())
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

    fn step_page(&mut self, delta: isize) -> Task<Message> {
        let Some(doc) = &self.doc else {
            return Task::none();
        };
        let target = doc.current.saturating_add_signed(delta).min(doc.len() - 1);
        self.go_to(target)
    }

    fn go_to(&mut self, index: usize) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let index = index.min(doc.len().saturating_sub(1));
        if let Some(key) = doc.key_at(index) {
            doc.select_only(key);
        }
        if index == doc.current {
            return Task::none();
        }
        doc.current = index;
        let follow = if doc.len() > 1 {
            index as f32 / (doc.len() - 1) as f32
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

    /// Requests the current page at the current zoom and display scale.
    fn render(&mut self) -> Task<Message> {
        let (Ok(engine), Some(page)) = (
            self.engine.clone(),
            self.doc.as_ref().and_then(Document::current_page),
        ) else {
            return Task::none();
        };
        let generation = self.latest_render.fetch_add(1, Ordering::Relaxed) + 1;
        let latest = Arc::clone(&self.latest_render);
        let (zoom_mode, canvas, scale_factor) = (self.zoom, self.canvas_size(), self.scale_factor);

        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        if latest.load(Ordering::Relaxed) != generation {
                            return Ok(None);
                        }
                        let size = crate::render::page_size(e, page)?;
                        let zoom = zoom_mode.resolve(size, canvas);
                        render_page(e, page, zoom * LOGICAL_PX_PER_POINT, scale_factor, zoom)
                            .map(Some)
                    })
                    .await;
                flatten(result)
            },
            Message::Rendered,
        )
    }

    /// Renders the next missing thumbnail; each result schedules the one after it.
    fn next_thumbnail(&mut self) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        let Some(doc) = self.doc.as_mut().filter(|d| !d.thumbnail_busy) else {
            return Task::none();
        };
        let Some((key, page)) = doc.next_thumbnail() else {
            return Task::none();
        };
        doc.thumbnail_busy = true;
        let doc_id = doc.info.id;
        let scale_factor = self.scale_factor.max(2.0);
        Task::perform(
            async move {
                let result = engine
                    .call(move |e| render_thumbnail(e, page, scale_factor))
                    .await;
                flatten(result)
            },
            move |result| Message::Thumbnail(doc_id, key, page.rotation, result),
        )
    }
}

fn render_thumbnail(
    engine: &mut PdfiumEngine,
    page: PageRef,
    scale_factor: f32,
) -> Result<Rendered, ferrix_engine::EngineError> {
    let size = crate::render::page_size(engine, page)?;
    // Fit a box THUMB_WIDTH wide, and not much taller, whatever the orientation.
    let scale = (THUMB_WIDTH / size.width).min(THUMB_WIDTH * 1.4 / size.height);
    render_page(engine, page, scale, scale_factor, 0.0)
}

/// Everything needed to write the edited document, gathered on the UI thread.
#[derive(Debug)]
struct Export {
    engine: Worker,
    base: Origin,
    stamp: Option<FileStamp>,
    imports: Vec<Origin>,
    layout: Vec<LayoutPage>,
}

impl Export {
    /// Writes the edited document to bytes and checks them with the rendering engine.
    async fn run(self) -> Result<Vec<u8>, String> {
        let expected = self.layout.len();
        let Self {
            engine,
            base,
            stamp,
            imports,
            layout,
        } = self;
        let bytes = blocking(move || {
            if let Some(path) = base.path()
                && stamp.is_some()
                && FileStamp::of(path).ok() != stamp
            {
                return Err(format!(
                    "“{}” was changed by another app after it was opened",
                    base.file_name()
                ));
            }
            let base_bytes = base.read()?;
            let import_bytes = imports
                .iter()
                .map(Origin::read)
                .collect::<Result<Vec<_>, _>>()?;
            let sources: Vec<PdfSource<'_>> =
                import_bytes.iter().map(|b| PdfSource::new(b)).collect();
            ferrix_pdf::write_page_layout(PdfSource::new(&base_bytes), &sources, &layout)
                .map_err(|e| e.to_string())
        })
        .await?;

        // Never replace a file with something our own engine can't open.
        let check = bytes.clone();
        let pages = flatten(
            engine
                .call(move |e| {
                    let info = e.open(DocSource::Bytes(check), None)?;
                    e.close(info.id);
                    Ok(info.page_count)
                })
                .await,
        )
        .map_err(|e| format!("verification failed: {e}"))?;
        if pages as usize != expected {
            return Err(format!(
                "verification failed: {pages} pages written, {expected} expected"
            ));
        }
        Ok(bytes)
    }
}

fn shortcut(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    use keyboard::Key;
    match key.as_ref() {
        Key::Named(Named::ArrowUp) if modifiers.alt() => Some(Message::Move { earlier: true }),
        Key::Named(Named::ArrowDown) if modifiers.alt() => Some(Message::Move { earlier: false }),
        Key::Named(Named::ArrowRight | Named::PageDown) => Some(Message::Next),
        Key::Named(Named::ArrowLeft | Named::PageUp) => Some(Message::Previous),
        Key::Named(Named::Home) => Some(Message::First),
        Key::Named(Named::End) => Some(Message::Last),
        Key::Named(Named::Delete | Named::Backspace) => Some(Message::Delete),
        Key::Named(Named::Escape) => Some(Message::Escape),
        Key::Character(c) if modifiers.command() => {
            let shift = modifiers.shift();
            match c.to_ascii_lowercase().as_str() {
                "o" => Some(Message::OpenDialog),
                "n" => Some(Message::NewDialog),
                "p" => Some(Message::Print),
                "s" if shift => Some(Message::SaveAs),
                "s" => Some(Message::Save),
                "z" if shift => Some(Message::Redo),
                "z" => Some(Message::Undo),
                "y" => Some(Message::Redo),
                "r" => Some(Message::Rotate(Rotation::R90)),
                "l" => Some(Message::Rotate(Rotation::R270)),
                "q" => Some(Message::CloseRequested),
                "=" | "+" => Some(Message::ZoomIn),
                "-" => Some(Message::ZoomOut),
                "0" => Some(Message::ZoomPicked(ZoomChoice::Percent(100))),
                "1" => Some(Message::FitPage),
                "2" => Some(Message::FitWidth),
                "\\" => Some(Message::ToggleSidebar),
                _ => None,
            }
        }
        _ => None,
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
    fn auto_zoom_never_enlarges_past_the_cap() {
        let huge = iced::Size::new(4000.0, 4000.0);
        assert_eq!(Zoom::Auto.resolve(LETTER, huge), MAX_AUTO_ZOOM);
        assert_eq!(Zoom::Fixed(9.0).resolve(LETTER, huge), MAX_ZOOM);
    }

    #[test]
    fn shortcuts_respect_shift_and_case() {
        let cmd = keyboard::Modifiers::COMMAND;
        let cmd_shift = cmd | keyboard::Modifiers::SHIFT;
        let z = keyboard::Key::Character("z".into());
        let big_z = keyboard::Key::Character("Z".into());
        assert!(matches!(shortcut(&z, cmd), Some(Message::Undo)));
        assert!(matches!(shortcut(&big_z, cmd_shift), Some(Message::Redo)));
        assert!(matches!(
            shortcut(&keyboard::Key::Character("s".into()), cmd_shift),
            Some(Message::SaveAs)
        ));
    }

    /// A minimal PDF whose pages read "Page 1", "Page 2", ….
    fn fixture(pages: u32) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            String::new(), // page tree, filled in below
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut kids = Vec::new();
        for n in 1..=pages {
            let content = format!("BT /F1 24 Tf 72 700 Td (Page {n}) Tj ET");
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
            let page_id = objects.len() + 1;
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                page_id - 1
            ));
            kids.push(format!("{page_id} 0 R"));
        }
        objects[1] = format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        );
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).into_bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").into_bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .into_bytes(),
        );
        out
    }

    fn worker() -> Option<Worker> {
        match PdfiumEngine::new() {
            Ok(engine) => Some(EngineWorker::spawn(engine).expect("engine thread")),
            Err(e) if std::env::var_os("CI").is_none() => {
                eprintln!("skipping: {e}");
                None
            }
            Err(e) => panic!("PDFium is required on CI: {e}"),
        }
    }

    fn open_fixture(worker: &Worker, name: &str, pages: u32) -> Document {
        let dir = std::env::temp_dir().join(format!("ferrix-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, fixture(pages)).expect("write fixture");
        let source = DocSource::Path(path.clone());
        let info = flatten(iced::futures::executor::block_on(
            worker.call(move |e| e.open(source, None)),
        ))
        .expect("opens");
        Document::new(Origin::File(path), info)
    }

    fn export(worker: &Worker, doc: &Document) -> Result<Vec<u8>, String> {
        iced::futures::executor::block_on(
            Export {
                engine: worker.clone(),
                base: doc.origin.clone(),
                stamp: doc.stamp,
                imports: doc.imports(),
                layout: doc.layout(),
            }
            .run(),
        )
    }

    #[test]
    fn export_writes_the_edited_page_list() {
        let Some(worker) = worker() else { return };
        let mut doc = open_fixture(&worker, "export.pdf", 4);
        let keys: Vec<PageKey> = (0..4).filter_map(|i| doc.key_at(i)).collect();
        doc.edit(
            Edit::Remove {
                keys: vec![keys[1]],
            },
            "Delete".into(),
        )
        .expect("delete");
        let to_front = doc.pages.move_edit(&[keys[3]], 0).expect("move");
        doc.edit(to_front, "Move".into()).expect("move");
        doc.edit(
            Edit::Rotate {
                keys: vec![keys[0]],
                by: Rotation::R90,
            },
            "Rotate".into(),
        )
        .expect("rotate");

        let bytes = export(&worker, &doc).expect("export");
        let (texts, first_rotation) =
            flatten(iced::futures::executor::block_on(worker.call(move |e| {
                let info = e.open(DocSource::Bytes(bytes), None)?;
                let texts = (0..info.page_count)
                    .map(|i| {
                        e.text_page(info.id, ferrix_types::PageIndex(i))
                            .map(|t| t.text().trim().to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let rotation = e.page_info(info.id, ferrix_types::PageIndex(1))?.rotation;
                Ok((texts, rotation))
            })))
            .expect("reads back");
        assert_eq!(texts, ["Page 4", "Page 1", "Page 3"]);
        assert_eq!(first_rotation, Rotation::R90);
    }

    #[test]
    fn export_refuses_a_file_changed_by_another_app() {
        let Some(worker) = worker() else { return };
        let doc = open_fixture(&worker, "changed.pdf", 2);
        let path = doc.origin.path().expect("file-backed");
        std::fs::write(path, fixture(3)).expect("external change");
        let error = export(&worker, &doc).expect_err("must refuse");
        assert!(error.contains("changed by another app"), "{error}");
    }
}
