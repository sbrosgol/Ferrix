//! Lucide icons (ISC, see `assets/icons/LICENSE`), vendored at a pinned version.

use std::sync::LazyLock;

use iced::widget::svg;

/// Lucide's default 2px stroke is heavy at toolbar sizes; this matches system UI weight.
const STROKE_WIDTH: &str = "1.75";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    PanelLeft,
    FolderOpen,
    Printer,
    ChevronLeft,
    ChevronRight,
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    Sun,
    Moon,
    FileText,
    Alert,
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
    Printer => "printer",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
    FitWidth => "move-horizontal",
    FitPage => "maximize",
    Sun => "sun",
    Moon => "moon",
    FileText => "file-text",
    Alert => "circle-alert",
}

fn load(source: &str) -> svg::Handle {
    let source = source.replace(
        "stroke-width=\"2\"",
        &format!("stroke-width=\"{STROKE_WIDTH}\""),
    );
    svg::Handle::from_memory(source.into_bytes())
}
