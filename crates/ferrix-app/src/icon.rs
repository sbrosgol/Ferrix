//! Lucide icons (ISC, see `assets/icons/LICENSE`), vendored at a pinned version.

use std::sync::LazyLock;

use iced::widget::svg;

/// Lucide's default 2px stroke is heavy at toolbar sizes; this matches system UI weight.
const STROKE_WIDTH: &str = "1.75";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    PanelLeft,
    FolderOpen,
    ChevronLeft,
    ChevronRight,
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    FileText,
    Alert,
    Save,
    Undo,
    Redo,
    RotateLeft,
    RotateRight,
    MoveUp,
    MoveDown,
    Delete,
    InsertPages,
    New,
    Bold,
    Italic,
    Underline,
    Strikethrough,
    AlignLeft,
    AlignCenter,
    AlignRight,
    AlignJustify,
    PageSetup,
    ClearFormatting,
    BulletList,
    NumberedList,
    Picture,
    PageBreak,
    Search,
    Close,
    MatchCase,
    More,
    Check,
    Markup,
    Pointer,
    StickyNote,
    Pencil,
    Square,
    Circle,
    Minus,
    ArrowUpRight,
    Lock,
    Comments,
}

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        impl Icon {
            pub(crate) fn handle(self) -> svg::Handle {
                $(
                    #[allow(non_upper_case_globals)]
                    static $variant: LazyLock<svg::Handle> = LazyLock::new(|| {
                        load(include_str!(concat!("../assets/icons/", $file, ".svg")))
                    });
                )*
                match self {
                    $(Self::$variant => $variant.clone(),)*
                }
            }
        }
    };
}

icons! {
    PanelLeft => "panel-left",
    FolderOpen => "folder-open",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
    FitWidth => "move-horizontal",
    FitPage => "maximize",
    FileText => "file-text",
    Alert => "circle-alert",
    Save => "save",
    Undo => "undo-2",
    Redo => "redo-2",
    RotateLeft => "rotate-ccw",
    RotateRight => "rotate-cw",
    MoveUp => "arrow-up",
    MoveDown => "arrow-down",
    Delete => "trash-2",
    InsertPages => "file-plus",
    New => "file-new",
    Bold => "bold",
    Italic => "italic",
    Underline => "underline",
    Strikethrough => "strikethrough",
    AlignLeft => "text-align-start",
    AlignCenter => "text-align-center",
    AlignRight => "text-align-end",
    AlignJustify => "text-align-justify",
    PageSetup => "file-cog",
    ClearFormatting => "remove-formatting",
    BulletList => "list",
    NumberedList => "list-ordered",
    Picture => "image",
    PageBreak => "separator-horizontal",
    Search => "search",
    Close => "x",
    MatchCase => "case-sensitive",
    More => "ellipsis",
    Check => "check",
    Markup => "pen-line",
    Pointer => "mouse-pointer-2",
    StickyNote => "sticky-note",
    Pencil => "pencil",
    Square => "square",
    Circle => "circle",
    Minus => "minus",
    ArrowUpRight => "move-up-right",
    Lock => "lock",
    Comments => "message-square-text",
}

fn load(source: &str) -> svg::Handle {
    let source = source.replace(
        "stroke-width=\"2\"",
        &format!("stroke-width=\"{STROKE_WIDTH}\""),
    );
    svg::Handle::from_memory(source.into_bytes())
}
