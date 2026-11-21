//! Double-clicking the title bar. On macOS the toolbar replaces the system title bar
//! (`fullsize_content_view`), so Ferrix has to do what the system bar would: follow the
//! "Double-click a window's title bar to" setting in System Settings › Desktop & Dock.

/// What a double-click on the title bar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleBarAction {
    /// Fill the screen without entering full screen (macOS "Zoom"), or restore.
    #[default]
    Zoom,
    Minimize,
    Nothing,
}

/// Interprets the global defaults `AppleActionOnDoubleClick` (macOS 11+: `Maximize`, `Minimize`
/// or `None`) and, when it is absent, the older `AppleMiniaturizeOnDoubleClick` (`1` minimizes).
/// The system default is to zoom.
pub(crate) fn from_defaults(action: Option<&str>, miniaturize: Option<&str>) -> TitleBarAction {
    match action.map(str::trim) {
        Some("Minimize") => TitleBarAction::Minimize,
        Some("None") => TitleBarAction::Nothing,
        Some("Maximize" | "Fill" | "Zoom") => TitleBarAction::Zoom,
        _ => match miniaturize.map(str::trim) {
            Some("1" | "YES" | "true") => TitleBarAction::Minimize,
            _ => TitleBarAction::Zoom,
        },
    }
}

/// The user's setting. Runs `defaults`, so call it off the UI thread.
pub(crate) fn preference() -> TitleBarAction {
    if !cfg!(target_os = "macos") {
        // Other platforms draw their own title bar, which handles double-clicks itself.
        return TitleBarAction::Zoom;
    }
    let read = |key: &str| {
        std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", key])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let action = read("AppleActionOnDoubleClick");
    let miniaturize = if action.is_none() {
        read("AppleMiniaturizeOnDoubleClick")
    } else {
        None
    };
    from_defaults(action.as_deref(), miniaturize.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_macos_setting() {
        assert_eq!(
            from_defaults(Some("Maximize\n"), None),
            TitleBarAction::Zoom
        );
        assert_eq!(
            from_defaults(Some("Minimize\n"), None),
            TitleBarAction::Minimize
        );
        assert_eq!(from_defaults(Some("None\n"), None), TitleBarAction::Nothing);
    }

    #[test]
    fn falls_back_to_the_old_setting_then_to_zoom() {
        assert_eq!(from_defaults(None, Some("1\n")), TitleBarAction::Minimize);
        assert_eq!(from_defaults(None, Some("0\n")), TitleBarAction::Zoom);
        assert_eq!(from_defaults(None, None), TitleBarAction::Zoom);
        assert_eq!(from_defaults(Some("Garbage"), None), TitleBarAction::Zoom);
    }
}
