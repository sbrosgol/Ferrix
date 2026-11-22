//! Application state and update logic. Layout lives in `view.rs`.

use std::{fmt, path::PathBuf, sync::Arc};

use ferrix_convert::{Converter, Kind};
use ferrix_core::{Edit, PageKey};
use ferrix_engine::{DocInfo, DocSource, PdfEngine};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_io::FileStamp;
use ferrix_pdf::{LayoutPage, PaperSize, Source as PdfSource};
use ferrix_print::{PrintJob, PrintOutcome};
use ferrix_types::{DocId, PageIndex, Rotation, Size};
use iced::{
    Event, Subscription, Task, Theme, event, keyboard,
    keyboard::key::Named,
    system,
    widget::{operation, scrollable},
    window,
};

use ferrix_convert::kind_of;
pub use ferrix_store::Layout;

use ferrix_compose::{Editor, Fonts, Opened, PageSetup};

use crate::{
    document::{
        Click, Document, Origin, Shown, Source, Thumbnail, file_name, pages_fit, pages_label,
    },
    recovery::{Journal, Recoverable, WriterJournal},
    render::{LOGICAL_PX_PER_POINT, PageRef, Rendered, render_page},
    strip::Strip,
    worker::{EngineWorker, blocking, flatten},
    writer::{Action as WriteAction, KeyCommand, MarginChoice, Writer},
};

const ZOOM_STEPS: [f32; 18] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0,
];
pub(crate) const MIN_ZOOM: f32 = 0.1;
/// Large pages render as tiles of the part in view, so zoom isn't limited by page memory.
pub(crate) const MAX_ZOOM: f32 = 8.0;
/// "Automatic" zoom fits the width but never enlarges beyond this (as pdf.js does).
const MAX_AUTO_ZOOM: f32 = 1.25;
pub(crate) const THUMB_WIDTH: f32 = 112.0;

pub(crate) type Worker = EngineWorker<PdfiumEngine>;

/// Light, dark, or whatever the operating system uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

impl Appearance {
    fn from_saved(saved: ferrix_store::Appearance) -> Self {
        match saved {
            ferrix_store::Appearance::System => Self::System,
            ferrix_store::Appearance::Light => Self::Light,
            ferrix_store::Appearance::Dark => Self::Dark,
        }
    }

    fn to_saved(self) -> ferrix_store::Appearance {
        match self {
            Self::System => ferrix_store::Appearance::System,
            Self::Light => ferrix_store::Appearance::Light,
            Self::Dark => ferrix_store::Appearance::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Zoom {
    Auto,
    FitWidth,
    FitPage,
    Fixed(f32),
}

impl Zoom {
    fn from_saved(saved: ferrix_store::SavedZoom) -> Self {
        match saved.sanitized() {
            ferrix_store::SavedZoom::Auto => Self::Auto,
            ferrix_store::SavedZoom::FitWidth => Self::FitWidth,
            ferrix_store::SavedZoom::FitPage => Self::FitPage,
            ferrix_store::SavedZoom::Fixed(z) => Self::Fixed(z.clamp(MIN_ZOOM, MAX_ZOOM)),
        }
    }

    fn to_saved(self) -> ferrix_store::SavedZoom {
        match self {
            Self::Auto => ferrix_store::SavedZoom::Auto,
            Self::FitWidth => ferrix_store::SavedZoom::FitWidth,
            Self::FitPage => ferrix_store::SavedZoom::FitPage,
            Self::Fixed(z) => ferrix_store::SavedZoom::Fixed(z),
        }
    }

    /// The zoom factor (1.0 = 100%) for a page of `page` points in a viewport of `canvas`
    /// logical pixels.
    fn resolve(self, page: Size, canvas: iced::Size, layout: Layout) -> f32 {
        // Two pages side by side share the width, less the gap between them.
        let (across, gaps) = match layout {
            Layout::Continuous | Layout::SinglePage => (1.0, 0.0),
            Layout::TwoPages | Layout::TwoPagesCover => (2.0, crate::strip::PAGE_GAP),
        };
        let fit_width =
            (canvas.width - gaps).max(1.0) / (across * page.width * LOGICAL_PX_PER_POINT);
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
    Recover,
    Quit,
}

/// The answer in the crash-recovery dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    Recover,
    /// Keep the recovery data for a later launch.
    Later,
    Discard,
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

/// The Page Setup dialog of a written document.
#[derive(Debug, Clone)]
pub(crate) struct PageSetupForm {
    pub(crate) paper: Paper,
    pub(crate) orientation: Orientation,
    pub(crate) margins: MarginChoice,
    pub(crate) title: String,
    pub(crate) header: String,
    pub(crate) footer: String,
    /// Leave header and footer off the first page.
    pub(crate) skip_first: bool,
    /// Save as PDF/A-3b.
    pub(crate) archival: bool,
}

/// The find and replace bar of a written document.
#[derive(Debug, Clone, Default)]
pub(crate) struct FindState {
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) match_case: bool,
    /// Typing goes to the bar's fields rather than the document.
    pub(crate) focused: bool,
}

/// The password dialog of an encrypted PDF.
#[derive(Debug, Clone)]
pub(crate) struct PasswordPrompt {
    pub(crate) origin: Origin,
    pub(crate) password: String,
    pub(crate) reveal: bool,
    /// The last password was wrong.
    pub(crate) wrong: bool,
    /// Waiting for the engine to try the password.
    pub(crate) checking: bool,
}

pub(crate) fn password_input_id() -> iced::widget::Id {
    iced::widget::Id::new("password")
}

pub(crate) fn find_input_id() -> iced::widget::Id {
    iced::widget::Id::new("find")
}

/// Fonts loaded on a background thread, handed over once.
#[derive(Clone)]
pub struct FontsSlot(Arc<std::sync::Mutex<Option<Fonts>>>);

impl FontsSlot {
    fn new(fonts: Option<Fonts>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(fonts)))
    }

    fn take(&self) -> Option<Fonts> {
        self.0.lock().ok().and_then(|mut f| f.take())
    }
}

impl fmt::Debug for FontsSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FontsSlot")
    }
}

/// What a PDF turned out to be when checked for a Ferrix document source.
#[derive(Debug, Clone)]
pub enum Checked {
    Writable(Box<ferrix_compose::Document>),
    /// Written by Ferrix, but changed by another app since.
    Changed,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug)]
pub struct App {
    pub(crate) engine: Result<Worker, String>,
    window: Option<window::Id>,
    pub(crate) window_size: iced::Size,
    pub(crate) scale_factor: f32,
    pub(crate) appearance: Appearance,
    system_dark: bool,
    /// The toolbar's overflow menu (`⋯`) is open.
    pub(crate) menu_open: bool,
    /// Markup mode (annotation tools), while on.
    pub(crate) markup: Option<crate::markup::Markup>,
    /// The note whose text is being edited.
    pub(crate) note: Option<crate::annotate_ui::NoteEditor>,
    /// Asking whether to annotate a signed document.
    pub(crate) signed_prompt: bool,
    pub(crate) sidebar_tab: crate::annotate_ui::SidebarTab,
    /// The author name for new annotations and replies, asked for once per session (the
    /// settings store of ADR-0005 doesn't exist yet), and the field asking for it.
    pub(crate) author: Option<String>,
    pub(crate) author_input: String,
    /// Text selected on a page.
    pub(crate) text_selection: Option<crate::text_select::TextSelection>,
    pub(crate) sidebar: bool,
    pub(crate) zoom: Zoom,
    pub(crate) doc: Option<Document>,
    pub(crate) page_input: String,
    pub(crate) error: Option<String>,
    pub(crate) notice: Option<String>,
    /// Set while a save or export runs; editing is paused until it finishes.
    pub(crate) busy: Option<&'static str>,
    /// Asking whether to save unsaved changes before doing this.
    pub(crate) confirm: Option<Proceed>,
    after_save: Option<Proceed>,
    /// Page to return to when a document is reopened after saving.
    reopen_at: Option<usize>,
    pub(crate) modifiers: keyboard::Modifiers,
    /// Installed conversion tools, detected in the background at startup.
    pub(crate) converter: Option<Arc<Converter>>,
    /// The New Document dialog, while open.
    pub(crate) new_form: Option<NewForm>,
    /// Crash-recovery journal of the current document while it has unsaved changes.
    journal: Option<Journal>,
    /// Unsaved work from crashed sessions, offered one at a time.
    pub(crate) recoveries: Vec<Recoverable>,
    /// The document being written, when one is open (instead of `doc`).
    pub(crate) writer: Option<Writer>,
    /// Fonts for writing: loaded in the background at startup, reused between documents.
    fonts: Option<Fonts>,
    writer_journal: Option<WriterJournal>,
    /// The Page Setup dialog, while open.
    pub(crate) page_setup: Option<PageSetupForm>,
    /// Shown once the document being opened is open.
    open_notice: Option<String>,
    /// The find and replace bar, while open.
    pub(crate) find: Option<FindState>,
    /// Formatted copy and paste for written documents.
    clipboard: crate::clipboard::RichClipboard,
    /// Preferences as last saved (ADR-0005); compared after each update to save changes.
    settings: ferrix_store::Settings,
    /// Recent files and positions; `None` until opened, or if it can't be.
    history: Option<crate::prefs::History>,
    /// Open Recent, newest first.
    pub(crate) recent: Vec<ferrix_store::RecentFile>,
    /// How pages are arranged.
    pub(crate) layout: Layout,
    /// The Open Recent flyout of the `⋯` menu is open.
    pub(crate) recent_open: bool,
    /// Asking for the password of an encrypted PDF.
    pub(crate) password: Option<PasswordPrompt>,
    /// A web or mail link waiting for confirmation.
    pub(crate) link_prompt: Option<String>,
    /// Where the hovered link goes, for the status bar.
    pub(crate) link_hint: Option<String>,
    /// Pages shown with inverted colours.
    pub(crate) invert: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenDialog,
    Open(PathBuf),
    /// A file from Open Recent; forgotten if it's gone.
    OpenRecent(PathBuf),
    ClearRecent,
    ToggleRecent,
    HistoryReady(Result<(crate::prefs::History, Vec<ferrix_store::RecentFile>), String>),
    RecentChanged(Vec<ferrix_store::RecentFile>),
    SetLayout(Layout),
    ToggleInvert,
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
    ToggleMenu,
    Annot(crate::annotate_ui::AnnotMsg),
    /// A page's text arrived (document, page, text).
    TextLoaded(
        DocId,
        PageKey,
        Result<
            (
                std::sync::Arc<ferrix_text::PageText>,
                crate::navigate::Links,
            ),
            String,
        >,
    ),
    /// The file is encrypted: ask for its password.
    NeedsPassword(Origin),
    PasswordInput(String),
    PasswordReveal,
    PasswordSubmit,
    PasswordCancel,
    OutlineLoaded(DocId, Result<Vec<ferrix_engine::OutlineItem>, String>),
    /// Open or close an outline entry (index, with its descendants).
    OutlineToggle(usize, bool),
    OutlineJump(usize),
    /// A link on a page was clicked: the release that ends the press, then the link.
    LinkReleased(PageKey, ferrix_types::PdfPoint, ferrix_engine::LinkTarget),
    /// The pointer moved onto a link, or off one.
    LinkHover(PageKey, Option<ferrix_engine::LinkTarget>),
    /// The link confirmation: open it or not.
    LinkAnswer(bool),
    CopyLink,
    Back,
    Forward,
    /// Space / Shift+Space: one screen down (true) or up.
    ScreenScroll(bool),
    /// The scroll wheel turned; with ⌘ (Ctrl elsewhere) held it zooms by this factor.
    Wheel(f32),
    CopyText,
    SelectAllText,
    /// Mark the selected text: style and palette colour index.
    MarkSelection(ferrix_core::MarkupStyle, usize),
    SidebarTab(crate::annotate_ui::SidebarTab),
    SetAppearance(Appearance),
    SystemTheme(iced::theme::Mode),
    Measured(
        DocId,
        Result<Vec<(Size, ferrix_types::PageTransform)>, String>,
    ),
    /// A page image: (document, page, rotation, the engine document it was rendered from,
    /// scale factor, result).
    Rendered(
        DocId,
        PageKey,
        Rotation,
        DocId,
        f32,
        Result<Rendered, String>,
    ),
    /// The page view scrolled: vertical and horizontal offsets.
    PageScrolled(f32, f32),
    /// A band of tiles arrived (document, page, level, tiles).
    TilesRendered(
        DocId,
        PageKey,
        crate::render::Level,
        Result<Vec<ferrix_render::TilePixels>, String>,
    ),
    /// The thumbnail column scrolled: offset and visible height.
    ThumbsScrolled(f32, f32),
    Thumbnail(DocId, PageKey, Rotation, DocId, Result<Rendered, String>),
    WindowOpened(Option<window::Id>),
    ScaleFactor(f32),
    Resized(iced::Size),
    ModifiersChanged(keyboard::Modifiers),
    DragWindow,
    /// The title bar (the toolbar on macOS) was double-clicked.
    TitleBarDoubleClick,
    TitleBarAction(crate::titlebar::TitleBarAction),
    DismissStatus,
    Print,
    PrintReady(Result<PathBuf, String>),
    Printed(Result<PrintOutcome, String>),
    ThumbnailClicked(PageKey),
    Rotate(Rotation),
    Delete,
    Move {
        earlier: bool,
    },
    InsertDialog,
    Insert(PathBuf),
    Inserted(Origin, Result<DocInfo, String>),
    Undo,
    Redo,
    Save,
    SaveAs,
    SaveAsPicked(Option<PathBuf>),
    Saved(Result<PathBuf, String>),
    /// A preview of the document with its annotation changes is open in the engine
    /// (document, generation, preview engine document or `None` for the original).
    PreviewReady(DocId, u64, Result<Option<DocInfo>, String>),
    /// A source's annotations were read (engine document, source index).
    AnnotationsLoaded(
        DocId,
        ferrix_core::SourceId,
        Result<crate::annotate::Loaded, String>,
    ),
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
    RecoveryFound(Vec<Recoverable>),
    Recovery(Recovery),
    Recovered(Box<Recoverable>, Result<Vec<DocInfo>, String>),
    Write(WriteAction),
    FontsReady(FontsSlot),
    CreateWriting,
    WritingChecked(PathBuf, Result<Checked, String>),
    WriterSaved(Result<PathBuf, String>),
    PageSetupDialog,
    PageSetupPaper(Paper),
    PageSetupOrientation(Orientation),
    PageSetupMargins(MarginChoice),
    PageSetupTitle(String),
    PageSetupHeader(String),
    PageSetupFooter(String),
    PageSetupSkipFirst(bool),
    PageSetupArchival(bool),
    ApplyPageSetup,
    FindOpen,
    FindQuery(String),
    FindReplacement(String),
    FindCase(bool),
    FindNext,
    FindPrevious,
    ReplaceOne,
    ReplaceAll,
    FindClose,
}

pub(crate) fn page_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("page")
}

pub(crate) fn sidebar_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("sidebar")
}

pub(crate) fn writer_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("writer")
}

impl App {
    pub(crate) fn new(
        engine: Result<Worker, String>,
        initial: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        let error = engine.as_ref().err().cloned();
        let settings = crate::prefs::load_settings();
        let app = Self {
            engine,
            window: None,
            window_size: iced::Size::new(1180.0, 860.0),
            scale_factor: 1.0,
            appearance: Appearance::from_saved(settings.appearance),
            system_dark: false,
            menu_open: false,
            markup: None,
            note: None,
            signed_prompt: false,
            sidebar_tab: crate::annotate_ui::SidebarTab::Pages,
            text_selection: None,
            author: settings.author.clone(),
            author_input: String::new(),
            sidebar: settings.sidebar,
            zoom: Zoom::from_saved(settings.zoom),
            layout: settings.layout,
            invert: settings.invert_pages,
            doc: None,
            page_input: String::new(),
            error,
            notice: None,
            busy: None,
            confirm: None,
            after_save: None,
            reopen_at: None,
            modifiers: keyboard::Modifiers::default(),
            converter: None,
            new_form: None,
            journal: None,
            recoveries: Vec::new(),
            writer: None,
            fonts: None,
            writer_journal: None,
            page_setup: None,
            open_notice: None,
            find: None,
            clipboard: crate::clipboard::RichClipboard::default(),
            settings,
            history: None,
            recent: Vec::new(),
            link_prompt: None,
            link_hint: None,
            password: None,
            recent_open: false,
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
            Task::perform(crate::prefs::History::open(), Message::HistoryReady),
            Task::perform(blocking(|| Ok(crate::recovery::scan())), |found| {
                Message::RecoveryFound(found.unwrap_or_default())
            }),
            Task::perform(blocking(|| Ok(Fonts::with_system_fonts())), |fonts| {
                Message::FontsReady(FontsSlot::new(fonts.ok()))
            }),
        ]);
        (app, boot.chain(open))
    }

    pub(crate) fn title(&self) -> String {
        if let Some(w) = &self.writer {
            let edited = if w.editor.is_dirty() {
                " — Edited"
            } else {
                ""
            };
            return format!("{}{edited} — Ferrix", w.name);
        }
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
        (self.doc.as_ref().is_some_and(|d| d.encrypted.is_none()) || self.writer.is_some())
            && self.busy.is_none()
            && self.page_setup.is_none()
            && self.confirm.is_none()
            && self.new_form.is_none()
            && self.recoveries.is_empty()
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        Task::batch([task, self.save_settings()])
    }

    /// Saves the preferences if this update changed any of them.
    fn save_settings(&mut self) -> Task<Message> {
        let mut settings = self.settings.clone();
        settings.appearance = self.appearance.to_saved();
        settings.author.clone_from(&self.author);
        settings.sidebar = self.sidebar;
        settings.zoom = self.zoom.to_saved();
        settings.layout = self.layout;
        settings.invert_pages = self.invert;
        if settings == self.settings {
            return Task::none();
        }
        self.settings = settings.clone();
        Task::future(crate::prefs::save_settings(settings)).discard()
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenRecent(path) => {
                self.menu_open = false;
                self.recent_open = false;
                if path.is_file() {
                    return self.guard(Proceed::Open(path));
                }
                self.error = Some(format!(
                    "“{}” couldn’t be found. It may have been moved or deleted, so it’s been \
                     removed from Recent.",
                    file_name(&path)
                ));
                self.recent.retain(|r| r.path != path);
                if let Some(history) = self.history.clone() {
                    return Task::perform(history.forget(path), Message::RecentChanged);
                }
            }
            Message::ToggleRecent => self.recent_open = !self.recent_open,
            Message::ClearRecent => {
                self.menu_open = false;
                self.recent_open = false;
                self.recent.clear();
                self.notice = Some("Recent documents cleared.".into());
                if let Some(history) = self.history.clone() {
                    return Task::perform(history.clear(), Message::RecentChanged);
                }
            }
            Message::NeedsPassword(origin) => {
                self.reopen_at = None;
                match self.password.as_mut() {
                    // A wrong password: say so and select it, so typing replaces it.
                    Some(prompt) if prompt.checking => {
                        prompt.checking = false;
                        prompt.wrong = true;
                    }
                    _ => {
                        self.password = Some(PasswordPrompt {
                            origin,
                            password: String::new(),
                            reveal: false,
                            wrong: false,
                            checking: false,
                        });
                    }
                }
                return Task::batch([
                    operation::focus(password_input_id()),
                    operation::select_all(password_input_id()),
                ]);
            }
            Message::PasswordInput(value) => {
                if let Some(prompt) = self.password.as_mut() {
                    prompt.password = value;
                    prompt.wrong = false;
                }
            }
            Message::PasswordReveal => {
                if let Some(prompt) = self.password.as_mut() {
                    prompt.reveal = !prompt.reveal;
                }
            }
            Message::PasswordCancel => self.password = None,
            Message::PasswordSubmit => {
                let Some(prompt) = self.password.as_mut().filter(|p| !p.checking) else {
                    return Task::none();
                };
                if prompt.password.is_empty() {
                    return Task::none();
                }
                prompt.checking = true;
                let (origin, secret) = (
                    prompt.origin.clone(),
                    ferrix_engine::Secret::new(prompt.password.clone()),
                );
                return self.load_with(origin, Some(secret), Message::Opened);
            }
            Message::OutlineLoaded(doc, result) => self.outline_loaded(doc, result),
            Message::OutlineToggle(index, all) => self.outline_toggle(index, all),
            Message::OutlineJump(index) => return self.outline_jump(index),
            Message::LinkReleased(page, at, target) => {
                self.link_hint = None;
                let release = self.annot_update(crate::annotate_ui::AnnotMsg::Pointer(
                    page,
                    crate::markup_canvas::Pointer::Release(at),
                ));
                // A click leaves no selection behind.
                self.text_selection = None;
                return Task::batch([release, self.follow_link(page, target)]);
            }
            Message::LinkHover(page, target) => self.link_hover(page, target),
            Message::LinkAnswer(open) => return self.link_answer(open),
            Message::CopyLink => return self.copy_link(),
            Message::Back => return self.go_back(),
            Message::ScreenScroll(down) => {
                let height = self.viewport_height();
                let Some(doc) = self.doc.as_ref().filter(|_| self.writer.is_none()) else {
                    return Task::none();
                };
                // One screen, keeping 48 of overlap so the reader doesn't lose the line.
                let step = (height - 48.0).max(48.0);
                let max = (doc.strip.content.height - height).max(0.0);
                let top = if down {
                    doc.scroll + step
                } else {
                    doc.scroll - step
                }
                .clamp(0.0, max);
                return self.scroll_pages_to(top);
            }
            Message::Wheel(factor) => {
                let zoom_key = if cfg!(target_os = "macos") {
                    self.modifiers.logo()
                } else {
                    self.modifiers.control()
                };
                if !zoom_key || self.confirm.is_some() || self.new_form.is_some() {
                    return Task::none();
                }
                let current = match &self.writer {
                    Some(w) => Some(w.zoom / LOGICAL_PX_PER_POINT),
                    None => self
                        .doc
                        .as_ref()
                        .filter(|d| d.is_measured())
                        .map(|d| d.strip.zoom),
                };
                if let Some(current) = current {
                    let next = (current * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                    if (next - current).abs() > 1e-4 {
                        return self.set_zoom(Zoom::Fixed(next));
                    }
                }
            }
            Message::Forward => return self.go_forward(),
            Message::HistoryReady(Ok((history, recent))) => {
                self.history = Some(history);
                self.recent = recent;
            }
            Message::HistoryReady(Err(e)) => tracing::warn!("history: {e}"),
            Message::RecentChanged(recent) => self.recent = recent,
            Message::SetLayout(layout) => {
                self.menu_open = false;
                if layout != self.layout {
                    self.layout = layout;
                    return self.render();
                }
            }
            Message::ToggleInvert => {
                self.menu_open = false;
                self.invert = !self.invert;
                if let Some(doc) = self.doc.as_mut() {
                    // Rendered pages and thumbnails carry the old colours.
                    doc.images.clear();
                    doc.tiles.clear();
                    doc.thumbnails.clear();
                }
                return Task::batch([self.render_pages(), self.next_thumbnail()]);
            }
            Message::OpenDialog => {
                self.menu_open = false;
                return self.guard(Proceed::OpenDialog);
            }
            Message::Open(path) => return self.guard(Proceed::Open(path)),
            Message::DialogClosed => {}
            Message::Opened(origin, Ok(info)) => {
                let password = self.password.take().filter(|p| p.checking);
                let task = self.opened(origin, info);
                if let (Some(doc), Some(prompt)) = (self.doc.as_mut(), password) {
                    doc.encrypted = Some(ferrix_engine::Secret::new(prompt.password));
                    self.notice =
                        Some("This PDF is encrypted, so Ferrix shows it read-only for now.".into());
                }
                return task;
            }
            Message::Opened(origin, Err(e)) => {
                self.reopen_at = None;
                self.password = None;
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
                    Ok(n) if n >= 1 => self.jump(n - 1, None),
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
            Message::ToggleMenu => {
                self.menu_open = !self.menu_open;
                self.recent_open = false;
            }
            Message::Annot(msg) => return self.annot_update(msg),
            Message::TextLoaded(doc_id, page, result) => {
                return self.text_loaded(doc_id, page, result);
            }
            Message::CopyText => return self.copy_text(),
            Message::SelectAllText => self.select_page_text(),
            Message::MarkSelection(style, color) => {
                if let Some((_, color)) = crate::markup::COLORS.get(color) {
                    return self.mark_selection(style, *color);
                }
            }
            Message::SidebarTab(tab) => {
                self.sidebar_tab = tab;
                self.sidebar = true;
            }
            Message::SetAppearance(appearance) => {
                self.appearance = appearance;
                self.menu_open = false;
            }
            Message::SystemTheme(mode) => self.system_dark = mode == iced::theme::Mode::Dark,
            Message::Measured(doc_id, result) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let Some(source) = doc.sources.iter_mut().find(|s| s.doc == doc_id) else {
                    return Task::none(); // the document was closed meanwhile
                };
                match result {
                    Ok(pages) => {
                        source.sizes = pages.iter().map(|(s, _)| *s).collect();
                        // Until (or unless) the annotations are read, which read them too.
                        if source.transforms.is_empty() {
                            source.transforms = pages.into_iter().map(|(_, t)| t).collect();
                        }
                    }
                    Err(e) => self.error = Some(format!("Couldn’t read the page sizes: {e}")),
                }
                return self.render();
            }
            Message::Rendered(doc_id, key, rotation, source, scale_factor, result) => {
                let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
                    return Task::none();
                };
                doc.render_busy = false;
                match result {
                    // Rendered before Dark Pages was switched; render it again.
                    Ok(r) if r.inverted != self.invert => {}
                    Ok(r) => {
                        doc.images.insert(
                            key,
                            Shown {
                                rotation,
                                source,
                                zoom: r.zoom,
                                scale_factor,
                                base: r.base,
                                image: r.into_image(),
                            },
                        );
                    }
                    Err(e) => {
                        let page = doc.state.pages.position(key).map_or(0, |i| i + 1);
                        self.error = Some(format!("Couldn’t show page {page}: {e}"));
                        doc.render_failed.insert(key);
                    }
                }
                return Task::batch([self.render_pages(), self.load_texts()]);
            }
            Message::PageScrolled(top, left) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.scroll_x = left;
                }
                return self.page_scrolled(top);
            }
            Message::TilesRendered(doc_id, key, level, result) => {
                return self.tiles_rendered(doc_id, key, level, result);
            }
            Message::ThumbsScrolled(top, height) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.thumb_scroll = top;
                    doc.thumb_view = height;
                }
                return self.next_thumbnail();
            }
            Message::Thumbnail(doc_id, key, rotation, source, result) => {
                let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
                    return Task::none(); // the document was closed meanwhile
                };
                doc.thumbnail_busy = false;
                if let Ok(r) = result
                    && r.inverted == self.invert
                {
                    doc.thumbnails.insert(
                        key,
                        Thumbnail {
                            rotation,
                            source,
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
            // Read the system setting each time, off the UI thread, so a change in System
            // Settings applies without restarting.
            Message::TitleBarDoubleClick => {
                return Task::perform(
                    crate::worker::blocking(|| Ok(crate::titlebar::preference())),
                    |action| Message::TitleBarAction(action.unwrap_or_default()),
                );
            }
            Message::TitleBarAction(action) => {
                use crate::titlebar::TitleBarAction;
                if let Some(id) = self.window {
                    return match action {
                        TitleBarAction::Zoom => window::toggle_maximize(id),
                        TitleBarAction::Minimize => window::minimize(id, true),
                        TitleBarAction::Nothing => Task::none(),
                    };
                }
            }
            Message::DismissStatus => {
                self.error = None;
                self.notice = None;
            }
            Message::Print => {
                self.menu_open = false;
                return self.print();
            }
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
                let Some(doc) = &mut self.doc else {
                    return Task::none();
                };
                let before = doc.current;
                doc.click(key, click);
                let index = doc.current;
                self.sync_page_input();
                return if index == before {
                    Task::none()
                } else {
                    self.show_page(index)
                };
            }
            Message::Rotate(by) => {
                let Some(doc) = self.editable() else {
                    return Task::none();
                };
                let keys = doc.targets();
                let label = pages_label("Rotate", keys.len());
                return self.apply(Edit::Rotate { keys, by }, label);
            }
            // With an annotation selected in Markup mode, Delete removes it, never the page.
            Message::Delete if self.markup.as_ref().is_some_and(|m| m.selected.is_some()) => {
                return self.delete_selected();
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
                let positions: Vec<usize> = keys
                    .iter()
                    .filter_map(|k| doc.state.pages.position(*k))
                    .collect();
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
                match doc.state.pages.move_edit(&keys, to) {
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
            Message::Undo if self.writer.is_some() => {
                if self.can_edit()
                    && let Some(w) = &mut self.writer
                    && w.editor.undo()
                {
                    return self.after_write(true);
                }
            }
            Message::Redo if self.writer.is_some() => {
                if self.can_edit()
                    && let Some(w) = &mut self.writer
                    && w.editor.redo()
                {
                    return self.after_write(true);
                }
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
            Message::Save if self.writer.is_some() => {
                let Some(w) = self.writer.as_ref().filter(|_| self.busy.is_none()) else {
                    return Task::none();
                };
                return match w.path.clone() {
                    Some(path) if w.editor.is_dirty() => self.save_writer(path),
                    Some(_) => {
                        // Nothing to save; still carry on with what was waiting for a save.
                        match self.after_save.take() {
                            Some(next) => self.proceed(next),
                            None => Task::none(),
                        }
                    }
                    None => self.update(Message::SaveAs),
                };
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
            Message::SaveAs if self.writer.is_some() => {
                let Some(w) = self.writer.as_ref().filter(|_| self.busy.is_none()) else {
                    return Task::none();
                };
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Save As")
                    .add_filter("PDF documents", &["pdf"])
                    .set_file_name(&w.name);
                if let Some(dir) = w.path.as_deref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                }
                return Task::perform(dialog.save_file(), |file| {
                    Message::SaveAsPicked(file.map(|f| f.path().to_path_buf()))
                });
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
            Message::SaveAsPicked(Some(path)) if self.writer.is_some() => {
                return self.save_writer(path);
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
            Message::AnnotationsLoaded(engine_doc, source, result) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                // The document may have been closed or replaced meanwhile.
                if doc.sources.get(source.0 as usize).map(|s| s.doc) != Some(engine_doc) {
                    return Task::none();
                }
                match result {
                    Ok(loaded) => {
                        if doc.adopt_annotations(source, loaded) {
                            return self.refresh_preview();
                        }
                    }
                    // Viewing still works; editing annotations stays off for this document.
                    Err(e) => tracing::warn!("couldn’t read annotations: {e}"),
                }
            }
            Message::PreviewReady(doc_id, generation, result) => {
                let engine = self.engine.clone().ok();
                let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
                    return close_engine_doc(engine, result);
                };
                if generation != doc.preview_gen {
                    return close_engine_doc(engine, result); // a newer preview is on its way
                }
                match result {
                    Ok(info) => {
                        let shown = info.map_or(doc.info.id, |i| i.id);
                        let Some(base) = doc.sources.first_mut() else {
                            return Task::none();
                        };
                        let old = std::mem::replace(&mut base.doc, shown);
                        // The original stays open (it is the document's identity); previews go.
                        let close = (old != doc.info.id && old != shown).then_some(old);
                        let task = self.render_pages();
                        let thumbs = self.next_thumbnail();
                        return Task::batch([task, thumbs, close_engine_id(engine, close)]);
                    }
                    Err(e) => self.error = Some(format!("Couldn’t show the annotations: {e}")),
                }
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
            Message::Escape if self.link_prompt.is_some() => self.link_prompt = None,
            Message::Escape if self.password.as_ref().is_some_and(|p| !p.checking) => {
                self.password = None;
            }
            Message::Escape if self.menu_open => self.menu_open = false,
            Message::Escape if self.signed_prompt => self.signed_prompt = false,
            Message::Escape if self.note.is_some() => return self.close_note(),
            Message::Escape if self.text_selection.is_some() => self.text_selection = None,
            // In Markup mode, Esc cancels a drag, then returns to Select, then deselects.
            Message::Escape
                if self.markup.as_ref().is_some_and(|m| {
                    m.gesture.is_some()
                        || m.tool != crate::markup::Tool::Select
                        || m.selected.is_some()
                }) =>
            {
                if let Some(m) = self.markup.as_mut()
                    && m.gesture.take().is_none()
                {
                    if m.tool == crate::markup::Tool::Select {
                        m.selected = None;
                    } else {
                        m.tool = crate::markup::Tool::Select;
                    }
                }
            }
            Message::Escape => {
                if self.confirm.is_none() && self.new_form.is_none() && self.page_setup.is_none() {
                    if self.writer.is_none() {
                        self.close_pdf_find();
                    } else {
                        self.find = None;
                    }
                }
                self.confirm = None;
                self.new_form = None;
                self.page_setup = None;
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
            Message::NewDialog => {
                self.menu_open = false;
                return self.guard(Proceed::NewDialog);
            }
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
            Message::RecoveryFound(found) => self.recoveries = found,
            Message::Recovery(choice) => {
                if self.busy.is_some() || self.recoveries.is_empty() {
                    return Task::none();
                }
                match choice {
                    Recovery::Recover => return self.guard(Proceed::Recover),
                    Recovery::Later => {
                        self.recoveries.remove(0);
                    }
                    Recovery::Discard => self.recoveries.remove(0).discard(),
                }
            }
            Message::Recovered(found, Ok(infos)) => return self.recovered(*found, infos),
            Message::Recovered(found, Err(e)) => {
                self.busy = None;
                self.error = Some(format!("Couldn’t recover “{}”: {e}", found.name()));
            }
            Message::Write(action) => return self.write(action),
            Message::FontsReady(slot) => {
                if self.fonts.is_none() {
                    self.fonts = slot.take();
                }
            }
            Message::CreateWriting => {
                let Some(form) = self.new_form.take() else {
                    return Task::none();
                };
                let size = match form.orientation {
                    Orientation::Portrait => form.paper.size(),
                    Orientation::Landscape => form.paper.size().landscape(),
                };
                let doc =
                    ferrix_compose::Document::with_page(PageSetup::new((size.width, size.height)));
                let editor = Editor::new(doc, self.take_fonts());
                return self.open_writer(Writer::new(editor, None, None));
            }
            Message::WritingChecked(path, result) => {
                return match result {
                    Ok(Checked::Writable(doc)) => {
                        let stamp = FileStamp::of(&path).ok();
                        let editor = Editor::new(*doc, self.take_fonts());
                        self.open_writer(Writer::new(editor, Some(path), stamp))
                    }
                    Ok(Checked::Changed) => {
                        self.open_notice = Some(format!(
                            "“{}” was changed by another app after Ferrix wrote it, so it opens as \
                             an ordinary PDF",
                            file_name(&path)
                        ));
                        self.load(Origin::File(path), Message::Opened)
                    }
                    Ok(Checked::Plain) | Err(_) => self.load(Origin::File(path), Message::Opened),
                };
            }
            Message::WriterSaved(Ok(path)) => {
                self.busy = None;
                if let Some(w) = &mut self.writer {
                    w.stamp = FileStamp::of(&path).ok();
                    w.name = file_name(&path);
                    w.path = Some(path.clone());
                    w.editor.mark_saved();
                }
                self.sync_writer_journal();
                self.notice = Some(format!("Saved “{}”", file_name(&path)));
                if let Some(next) = self.after_save.take() {
                    return self.proceed(next);
                }
            }
            Message::WriterSaved(Err(e)) => {
                self.busy = None;
                self.after_save = None;
                self.error = Some(format!("Couldn’t save: {e}"));
            }
            Message::PageSetupDialog => {
                self.menu_open = false;
                if let Some(w) = self.writer.as_ref().filter(|_| self.can_edit()) {
                    let doc = w.editor.document();
                    let page = doc.page;
                    let landscape = page.width > page.height;
                    let (short, long) = (page.width.min(page.height), page.width.max(page.height));
                    let paper = PAPERS
                        .into_iter()
                        .find(|p| {
                            let s = p.size();
                            (s.width - short).abs() < 1.0 && (s.height - long).abs() < 1.0
                        })
                        .unwrap_or(Paper::A4);
                    self.page_setup = Some(PageSetupForm {
                        paper,
                        orientation: if landscape {
                            Orientation::Landscape
                        } else {
                            Orientation::Portrait
                        },
                        margins: MarginChoice::nearest(page.margins.left),
                        title: doc.title.clone().unwrap_or_default(),
                        header: doc
                            .header
                            .as_ref()
                            .map(|t| t.text.clone())
                            .unwrap_or_default(),
                        footer: doc
                            .footer
                            .as_ref()
                            .map(|t| t.text.clone())
                            .unwrap_or_default(),
                        skip_first: doc.header.iter().chain(&doc.footer).any(|t| t.skip_first),
                        archival: doc.archival,
                    });
                }
            }
            Message::PageSetupPaper(paper) => {
                if let Some(form) = &mut self.page_setup {
                    form.paper = paper;
                }
            }
            Message::PageSetupOrientation(orientation) => {
                if let Some(form) = &mut self.page_setup {
                    form.orientation = orientation;
                }
            }
            Message::PageSetupMargins(margins) => {
                if let Some(form) = &mut self.page_setup {
                    form.margins = margins;
                }
            }
            Message::PageSetupTitle(title) => {
                if let Some(form) = &mut self.page_setup {
                    form.title = title.chars().take(200).collect();
                }
            }
            Message::PageSetupHeader(text) => {
                if let Some(form) = &mut self.page_setup {
                    form.header = text.chars().take(200).collect();
                }
            }
            Message::PageSetupFooter(text) => {
                if let Some(form) = &mut self.page_setup {
                    form.footer = text.chars().take(200).collect();
                }
            }
            Message::PageSetupSkipFirst(skip) => {
                if let Some(form) = &mut self.page_setup {
                    form.skip_first = skip;
                }
            }
            Message::PageSetupArchival(on) => {
                if let Some(form) = &mut self.page_setup {
                    form.archival = on;
                }
            }
            Message::FindOpen if self.writer.is_none() => {
                if self.doc.is_none() {
                    return Task::none();
                }
                // Start from selected text, if it is a short single line.
                let selected = self
                    .text_selection
                    .and_then(|s| Some(self.page_text(s.page)?.text(s.sel.range())))
                    .unwrap_or_default();
                let mut find = self.find.take().unwrap_or_default();
                if !selected.trim().is_empty() && selected.len() <= 200 && !selected.contains('\n')
                {
                    find.query = selected;
                }
                find.focused = true;
                self.find = Some(find);
                return Task::batch([
                    self.search_pdf(),
                    operation::focus(find_input_id()),
                    operation::select_all(find_input_id()),
                ]);
            }
            Message::FindOpen => {
                let Some(w) = self.writer.as_ref().filter(|_| self.can_edit()) else {
                    return Task::none();
                };
                // Start from the selected text, if it is a short single line.
                let selected = w.editor.selected_text();
                let mut find = self.find.take().unwrap_or_default();
                if !selected.is_empty() && selected.len() <= 200 && !selected.contains('\n') {
                    find.query = selected;
                }
                find.focused = true;
                self.find = Some(find);
                return Task::batch([
                    operation::focus(find_input_id()),
                    operation::select_all(find_input_id()),
                ]);
            }
            Message::FindQuery(query) => {
                if let Some(find) = &mut self.find {
                    find.query = query.chars().take(200).collect();
                    find.focused = true;
                }
                if self.writer.is_none() {
                    return self.search_pdf();
                }
            }
            Message::FindReplacement(text) => {
                if let Some(find) = &mut self.find {
                    find.replacement = text.chars().take(200).collect();
                    find.focused = true;
                }
            }
            Message::FindCase(on) => {
                if let Some(find) = &mut self.find {
                    find.match_case = on;
                }
                if self.writer.is_none() {
                    return self.search_pdf();
                }
            }
            Message::FindNext | Message::FindPrevious if self.writer.is_none() => {
                if self.find.is_none() {
                    return self.update(Message::FindOpen);
                }
                return self.step_match(matches!(message, Message::FindPrevious));
            }
            Message::FindNext
            | Message::FindPrevious
            | Message::ReplaceOne
            | Message::ReplaceAll => {
                let (Some(find), true) = (self.find.clone(), self.can_edit()) else {
                    return Task::none();
                };
                let Some(w) = &mut self.writer else {
                    return Task::none();
                };
                let e = &mut w.editor;
                let found = match message {
                    Message::FindPrevious => e.find_next(&find.query, find.match_case, true),
                    Message::ReplaceOne => {
                        e.replace(&find.query, &find.replacement, find.match_case)
                    }
                    Message::ReplaceAll => {
                        let count = e.replace_all(&find.query, &find.replacement, find.match_case);
                        self.notice = Some(match count {
                            0 => "Nothing to replace".to_owned(),
                            1 => "Replaced 1 match".to_owned(),
                            n => format!("Replaced {n} matches"),
                        });
                        true
                    }
                    _ => e.find_next(&find.query, find.match_case, false),
                };
                if !found && !find.query.is_empty() {
                    self.notice = Some(format!("“{}” not found", find.query));
                }
                return self.after_write(true);
            }
            Message::FindClose if self.writer.is_none() => self.close_pdf_find(),
            Message::FindClose => self.find = None,
            Message::ApplyPageSetup => {
                let (Some(form), Some(w)) = (self.page_setup.take(), self.writer.as_mut()) else {
                    return Task::none();
                };
                let size = match form.orientation {
                    Orientation::Portrait => form.paper.size(),
                    Orientation::Landscape => form.paper.size().landscape(),
                };
                let mut settings = w.editor.document().settings();
                settings.page = PageSetup::new((size.width, size.height));
                settings.page.margins = ferrix_compose::Margins::uniform(form.margins.points());
                let title = form.title.trim();
                settings.title = (!title.is_empty()).then(|| title.to_owned());
                let page_text = |text: &str| {
                    let text = text.trim();
                    (!text.is_empty()).then(|| ferrix_compose::PageText {
                        skip_first: form.skip_first,
                        ..ferrix_compose::PageText::new(text)
                    })
                };
                settings.header = page_text(&form.header);
                settings.footer = page_text(&form.footer);
                settings.archival = form.archival;
                if settings != w.editor.document().settings() {
                    w.editor.set_settings(settings);
                    return Task::batch([self.after_write(false), self.render()]);
                }
            }
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
                Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Back)) => {
                    Some(Message::Back)
                }
                Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Forward)) => {
                    Some(Message::Forward)
                }
                _ => None,
            }),
            // ⌘ + scroll on macOS (Ctrl + scroll is the system's zoom there), Ctrl elsewhere.
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => {
                    Some(Message::Wheel(wheel_factor(delta)))
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
        if self.doc.as_ref().is_some_and(Document::is_dirty)
            || self.writer.as_ref().is_some_and(|w| w.editor.is_dirty())
        {
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
                    self.check_writing(path)
                } else {
                    self.convert(Purpose::Open, vec![path])
                }
            }
            Proceed::NewDialog => {
                self.new_form = Some(NewForm::default());
                Task::none()
            }
            Proceed::Recover => self.start_recovery(),
            Proceed::Quit => {
                // The app exits before a background write could finish.
                if let (Some((path, view)), Some(history)) = (self.view_state(), &self.history) {
                    history.set_view_now(&path, view);
                }
                // Leaving with nothing unsaved (or by choice): no recovery needed.
                if let Some(journal) = self.journal.take() {
                    journal.discard();
                }
                if let Some(journal) = self.writer_journal.take() {
                    journal.discard();
                }
                iced::exit()
            }
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
        self.load_with(origin, None, done)
    }

    /// Opens `origin` with `password`; an encrypted file asks for its password instead of
    /// reporting an error.
    fn load_with(
        &mut self,
        origin: Origin,
        password: Option<ferrix_engine::Secret>,
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
                let result = engine
                    .call(move |e| e.open(source, password.as_ref()))
                    .await;
                (origin, result)
            },
            move |(origin, result)| match result {
                Ok(Err(ferrix_engine::EngineError::PasswordRequired)) => {
                    Message::NeedsPassword(origin)
                }
                other => done(origin, flatten(other)),
            },
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
        let mut touch = Task::none();
        if let Some(at) = self.reopen_at.take() {
            let at = at.min(doc.len() - 1);
            doc.current = at;
            if let Some(key) = doc.key_at(at) {
                doc.select_only(key);
            }
        } else {
            self.notice = self.open_notice.take();
            if let (Some(history), Some(path)) =
                (&self.history, doc.origin.path().map(PathBuf::from))
            {
                let saved = self
                    .settings
                    .restore_position
                    .then(|| history.view(&path))
                    .flatten();
                if let Some(view) = saved {
                    let at = (view.page as usize).min(doc.len() - 1);
                    doc.current = at;
                    doc.scroll_within = view.within;
                    if let Some(key) = doc.key_at(at) {
                        doc.select_only(key);
                    }
                    self.zoom = Zoom::from_saved(view.zoom);
                    self.layout = view.layout;
                }
                touch = Task::perform(history.clone().touch(path), Message::RecentChanged);
            }
        }
        let measure = self.measure(doc.info.id, doc.info.page_count);
        let annots = load_annotations(&doc.origin, doc.info.id, ferrix_core::SourceId::BASE);
        let outline = self.load_outline(doc.info.id);
        self.doc = Some(doc);
        self.sync_journal();
        self.sync_page_input();
        Task::batch([
            close,
            measure,
            annots,
            outline,
            touch,
            self.next_thumbnail(),
        ])
    }

    /// Opens the documents behind the first recoverable session.
    fn start_recovery(&mut self) -> Task<Message> {
        if let Some(found) = self.recoveries.first().cloned()
            && let Some(doc) = found.written.clone()
        {
            if found.base_changed {
                return Task::none();
            }
            let path = found.base.path().map(PathBuf::from);
            let stamp = path.as_deref().and_then(|p| FileStamp::of(p).ok());
            let editor = Editor::unsaved(doc, self.take_fonts());
            let mut writer = Writer::new(editor, path, stamp);
            writer.name = found.name();
            let task = self.open_writer(writer);
            if self.writer_journal.is_some() {
                found.discard();
            }
            self.recoveries.retain(|r| r.journal != found.journal);
            self.notice = Some(format!("Recovered unsaved changes to “{}”", found.name()));
            return task;
        }
        let (Some(found), Ok(engine)) = (self.recoveries.first().cloned(), self.engine.clone())
        else {
            return Task::none();
        };
        if found.base_changed {
            return Task::none();
        }
        self.busy = Some("Recovering…");
        Task::perform(
            async move {
                let mut infos = Vec::new();
                for origin in std::iter::once(&found.base).chain(&found.sources) {
                    let source = match origin {
                        Origin::File(path) => DocSource::Path(path.clone()),
                        Origin::Untitled { bytes, .. } => DocSource::Bytes(bytes.as_ref().clone()),
                    };
                    match flatten(engine.call(move |e| e.open(source, None)).await) {
                        Ok(info) => infos.push(info),
                        Err(e) => {
                            let ids: Vec<DocId> = infos.iter().map(|i: &DocInfo| i.id).collect();
                            let _ = engine
                                .call(move |e| ids.into_iter().for_each(|id| e.close(id)))
                                .await;
                            return (found, Err(e));
                        }
                    }
                }
                (found, Ok(infos))
            },
            |(found, result)| Message::Recovered(Box::new(found), result),
        )
    }

    fn recovered(&mut self, found: Recoverable, infos: Vec<DocInfo>) -> Task<Message> {
        self.busy = None;
        let counts: Vec<u32> = infos.iter().map(|i| i.page_count).collect();
        if !pages_fit(&found.pages, &counts) {
            self.error = Some(format!(
                "Couldn’t recover “{}”: its pages no longer match the files",
                found.name()
            ));
            if let Ok(engine) = self.engine.clone() {
                let ids: Vec<DocId> = infos.iter().map(|i| i.id).collect();
                return Task::future(async move {
                    engine
                        .call(move |e| ids.into_iter().for_each(|id| e.close(id)))
                        .await
                })
                .discard();
            }
            return Task::none();
        }
        let close = self.close_document();
        let base_and_sources: Vec<(DocId, u32)> =
            infos.iter().map(|i| (i.id, i.page_count)).collect();
        let base_and_sources = base_and_sources.into_iter();
        let mut infos = infos.into_iter();
        let Some(base_info) = infos.next() else {
            return close;
        };
        let sources = found
            .sources
            .iter()
            .cloned()
            .zip(infos)
            .map(|(origin, info)| Source::new(origin, info.id))
            .collect();
        let name = found.name();
        self.doc = Some(Document::recovered(
            found.base.clone(),
            base_info,
            sources,
            found.pages.clone(),
            found.annots.clone(),
        ));
        // The new session's journal now holds the work; the crashed one can go.
        self.sync_journal();
        if self.journal.is_some() {
            found.discard();
        }
        self.recoveries.retain(|r| r.journal != found.journal);
        self.notice = Some(format!("Recovered unsaved changes to “{name}”"));
        self.sync_page_input();
        let measure = Task::batch(base_and_sources.map(|(id, count)| self.measure(id, count)));
        Task::batch([close, measure, self.next_thumbnail()])
    }

    /// Keeps the recovery journal in step with the document: started on the first unsaved
    /// change, appended to on every change, deleted once nothing is unsaved.
    fn sync_journal(&mut self) {
        let dirty = self.doc.as_ref().filter(|d| d.is_dirty());
        let result = match (dirty, &mut self.journal) {
            (None, _) => {
                if let Some(journal) = self.journal.take() {
                    journal.discard();
                }
                Ok(())
            }
            (Some(doc), Some(journal)) => journal.record(doc),
            (Some(doc), None) => Journal::start(doc).map(|journal| self.journal = Some(journal)),
        };
        if let Err(e) = result {
            tracing::warn!("recovery journal: {e}");
            self.error = Some(format!("Couldn’t save recovery data: {e}"));
        }
    }

    /// Closes the current document and its inserted sources on the engine thread.
    fn close_document(&mut self) -> Task<Message> {
        if let Some(journal) = self.journal.take() {
            journal.discard();
        }
        if let Some(journal) = self.writer_journal.take() {
            journal.discard();
        }
        self.page_setup = None;
        self.find = None;
        self.markup = None;
        self.note = None;
        self.text_selection = None;
        self.signed_prompt = false;
        if let Some(writer) = self.writer.take() {
            self.fonts = Some(writer.editor.into_fonts());
        }
        let remember = self.remember_view();
        let (Some(doc), Ok(engine)) = (self.doc.take(), self.engine.clone()) else {
            return remember;
        };
        let ids = doc.engine_docs();
        let close = Task::future(async move {
            engine
                .call(move |e| ids.into_iter().for_each(|id| e.close(id)))
                .await
        })
        .discard();
        Task::batch([remember, close])
    }

    /// Where the open PDF is left, if it is a file whose position is worth keeping.
    fn view_state(&self) -> Option<(PathBuf, ferrix_store::ViewState)> {
        let doc = self.doc.as_ref().filter(|d| d.is_measured())?;
        let path = doc.origin.path()?.to_path_buf();
        let view = ferrix_store::ViewState {
            page: u32::try_from(doc.current).unwrap_or(0),
            within: doc.scroll_within,
            zoom: self.zoom.to_saved(),
            layout: self.layout,
        };
        Some((path, view))
    }

    /// Remembers where the open PDF was left, in the background.
    fn remember_view(&self) -> Task<Message> {
        match (self.view_state(), self.history.clone()) {
            (Some((path, view)), Some(history)) => {
                Task::future(history.set_view(path, view)).discard()
            }
            _ => Task::none(),
        }
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

    pub(crate) fn after_edit(&mut self) -> Task<Message> {
        self.sync_journal();
        self.sync_page_input();
        Task::batch([self.render(), self.next_thumbnail(), self.refresh_preview()])
    }

    /// Rebuilds the on-screen revision when the annotations differ from what is shown.
    fn refresh_preview(&mut self) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let Some(bytes) = doc.base_bytes.clone() else {
            return Task::none();
        };
        if doc.state.annots == doc.previewed {
            return Task::none();
        }
        doc.previewed = doc.state.annots.clone();
        doc.preview_gen += 1;
        let (doc_id, generation) = (doc.info.id, doc.preview_gen);
        let pages: Vec<_> = doc
            .state
            .pages
            .entries()
            .iter()
            .filter(|e| e.source == ferrix_core::SourceId::BASE)
            .copied()
            .collect();
        let annots = doc.state.annots.clone();
        Task::perform(
            async move {
                let Some(bytes) = crate::annotate::build_preview(bytes, pages, annots).await?
                else {
                    return Ok(None);
                };
                let info = flatten(
                    engine
                        .call(move |e| e.open(DocSource::Bytes(bytes), None))
                        .await,
                )
                .map_err(|e| e.to_string())?;
                Ok(Some(info))
            },
            move |result| Message::PreviewReady(doc_id, generation, result),
        )
    }

    fn inserted(&mut self, origin: Origin, info: DocInfo) -> Task<Message> {
        let Some(doc) = self.editable() else {
            return Task::none();
        };
        let source = ferrix_core::SourceId(doc.sources.len() as u32);
        let annots = load_annotations(&origin, info.id, source);
        doc.sources.push(Source::new(origin, info.id));
        let entries = doc
            .state
            .pages
            .new_entries(source, (0..info.page_count).map(ferrix_types::PageIndex));
        let at = doc.current + 1;
        let count = entries.len();
        let pages = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| (at + i, e))
            .collect();
        let measure = self.measure(info.id, info.page_count);
        let task = self.apply(Edit::Insert { pages }, pages_label("Insert", count));
        // Show the first inserted page.
        Task::batch([measure, annots, task, self.go_to(at)])
    }

    // ── Writing ───────────────────────────────────────────────────────────────

    /// Fonts for a new writer: those loaded at startup, or loaded now if they aren't ready yet.
    fn take_fonts(&mut self) -> Fonts {
        self.fonts.take().unwrap_or_else(Fonts::with_system_fonts)
    }

    /// Replaces whatever is open with a written document.
    fn open_writer(&mut self, writer: Writer) -> Task<Message> {
        let close = self.close_document();
        let touch = match (&self.history, &writer.path) {
            (Some(history), Some(path)) => {
                Task::perform(history.clone().touch(path.clone()), Message::RecentChanged)
            }
            _ => Task::none(),
        };
        self.error = None;
        self.writer = Some(writer);
        self.sync_writer_journal();
        self.refresh_writer();
        Task::batch([
            close,
            touch,
            operation::snap_to(writer_scroll_id(), scrollable::RelativeOffset::START),
        ])
    }

    /// Opens a PDF, for writing if Ferrix wrote it and nothing has changed it since.
    fn check_writing(&mut self, path: PathBuf) -> Task<Message> {
        let file = path.clone();
        Task::perform(
            blocking(move || {
                let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                // Cheap test before parsing: the source's file name is stored uncompressed.
                let name = ferrix_compose::SOURCE_NAME.as_bytes();
                if !bytes.windows(name.len()).any(|w| w == name) {
                    return Ok(Checked::Plain);
                }
                Ok(match ferrix_compose::open_pdf(&bytes) {
                    Ok(Opened::Editable(doc)) => Checked::Writable(Box::new(doc)),
                    Ok(Opened::Changed(_)) => Checked::Changed,
                    Ok(Opened::Plain) | Err(_) => Checked::Plain,
                })
            }),
            move |result| Message::WritingChecked(path.clone(), result),
        )
    }

    fn write(&mut self, action: WriteAction) -> Task<Message> {
        let scale_factor = self.scale_factor;
        if let WriteAction::Scrolled(offset, height) = action {
            if let Some(w) = &mut self.writer {
                w.viewport = (offset, height);
                w.refresh(scale_factor);
            }
            return Task::none();
        }
        if !self.can_edit() {
            return Task::none();
        }
        let Some(w) = &mut self.writer else {
            return Task::none();
        };
        let before = w.editor.revision();
        let mut task = Task::none();
        let mut follow = true;
        let e = &mut w.editor;
        match action {
            WriteAction::Scrolled(..) => {}
            WriteAction::Press {
                page,
                x,
                y,
                clicks,
                extend,
            } => {
                w.press(page, x, y, clicks, extend);
                follow = false;
                if let Some(find) = &mut self.find {
                    find.focused = false;
                }
            }
            WriteAction::Drag { page, x, y } => e.drag(page, x, y),
            WriteAction::Key(KeyCommand::Paste) => match self.clipboard.paste() {
                Some(crate::clipboard::Pasted::Formatted(fragment)) => e.paste_fragment(&fragment),
                Some(crate::clipboard::Pasted::Text(text)) => e.paste(&text),
                None => {
                    return iced::clipboard::read()
                        .map(|text| Message::Write(WriteAction::Pasted(text)));
                }
            },
            WriteAction::Key(KeyCommand::PastePlain) => match self.clipboard.paste_plain() {
                Some(text) => e.paste(&text),
                None => {
                    return iced::clipboard::read()
                        .map(|text| Message::Write(WriteAction::Pasted(text)));
                }
            },
            WriteAction::Key(command @ (KeyCommand::Copy | KeyCommand::Cut)) => {
                let fragment = e.copy_fragment();
                if let Some(text) = w.key(command)
                    && !fragment.is_some_and(|f| self.clipboard.copy(f))
                {
                    task = iced::clipboard::write(text);
                }
            }
            WriteAction::Key(command) => {
                if let Some(text) = w.key(command) {
                    task = iced::clipboard::write(text);
                }
            }
            WriteAction::Pasted(Some(text)) => e.paste(&text),
            WriteAction::Pasted(None) => {}
            WriteAction::Style(choice) => e.set_style(choice.0),
            WriteAction::Family(family) => e.set_family(&family),
            WriteAction::Size(size) => e.set_size(size.0),
            WriteAction::Toggle(toggle) => e.toggle(toggle),
            WriteAction::Color(choice) => e.set_color(choice.color),
            WriteAction::Align(align) => e.set_align(align),
            WriteAction::Spacing(spacing) => e.set_line_spacing(spacing.0),
            WriteAction::ClearFormatting => e.clear_formatting(),
            WriteAction::List(kind) => e.toggle_list(kind),
            WriteAction::PageBreak => e.page_break(),
            WriteAction::InsertImage => {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Insert Picture")
                    .add_filter("Pictures", &["png", "jpg", "jpeg"]);
                return Task::perform(
                    async move {
                        let file = dialog.pick_file().await?;
                        let path = file.path().to_path_buf();
                        Some(
                            blocking(move || {
                                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                                ferrix_compose::ImageData::from_bytes(bytes)
                                    .map(Box::new)
                                    .map_err(|e| e.to_string())
                            })
                            .await,
                        )
                    },
                    |result| match result {
                        Some(result) => Message::Write(WriteAction::ImageLoaded(result)),
                        None => Message::DialogClosed,
                    },
                );
            }
            WriteAction::ImageLoaded(Ok(data)) => e.insert_image(*data),
            WriteAction::Table(crate::writer::TableChoice::Insert(rows, cols)) => {
                e.insert_table(rows, cols);
            }
            WriteAction::Table(crate::writer::TableChoice::Op(op)) => e.table_op(op),
            WriteAction::ImageLoaded(Err(error)) => {
                self.error = Some(format!("Couldn’t insert the picture: {error}"));
                return Task::none();
            }
        }
        if self
            .writer
            .as_ref()
            .is_some_and(|w| w.editor.revision() != before)
        {
            self.notice = None;
            return Task::batch([task, self.after_write(follow)]);
        }
        task
    }

    /// After the document or selection changed: journal, redraw, and keep the caret in view.
    fn after_write(&mut self, follow: bool) -> Task<Message> {
        self.sync_writer_journal();
        let scale_factor = self.scale_factor;
        let Some(w) = &mut self.writer else {
            return Task::none();
        };
        w.refresh(scale_factor);
        if follow && let Some(y) = w.scroll_for_caret() {
            w.viewport.0 = y;
            w.refresh(scale_factor);
            return operation::scroll_to(
                writer_scroll_id(),
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y),
                },
            );
        }
        Task::none()
    }

    /// Applies the zoom to the writer and redraws its pages.
    fn refresh_writer(&mut self) {
        let canvas = crate::view::canvas_size(self.window_size, false);
        let (zoom, scale_factor) = (self.zoom, self.scale_factor);
        let Some(w) = &mut self.writer else {
            return;
        };
        let page = w.page_setup();
        let factor = zoom.resolve(
            Size {
                width: page.width,
                height: page.height,
            },
            canvas,
            Layout::Continuous,
        );
        w.zoom = factor * LOGICAL_PX_PER_POINT;
        w.refresh(scale_factor);
    }

    /// Keeps the writer's recovery journal in step: started on the first unsaved change, one
    /// record per edit, deleted once nothing is unsaved.
    fn sync_writer_journal(&mut self) {
        let Some(w) = &mut self.writer else {
            if let Some(journal) = self.writer_journal.take() {
                journal.discard();
            }
            return;
        };
        let edits = w.editor.take_applied();
        if !w.editor.is_dirty() {
            if let Some(journal) = self.writer_journal.take() {
                journal.discard();
            }
            return;
        }
        let result = match &mut self.writer_journal {
            Some(journal) => journal.record(&edits),
            None => WriterJournal::start(&w.name, w.path.as_deref(), w.editor.document())
                .map(|journal| self.writer_journal = Some(journal)),
        };
        if let Err(e) = result {
            tracing::warn!("recovery journal: {e}");
            self.error = Some(format!("Couldn’t save recovery data: {e}"));
        }
    }

    fn save_writer(&mut self, target: PathBuf) -> Task<Message> {
        let Some(w) = self.writer.as_ref().filter(|_| self.busy.is_none()) else {
            return Task::none();
        };
        let job = w.editor.export_job();
        let (opened, stamp) = (w.path.clone(), w.stamp);
        self.busy = Some("Saving…");
        self.error = None;
        Task::perform(
            blocking(move || {
                if opened.as_ref() == Some(&target)
                    && stamp.is_some()
                    && FileStamp::of(&target).ok() != stamp
                {
                    return Err(format!(
                        "“{}” was changed by another app after it was opened; use Save As",
                        file_name(&target)
                    ));
                }
                let bytes = job.run().map_err(|e| e.to_string())?;
                ferrix_io::write_atomic(&target, &bytes).map_err(|e| e.to_string())?;
                Ok(target)
            }),
            Message::WriterSaved,
        )
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
            state: doc.state.clone(),
        })
    }

    // ── Printing ──────────────────────────────────────────────────────────────

    /// Prints the document as shown: unsaved edits are exported to a temporary file first.
    fn print(&mut self) -> Task<Message> {
        if let Some(w) = self.writer.as_ref().filter(|_| self.busy.is_none()) {
            let job = w.editor.export_job();
            let temp = std::env::temp_dir().join(format!(
                "ferrix-print-{}-{}",
                std::process::id(),
                w.name
            ));
            self.busy = Some("Preparing to print…");
            return Task::perform(
                blocking(move || {
                    let bytes = job.run().map_err(|e| e.to_string())?;
                    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
                    Ok(temp)
                }),
                Message::PrintReady,
            );
        }
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
        let Some(window) = self.window else {
            return Task::none();
        };
        let (title, page_count) = match (&self.writer, &self.doc) {
            (Some(w), _) => (w.name.clone(), w.page_count() as u32),
            (None, Some(doc)) => (doc.name(), doc.len() as u32),
            (None, None) => return Task::none(),
        };
        let job = PrintJob {
            title,
            path,
            page_count,
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
    /// The window minus the Markup bar while it shows, for page-layout sums.
    fn view_window(&self) -> iced::Size {
        let mut size = self.window_size;
        if self.markup.is_some() {
            size.height -= crate::view::BAR_HEIGHT + 1.0;
        }
        size
    }

    pub(crate) fn canvas_size(&self) -> iced::Size {
        crate::view::canvas_size(self.view_window(), self.sidebar && self.doc.is_some())
    }

    pub(crate) fn sync_page_input(&mut self) {
        self.page_input = self
            .doc
            .as_ref()
            .map(|d| (d.current + 1).to_string())
            .unwrap_or_default();
    }

    fn set_zoom(&mut self, zoom: Zoom) -> Task<Message> {
        if self.doc.is_none() && self.writer.is_none() {
            return Task::none();
        }
        self.zoom = zoom;
        self.render()
    }

    fn step_zoom(&mut self, up: bool) -> Task<Message> {
        let shown = match &self.writer {
            Some(w) => Some(w.zoom / LOGICAL_PX_PER_POINT),
            None => self
                .doc
                .as_ref()
                .filter(|d| d.is_measured())
                .map(|d| d.strip.zoom),
        };
        let Some(current) = shown else {
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

    pub(crate) fn rerender_if_fitting(&mut self) -> Task<Message> {
        // A fixed zoom keeps the layout, but a different window shows different pages.
        match self.zoom {
            Zoom::Fixed(_) => self.render_pages(),
            Zoom::Auto | Zoom::FitWidth | Zoom::FitPage => self.render(),
        }
    }

    /// Height of the scrolled page view, in logical pixels.
    pub(crate) fn viewport_height(&self) -> f32 {
        crate::view::page_view_height(self.view_window())
    }

    fn step_page(&mut self, delta: isize) -> Task<Message> {
        let Some(doc) = &self.doc else {
            return Task::none();
        };
        let target = doc.current.saturating_add_signed(delta).min(doc.len() - 1);
        self.go_to(target)
    }

    pub(crate) fn go_to(&mut self, index: usize) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let index = index.min(doc.len().saturating_sub(1));
        if let Some(key) = doc.key_at(index) {
            doc.select_only(key);
        }
        doc.current = index;
        self.sync_page_input();
        self.show_page(index)
    }

    /// Scrolls the page view to the top of page `index`, and the sidebar to its thumbnail.
    fn show_page(&mut self, index: usize) -> Task<Message> {
        self.place_page(index);
        let height = self.viewport_height();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        doc.current = index;
        let top = doc.strip.offset_of(index, height);
        doc.scroll_within = doc.strip.fraction(index, top);
        Task::batch([
            self.scroll_pages_to(top),
            self.follow_in_sidebar(),
            self.render_pages(),
        ])
    }

    /// In the single page layout, lays the view out for page `index` if another page is
    /// shown. Other layouts place every page already.
    pub(crate) fn place_page(&mut self, index: usize) {
        let layout = self.layout;
        let Some(doc) = self.doc.as_mut().filter(|d| d.is_measured()) else {
            return;
        };
        if layout != Layout::SinglePage
            || doc.strip.rows.first().is_some_and(|r| r.contains(&index))
        {
            return;
        }
        let sizes = doc.page_sizes();
        doc.current = index;
        doc.strip = Strip::with_layout(&sizes, doc.strip.zoom, layout, index);
    }

    pub(crate) fn scroll_pages_to(&mut self, top: f32) -> Task<Message> {
        if let Some(doc) = &mut self.doc {
            doc.scroll = top;
        }
        operation::scroll_to(
            page_scroll_id(),
            scrollable::AbsoluteOffset {
                x: None,
                y: Some(top),
            },
        )
    }

    /// Keeps the current page's thumbnail in view.
    pub(crate) fn follow_in_sidebar(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let Some(top) = doc
            .thumb_column()
            .reveal(doc.current, doc.thumb_scroll, doc.thumb_view)
        else {
            return Task::none();
        };
        doc.thumb_scroll = top;
        Task::batch([
            operation::scroll_to(
                sidebar_scroll_id(),
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(top),
                },
            ),
            self.next_thumbnail(),
        ])
    }

    /// The user scrolled the pages: the page filling most of the view becomes current.
    fn page_scrolled(&mut self, top: f32) -> Task<Message> {
        let height = self.viewport_height();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        doc.scroll = top;
        let Some(index) = doc.strip.current(top, height) else {
            return self.render_pages();
        };
        doc.scroll_within = doc.strip.fraction(index, top);
        if index == doc.current {
            return self.render_pages();
        }
        doc.current = index;
        // A single selected page follows the view; a multiple selection is left alone.
        if doc.selection.len() <= 1
            && let Some(key) = doc.key_at(index)
        {
            doc.select_only(key);
        }
        self.sync_page_input();
        Task::batch([self.follow_in_sidebar(), self.render_pages()])
    }

    /// Lays the pages out for the current zoom and window, keeping the view on the same spot
    /// of the current page, then renders the pages in view.
    fn render(&mut self) -> Task<Message> {
        if self.writer.is_some() {
            self.refresh_writer();
            return Task::none();
        }
        let (zoom, canvas, height, layout) = (
            self.zoom,
            self.canvas_size(),
            self.viewport_height(),
            self.layout,
        );
        let Some(doc) = self.doc.as_mut().filter(|d| d.is_measured()) else {
            return Task::none();
        };
        let sizes = doc.page_sizes();
        // Fitting the largest page makes every page fit.
        let largest = sizes.iter().fold(Size::new(0.0, 0.0), |m, s| {
            Size::new(m.width.max(s.width), m.height.max(s.height))
        });
        let strip = Strip::with_layout(
            &sizes,
            zoom.resolve(largest, canvas, layout),
            layout,
            doc.current,
        );
        let first = doc.strip.len() == 0;
        let top = if first && doc.scroll_within == 0.0 {
            let top = strip.offset_of(doc.current, height);
            doc.scroll_within = strip.fraction(doc.current, top);
            top
        } else {
            strip.at_fraction(doc.current, doc.scroll_within, height)
        };
        doc.strip = strip;
        let scroll = if first || (top - doc.scroll).abs() > 0.5 {
            self.scroll_pages_to(top)
        } else {
            Task::none()
        };
        Task::batch([scroll, self.render_pages()])
    }

    /// Reads the size of every page of an engine document in the background.
    fn measure(&self, doc: DocId, pages: u32) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = engine
                    .call(move |e| {
                        (0..pages)
                            .map(|i| {
                                e.page_info(doc, PageIndex(i))
                                    .map(|p| (p.size, p.transform))
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .await;
                flatten(result)
            },
            move |result| Message::Measured(doc, result),
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
        let invert = self.invert;
        Task::perform(
            async move {
                let result = engine
                    .call(move |e| render_thumbnail(e, page, scale_factor, invert))
                    .await;
                flatten(result)
            },
            move |result| Message::Thumbnail(doc_id, key, page.rotation, page.doc, result),
        )
    }
}

fn render_thumbnail(
    engine: &mut PdfiumEngine,
    page: PageRef,
    scale_factor: f32,
    invert: bool,
) -> Result<Rendered, ferrix_engine::EngineError> {
    let size = crate::render::page_size(engine, page)?;
    // Fit a box THUMB_WIDTH wide, and not much taller, whatever the orientation.
    let scale = (THUMB_WIDTH / size.width).min(THUMB_WIDTH * 1.4 / size.height);
    render_page(engine, page, scale, scale_factor, 0.0, invert)
}

/// Closes a preview that is no longer wanted.
fn close_engine_doc(
    engine: Option<Worker>,
    result: Result<Option<DocInfo>, String>,
) -> Task<Message> {
    close_engine_id(engine, result.ok().flatten().map(|i| i.id))
}

fn close_engine_id(engine: Option<Worker>, id: Option<DocId>) -> Task<Message> {
    match (engine, id) {
        (Some(engine), Some(id)) => {
            Task::future(async move { engine.call(move |e| e.close(id)).await }).discard()
        }
        _ => Task::none(),
    }
}

/// Reads a source's annotations in the background.
fn load_annotations(
    origin: &Origin,
    engine_doc: DocId,
    source: ferrix_core::SourceId,
) -> Task<Message> {
    let origin = origin.clone();
    Task::perform(crate::annotate::load(origin, source), move |result| {
        Message::AnnotationsLoaded(engine_doc, source, result)
    })
}

/// Everything needed to write the edited document, gathered on the UI thread.
#[derive(Debug)]
struct Export {
    engine: Worker,
    base: Origin,
    stamp: Option<FileStamp>,
    imports: Vec<Origin>,
    layout: Vec<LayoutPage>,
    /// Pages and annotations; annotations are written in the same revision (ADR-0009 §6).
    state: ferrix_core::DocState,
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
            state,
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
            let mut revision = ferrix_pdf::Revision::new(PdfSource::new(&base_bytes), &sources)
                .map_err(|e| e.to_string())?;
            // An unchanged page list keeps the original page tree; only touched pages change.
            if !state.pages.is_unchanged() {
                revision.set_layout(layout).map_err(|e| e.to_string())?;
            }
            let now = ferrix_annot::pdf_date(std::time::SystemTime::now());
            ferrix_annot::write_annotations(&mut revision, &state.pages, &state.annots, &now)
                .map_err(|e| e.to_string())?;
            let out = revision.finish().map_err(|e| e.to_string())?;
            ferrix_annot::verify(&out, None, &state.pages, &state.annots)
                .map_err(|e| e.to_string())?;
            Ok(out)
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
        // Back and Forward: ⌘[ ⌘] on macOS, Alt+← Alt+→ elsewhere.
        Key::Named(Named::ArrowLeft) if modifiers.alt() && !cfg!(target_os = "macos") => {
            Some(Message::Back)
        }
        Key::Named(Named::ArrowRight) if modifiers.alt() && !cfg!(target_os = "macos") => {
            Some(Message::Forward)
        }
        Key::Character("[") if modifiers.command() && cfg!(target_os = "macos") => {
            Some(Message::Back)
        }
        Key::Character("]") if modifiers.command() && cfg!(target_os = "macos") => {
            Some(Message::Forward)
        }
        // Sidebar tabs: ⌥⌘2/3/4 on macOS (Preview's keys), Ctrl+Shift+2/3/4 elsewhere, where
        // Ctrl+Alt is AltGr. Shifted digits arrive as symbols on some layouts, so match the
        // physical key's digit too.
        Key::Character(c)
            if modifiers.command()
                && (if cfg!(target_os = "macos") {
                    modifiers.alt()
                } else {
                    modifiers.shift()
                })
                && sidebar_tab_key(c).is_some() =>
        {
            sidebar_tab_key(c).map(Message::SidebarTab)
        }
        Key::Named(Named::Space) => Some(Message::ScreenScroll(!modifiers.shift())),
        Key::Named(Named::ArrowUp) if modifiers.alt() => Some(Message::Move { earlier: true }),
        Key::Named(Named::ArrowDown) if modifiers.alt() => Some(Message::Move { earlier: false }),
        Key::Named(Named::ArrowRight) => Some(Message::Next),
        Key::Named(Named::ArrowLeft) => Some(Message::Previous),
        // One screen at a time, as in Preview and Acrobat; ← → still turn pages.
        Key::Named(Named::PageDown) => Some(Message::ScreenScroll(true)),
        Key::Named(Named::PageUp) => Some(Message::ScreenScroll(false)),
        Key::Named(Named::Home) => Some(Message::First),
        Key::Named(Named::End) => Some(Message::Last),
        Key::Named(Named::Delete | Named::Backspace) => Some(Message::Delete),
        Key::Named(Named::Escape) => Some(Message::Escape),
        // Tool letters; ignored unless Markup mode is on.
        Key::Character(c) if !modifiers.command() && !modifiers.alt() && !modifiers.control() => {
            crate::markup::Tool::from_key(&c.to_ascii_lowercase())
                .map(|t| Message::Annot(crate::annotate_ui::AnnotMsg::Tool(t)))
        }
        Key::Character(c) if modifiers.command() => {
            let shift = modifiers.shift();
            match c.to_ascii_lowercase().as_str() {
                "a" if shift => Some(Message::Annot(crate::annotate_ui::AnnotMsg::Toggle)),
                "a" => Some(Message::SelectAllText),
                "c" => Some(Message::CopyText),
                "o" => Some(Message::OpenDialog),
                "n" => Some(Message::NewDialog),
                "p" => Some(Message::Print),
                "f" => Some(Message::FindOpen),
                "g" if shift => Some(Message::FindPrevious),
                "g" => Some(Message::FindNext),
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

/// The zoom factor for one wheel event: ×1.1 per notch, trackpads proportionally.
fn wheel_factor(delta: iced::mouse::ScrollDelta) -> f32 {
    let notches = match delta {
        iced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
    };
    1.1_f32.powf(notches.clamp(-5.0, 5.0))
}

/// The sidebar tab a digit key selects.
fn sidebar_tab_key(c: &str) -> Option<crate::annotate_ui::SidebarTab> {
    use crate::annotate_ui::SidebarTab;
    match c {
        "2" | "@" | "\"" | "é" => Some(SidebarTab::Pages),
        "3" | "#" | "§" | "\u{a3}" => Some(SidebarTab::Outline),
        "4" | "$" | "'" | "\u{a4}" => Some(SidebarTab::Comments),
        // ⌥ on macOS turns digits into symbols (US layout: ™ £ ¢).
        "™" => Some(SidebarTab::Pages),
        "¢" => Some(SidebarTab::Comments),
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
        assert!((Zoom::FitWidth.resolve(LETTER, canvas, Layout::Continuous) - 1.0).abs() < 1e-4);
        let fit_page = Zoom::FitPage.resolve(LETTER, canvas, Layout::Continuous);
        assert!((fit_page * 792.0 * LOGICAL_PX_PER_POINT - 600.0).abs() < 0.01);
    }

    #[test]
    fn auto_zoom_never_enlarges_past_the_cap() {
        let huge = iced::Size::new(4000.0, 4000.0);
        assert_eq!(
            Zoom::Auto.resolve(LETTER, huge, Layout::Continuous),
            MAX_AUTO_ZOOM
        );
        assert_eq!(
            Zoom::Fixed(9.0).resolve(LETTER, huge, Layout::Continuous),
            MAX_ZOOM
        );
    }

    #[test]
    fn page_keys_scroll_a_screen_and_arrows_turn_pages() {
        let none = keyboard::Modifiers::default();
        let key = |n| keyboard::Key::Named(n);
        assert!(matches!(
            shortcut(&key(Named::PageDown), none),
            Some(Message::ScreenScroll(true))
        ));
        assert!(matches!(
            shortcut(&key(Named::PageUp), none),
            Some(Message::ScreenScroll(false))
        ));
        assert!(matches!(
            shortcut(&key(Named::ArrowRight), none),
            Some(Message::Next)
        ));
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
                state: doc.state.clone(),
            }
            .run(),
        )
    }

    #[test]
    fn export_writes_annotations_with_page_edits_in_one_revision() {
        let Some(worker) = worker() else { return };
        let mut doc = open_fixture(&worker, "annotated.pdf", 3);
        let keys: Vec<PageKey> = (0..3).filter_map(|i| doc.key_at(i)).collect();
        let id = doc.state.annots.new_id();
        let square = ferrix_core::Annotation::new(
            ferrix_core::AnnotKind::Square,
            ferrix_types::PdfBox::from_corners(
                ferrix_types::PdfPoint::new(50.0, 50.0),
                ferrix_types::PdfPoint::new(150.0, 120.0),
            ),
        );
        doc.edit(
            ferrix_core::AnnotEdit::Add {
                page: keys[2],
                index: 0,
                id,
                annot: Box::new(square),
            },
            "Add Rectangle".into(),
        )
        .expect("adds");
        doc.edit(
            Edit::Remove {
                keys: vec![keys[0]],
            },
            "Delete".into(),
        )
        .expect("delete");

        let bytes = export(&worker, &doc).expect("exports");
        let written = ferrix_pdf::load(ferrix_pdf::Source::new(&bytes)).expect("parses");
        let annots = ferrix_annot::read_annotations(&written, ferrix_core::SourceId::BASE);
        let kinds: Vec<Vec<&str>> = annots
            .iter()
            .map(|page| page.iter().map(|a| a.kind.subtype()).collect())
            .collect();
        // Page 3 is now page 2, and carries the rectangle.
        assert_eq!(kinds, vec![vec![], vec!["Square"]]);
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
        let to_front = doc.state.pages.move_edit(&[keys[3]], 0).expect("move");
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
