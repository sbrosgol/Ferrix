//! Writing documents from scratch (ADR-0008, phase 2): the open document, its page images,
//! and how keys and toolbar choices become editor calls.

use std::{fmt, path::PathBuf};

use ferrix_compose::{
    Align, Clicks, Color, Editor, ImageData, ListKind, Motion, PageLayout, PageSetup,
    ParagraphStyle, Rasterizer, Toggle,
};
use ferrix_io::FileStamp;
use iced::keyboard;

use crate::render::LOGICAL_PX_PER_POINT;

/// Space around and between pages, in logical pixels.
pub(crate) const PAGE_GAP: f32 = 28.0;

/// A document being written.
#[derive(Debug)]
pub(crate) struct Writer {
    pub(crate) editor: Editor,
    /// Where it is saved; `None` until the first save.
    pub(crate) path: Option<PathBuf>,
    pub(crate) name: String,
    /// The file as it was when opened or last saved, to notice changes made by other apps.
    pub(crate) stamp: Option<FileStamp>,
    rasterizer: Rasterizer,
    pages: Vec<Option<PageImage>>,
    /// Vertical scroll position and height of the visible area, in logical pixels.
    pub(crate) viewport: (f32, f32),
    /// Logical pixels per point, as last drawn.
    pub(crate) zoom: f32,
}

/// A rendered page and the layout it was rendered from.
#[derive(Debug)]
struct PageImage {
    layout: PageLayout,
    scale: f32,
    /// Cut into tiles, like rendered PDF pages.
    image: crate::render::PageImage,
}

impl Writer {
    pub(crate) fn new(editor: Editor, path: Option<PathBuf>, stamp: Option<FileStamp>) -> Self {
        let name = path.as_deref().and_then(|p| p.file_name()).map_or_else(
            || "Untitled.pdf".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self {
            editor,
            path,
            name,
            stamp,
            rasterizer: Rasterizer::new(),
            pages: Vec::new(),
            viewport: (0.0, 2000.0),
            zoom: LOGICAL_PX_PER_POINT,
        }
    }

    pub(crate) fn page_setup(&self) -> PageSetup {
        self.editor.document().page
    }

    /// Size of one page in logical pixels.
    pub(crate) fn page_size(&self) -> iced::Size {
        let page = self.page_setup();
        iced::Size::new(page.width * self.zoom, page.height * self.zoom)
    }

    pub(crate) fn page_count(&self) -> usize {
        self.editor.layout().pages.len()
    }

    /// The top of page `index` in the scrolled content, in logical pixels.
    pub(crate) fn page_top(&self, index: usize) -> f32 {
        PAGE_GAP + index as f32 * (self.page_size().height + PAGE_GAP)
    }

    /// The page showing the middle of the visible area.
    pub(crate) fn current_page(&self) -> usize {
        let middle = self.viewport.0 + self.viewport.1 / 2.0;
        let step = self.page_size().height + PAGE_GAP;
        ((middle - PAGE_GAP / 2.0) / step)
            .floor()
            .clamp(0.0, (self.page_count() - 1) as f32) as usize
    }

    /// The images to draw, one entry per page; `None` for pages not rendered (off screen).
    pub(crate) fn images(&self) -> Vec<Option<&crate::render::PageImage>> {
        (0..self.page_count())
            .map(|i| self.pages.get(i).and_then(|p| p.as_ref()).map(|p| &p.image))
            .collect()
    }

    /// Re-renders pages near the visible area whose layout or scale changed, and forgets the
    /// images of pages far from it.
    pub(crate) fn refresh(&mut self, scale_factor: f32) {
        let scale = self.zoom * scale_factor;
        let height = self.page_size().height;
        let (top, visible) = self.viewport;
        let count = self.page_count();
        self.pages.resize_with(count, || None);
        for index in 0..count {
            let page_top = self.page_top(index);
            let near = page_top + height >= top - height && page_top <= top + visible + height;
            if !near {
                self.pages[index] = None;
                continue;
            }
            let layout = &self.editor.layout().pages[index];
            let fresh = self.pages[index]
                .as_ref()
                .is_some_and(|p| p.scale == scale && &p.layout == layout);
            if fresh {
                continue;
            }
            let layout = layout.clone();
            let raster = self.editor.rasterize(&mut self.rasterizer, index, scale);
            let logical = self.page_size();
            self.pages[index] = raster.map(|r| PageImage {
                layout,
                scale,
                image: crate::render::tiled_image(&r.pixels, r.width, r.height, logical),
            });
        }
    }

    /// Where the caret is in the scrolled content, in logical pixels: top and bottom.
    pub(crate) fn caret_span(&self) -> Option<(f32, f32)> {
        let (page, rect) = self.editor.caret_rect()?;
        let top = self.page_top(page) + rect.y * self.zoom;
        Some((top, top + rect.height * self.zoom))
    }

    /// A scroll position that brings the caret into view, if it is not.
    pub(crate) fn scroll_for_caret(&self) -> Option<f32> {
        let (top, bottom) = self.caret_span()?;
        let (offset, height) = self.viewport;
        let margin = 24.0;
        if top < offset + margin {
            Some((top - margin).max(0.0))
        } else if bottom > offset + height - margin {
            Some(bottom - height + margin)
        } else {
            None
        }
    }

    /// Performs a key command; returns text for the clipboard (copy and cut).
    pub(crate) fn key(&mut self, command: KeyCommand) -> Option<String> {
        let e = &mut self.editor;
        match command {
            KeyCommand::Insert(text) => e.type_text(&text),
            KeyCommand::Enter => e.enter(),
            KeyCommand::PageBreak => e.page_break(),
            KeyCommand::Tab { back } => e.tab(back),
            KeyCommand::Backspace { word } => e.backspace(word),
            KeyCommand::Delete { word } => e.delete_forward(word),
            KeyCommand::Move { motion, extend } => e.move_caret(motion, extend),
            KeyCommand::SelectAll => e.select_all(),
            KeyCommand::Toggle(toggle) => e.toggle(toggle),
            KeyCommand::Copy => {
                let text = e.selected_text();
                return (!text.is_empty()).then_some(text);
            }
            KeyCommand::Cut => {
                let text = e.cut();
                return (!text.is_empty()).then_some(text);
            }
            KeyCommand::Paste => {}
        }
        None
    }

    pub(crate) fn press(&mut self, page: usize, x: f32, y: f32, clicks: Clicks, extend: bool) {
        self.editor.click(page, x, y, clicks, extend);
    }
}

/// What a key press does in the writing view.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyCommand {
    Insert(String),
    Enter,
    /// Starts a new page (⌘/Ctrl+Enter).
    PageBreak,
    /// Nests a list item (or types a tab); `back` un-nests (Shift+Tab).
    Tab {
        back: bool,
    },
    Backspace {
        word: bool,
    },
    Delete {
        word: bool,
    },
    Move {
        motion: Motion,
        extend: bool,
    },
    SelectAll,
    Copy,
    Cut,
    Paste,
    Toggle(Toggle),
}

/// Maps a key press to a command. Keys it leaves alone (`None`) go on to the app's shortcuts.
pub(crate) fn key_command(
    key: &keyboard::Key,
    physical: keyboard::key::Physical,
    modifiers: keyboard::Modifiers,
    text: Option<&str>,
) -> Option<KeyCommand> {
    use keyboard::{Key, key::Named};
    let mac = cfg!(target_os = "macos");
    let command = modifiers.command();
    // Word-wise movement and deletion: Option on macOS, Ctrl elsewhere.
    let word = if mac {
        modifiers.alt()
    } else {
        modifiers.control()
    };
    let extend = modifiers.shift();
    let line_jump = mac && modifiers.logo();
    let doc_jump = if mac {
        modifiers.logo()
    } else {
        modifiers.control()
    };
    let go = |motion| Some(KeyCommand::Move { motion, extend });

    match key.as_ref() {
        Key::Named(Named::Enter) if command => Some(KeyCommand::PageBreak),
        Key::Named(Named::Enter) => Some(KeyCommand::Enter),
        Key::Named(Named::Backspace) => Some(KeyCommand::Backspace { word }),
        Key::Named(Named::Delete) => Some(KeyCommand::Delete { word }),
        Key::Named(Named::Tab) if !command => Some(KeyCommand::Tab { back: extend }),
        Key::Named(Named::ArrowLeft) if line_jump => go(Motion::LineStart),
        Key::Named(Named::ArrowLeft) if word => go(Motion::WordLeft),
        Key::Named(Named::ArrowLeft) => go(Motion::Left),
        Key::Named(Named::ArrowRight) if line_jump => go(Motion::LineEnd),
        Key::Named(Named::ArrowRight) if word => go(Motion::WordRight),
        Key::Named(Named::ArrowRight) => go(Motion::Right),
        Key::Named(Named::ArrowUp) if line_jump => go(Motion::DocumentStart),
        Key::Named(Named::ArrowUp) => go(Motion::Up),
        Key::Named(Named::ArrowDown) if line_jump => go(Motion::DocumentEnd),
        Key::Named(Named::ArrowDown) => go(Motion::Down),
        Key::Named(Named::Home) if doc_jump => go(Motion::DocumentStart),
        Key::Named(Named::Home) => go(Motion::LineStart),
        Key::Named(Named::End) if doc_jump => go(Motion::DocumentEnd),
        Key::Named(Named::End) => go(Motion::LineEnd),
        Key::Named(Named::PageUp) => go(Motion::PageUp),
        Key::Named(Named::PageDown) => go(Motion::PageDown),
        _ if command => match key.to_latin(physical) {
            Some('a') => Some(KeyCommand::SelectAll),
            Some('c') => Some(KeyCommand::Copy),
            Some('x') => Some(KeyCommand::Cut),
            Some('v') if !modifiers.alt() => Some(KeyCommand::Paste),
            Some('b') => Some(KeyCommand::Toggle(Toggle::Bold)),
            Some('i') => Some(KeyCommand::Toggle(Toggle::Italic)),
            Some('u') => Some(KeyCommand::Toggle(Toggle::Underline)),
            _ => None,
        },
        // Ctrl+letter on macOS produces control characters, which are not text.
        _ => {
            let text: String = text?.chars().filter(|c| !c.is_control()).collect();
            (!text.is_empty()).then_some(KeyCommand::Insert(text))
        }
    }
}

// ── Toolbar choices ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleChoice(pub ParagraphStyle);

impl fmt::Display for StyleChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.name())
    }
}

pub const STYLES: [StyleChoice; 5] = [
    StyleChoice(ParagraphStyle::Normal),
    StyleChoice(ParagraphStyle::Title),
    StyleChoice(ParagraphStyle::Heading1),
    StyleChoice(ParagraphStyle::Heading2),
    StyleChoice(ParagraphStyle::Heading3),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeChoice(pub f32);

impl fmt::Display for SizeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.fract() == 0.0 {
            write!(f, "{:.0}", self.0)
        } else {
            write!(f, "{:.1}", self.0)
        }
    }
}

pub(crate) const SIZES: [SizeChoice; 17] = [
    SizeChoice(8.0),
    SizeChoice(9.0),
    SizeChoice(10.0),
    SizeChoice(10.5),
    SizeChoice(11.0),
    SizeChoice(12.0),
    SizeChoice(14.0),
    SizeChoice(16.0),
    SizeChoice(18.0),
    SizeChoice(20.0),
    SizeChoice(24.0),
    SizeChoice(28.0),
    SizeChoice(32.0),
    SizeChoice(36.0),
    SizeChoice(48.0),
    SizeChoice(60.0),
    SizeChoice(72.0),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorChoice {
    pub name: &'static str,
    pub color: Color,
}

impl fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

pub(crate) const COLORS: [ColorChoice; 8] = [
    ColorChoice {
        name: "Automatic",
        color: Color::BLACK,
    },
    ColorChoice {
        name: "Grey",
        color: Color(0x5F, 0x63, 0x68),
    },
    ColorChoice {
        name: "Red",
        color: Color(0xC5, 0x22, 0x1F),
    },
    ColorChoice {
        name: "Orange",
        color: Color(0xD9, 0x6C, 0x00),
    },
    ColorChoice {
        name: "Green",
        color: Color(0x1E, 0x7D, 0x32),
    },
    ColorChoice {
        name: "Blue",
        color: Color(0x1A, 0x5F, 0xB4),
    },
    ColorChoice {
        name: "Purple",
        color: Color(0x6A, 0x3D, 0x9A),
    },
    ColorChoice {
        name: "Teal",
        color: Color(0x00, 0x79, 0x6B),
    },
];

pub(crate) fn color_choice(color: Color) -> ColorChoice {
    COLORS
        .iter()
        .copied()
        .find(|c| c.color == color)
        .unwrap_or(ColorChoice {
            name: "Custom",
            color,
        })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingChoice(pub f32);

impl fmt::Display for SpacingChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1.0 => f.write_str("Single"),
            2.0 => f.write_str("Double"),
            s => write!(f, "{s}"),
        }
    }
}

pub(crate) const SPACINGS: [SpacingChoice; 4] = [
    SpacingChoice(1.0),
    SpacingChoice(1.15),
    SpacingChoice(1.5),
    SpacingChoice(2.0),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarginChoice {
    Narrow,
    Normal,
    Wide,
}

impl MarginChoice {
    pub(crate) fn points(self) -> f32 {
        match self {
            Self::Narrow => 36.0,
            Self::Normal => 72.0,
            Self::Wide => 108.0,
        }
    }

    pub(crate) fn nearest(points: f32) -> Self {
        [Self::Narrow, Self::Normal, Self::Wide]
            .into_iter()
            .min_by(|a, b| {
                (a.points() - points)
                    .abs()
                    .total_cmp(&(b.points() - points).abs())
            })
            .unwrap_or(Self::Normal)
    }
}

impl fmt::Display for MarginChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Narrow => "Narrow (½ in)",
            Self::Normal => "Normal (1 in)",
            Self::Wide => "Wide (1½ in)",
        })
    }
}

pub(crate) const MARGINS: [MarginChoice; 3] = [
    MarginChoice::Narrow,
    MarginChoice::Normal,
    MarginChoice::Wide,
];

/// Things the writing view asks for.
#[derive(Debug, Clone)]
pub enum Action {
    Press {
        page: usize,
        x: f32,
        y: f32,
        clicks: Clicks,
        extend: bool,
    },
    Drag {
        page: usize,
        x: f32,
        y: f32,
    },
    Key(KeyCommand),
    Pasted(Option<String>),
    Scrolled(f32, f32),
    Style(StyleChoice),
    Family(String),
    Size(SizeChoice),
    Toggle(Toggle),
    Color(ColorChoice),
    Align(Align),
    Spacing(SpacingChoice),
    ClearFormatting,
    List(ListKind),
    PageBreak,
    /// Opens a file dialog for a picture.
    InsertImage,
    ImageLoaded(Result<Box<ImageData>, String>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::{Key, Modifiers, key::Code, key::Named, key::Physical};

    fn press(key: Key, modifiers: Modifiers, text: Option<&str>) -> Option<KeyCommand> {
        key_command(&key, Physical::Code(Code::KeyA), modifiers, text)
    }

    #[test]
    fn typing_and_editing_keys() {
        assert_eq!(
            press(Key::Character("a".into()), Modifiers::empty(), Some("a")),
            Some(KeyCommand::Insert("a".into()))
        );
        assert_eq!(
            press(Key::Character("A".into()), Modifiers::SHIFT, Some("A")),
            Some(KeyCommand::Insert("A".into()))
        );
        assert_eq!(
            press(Key::Named(Named::Enter), Modifiers::empty(), Some("\r")),
            Some(KeyCommand::Enter)
        );
        assert_eq!(
            press(Key::Named(Named::Backspace), Modifiers::empty(), None),
            Some(KeyCommand::Backspace { word: false })
        );
        assert_eq!(
            press(Key::Named(Named::ArrowLeft), Modifiers::SHIFT, None),
            Some(KeyCommand::Move {
                motion: Motion::Left,
                extend: true
            })
        );
        // Control characters are never typed.
        assert_eq!(
            press(Key::Character("a".into()), Modifiers::CTRL, Some("\u{1}")),
            {
                if cfg!(target_os = "macos") {
                    None
                } else {
                    Some(KeyCommand::SelectAll)
                }
            }
        );
    }

    #[test]
    fn command_shortcuts_are_handled_or_passed_on() {
        let command = if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        };
        assert_eq!(
            press(Key::Character("b".into()), command, Some("b")),
            Some(KeyCommand::Toggle(Toggle::Bold))
        );
        assert_eq!(
            press(Key::Character("v".into()), command, Some("v")),
            Some(KeyCommand::Paste)
        );
        assert_eq!(
            press(Key::Named(Named::Enter), command, None),
            Some(KeyCommand::PageBreak)
        );
        assert_eq!(
            press(Key::Named(Named::Tab), Modifiers::SHIFT, None),
            Some(KeyCommand::Tab { back: true })
        );
        // Save, undo and find belong to the app.
        assert_eq!(press(Key::Character("f".into()), command, Some("f")), None);
        // (and so do these)
        assert_eq!(press(Key::Character("s".into()), command, Some("s")), None);
        assert_eq!(press(Key::Character("z".into()), command, Some("z")), None);
        assert_eq!(
            press(Key::Named(Named::Escape), Modifiers::empty(), None),
            None
        );
    }

    #[test]
    fn choices_display_and_match() {
        assert_eq!(SizeChoice(10.5).to_string(), "10.5");
        assert_eq!(SizeChoice(12.0).to_string(), "12");
        assert_eq!(SpacingChoice(1.0).to_string(), "Single");
        assert_eq!(color_choice(Color(0xC5, 0x22, 0x1F)).name, "Red");
        assert_eq!(color_choice(Color(1, 2, 3)).name, "Custom");
        assert_eq!(MarginChoice::nearest(70.0), MarginChoice::Normal);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ferrix_compose::{Document, Fonts};

    #[test]
    fn page_images_contain_the_typed_text() {
        let mut editor = Editor::new(Document::new(), Fonts::bundled());
        editor.type_text("Hello world, this is visible text.");
        let mut writer = Writer::new(editor, None, None);
        writer.refresh(2.0);
        let images = writer.images();
        let page = images[0].expect("page 0 rendered");
        let mut ink = 0;
        for (handle, _) in page.rows.iter().flatten() {
            let iced::widget::image::Handle::Rgba {
                width,
                height,
                pixels,
                ..
            } = handle
            else {
                panic!("not an RGBA handle");
            };
            assert_eq!(pixels.len(), (*width * *height * 4) as usize);
            ink += pixels
                .chunks(4)
                .filter(|p| p[0] < 128 && p[3] == 255)
                .count();
        }
        assert!(ink > 100, "ink pixels: {ink}");
    }
}
