//! Microsoft Word, driven by AppleScript (macOS) or COM through PowerShell (Windows).

use std::path::Path;
#[cfg(any(target_os = "macos", windows))]
use std::time::Duration;

use crate::ConvertError;

/// Word may need the user to grant file access or automation permission on first use.
#[cfg(any(target_os = "macos", windows))]
const TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
pub struct Word {
    _private: (),
}

impl Word {
    #[cfg(target_os = "macos")]
    pub fn detect() -> Option<Self> {
        let mut apps = vec![std::path::PathBuf::from("/Applications/Microsoft Word.app")];
        if let Some(home) = std::env::var_os("HOME") {
            apps.push(std::path::PathBuf::from(home).join("Applications/Microsoft Word.app"));
        }
        apps.iter()
            .any(|a| a.is_dir())
            .then_some(Self { _private: () })
    }

    #[cfg(windows)]
    pub fn detect() -> Option<Self> {
        let mut command = std::process::Command::new("reg");
        command.args(["query", r"HKCR\Word.Application\CurVer"]);
        crate::process::run(command, "reg", Duration::from_secs(5))
            .ok()
            .filter(|f| f.success)
            .map(|_| Self { _private: () })
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn detect() -> Option<Self> {
        None
    }

    /// Converts `input` to PDF. Blocking; Word may become visible while it works.
    pub fn convert(&self, input: &Path) -> Result<Vec<u8>, ConvertError> {
        convert(std::path::absolute(input)?.as_path())
    }
}

/// Word is sandboxed on macOS: it can write where it was allowed to read, so the PDF goes
/// beside the input as a hidden file and is removed once read.
#[cfg(target_os = "macos")]
fn convert(input: &Path) -> Result<Vec<u8>, ConvertError> {
    use std::process::Command;

    const SCRIPT: &[&str] = &[
        "on run argv",
        "set inPath to item 1 of argv",
        "set outPath to item 2 of argv",
        "set wasRunning to application \"Microsoft Word\" is running",
        "tell application \"Microsoft Word\"",
        "open (POSIX file inPath)",
        "save as active document file name outPath file format format PDF",
        "close active document saving no",
        "if not wasRunning then quit saving no",
        "end tell",
        "end run",
    ];
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let output = input.with_file_name(format!(".{stem}.ferrix-{}.pdf", std::process::id()));

    let mut command = Command::new("osascript");
    for line in SCRIPT {
        command.args(["-e", line]);
    }
    command.arg(input).arg(&output);
    let result = crate::process::run(command, "Microsoft Word", TIMEOUT).and_then(|finished| {
        match std::fs::read(&output) {
            Ok(bytes) => Ok(bytes),
            Err(_) if finished.output.contains("-1743") => Err(ConvertError::Failed {
                tool: "Microsoft Word",
                message: "Ferrix isn’t allowed to control Word. Allow it in System Settings › \
                          Privacy & Security › Automation."
                    .into(),
            }),
            Err(_) => Err(ConvertError::Failed {
                tool: "Microsoft Word",
                message: crate::process::summary(&finished.output),
            }),
        }
    });
    let _ = std::fs::remove_file(&output);
    result
}

/// Word's COM automation runs document macros by default, so they are force-disabled first.
#[cfg(windows)]
fn convert(input: &Path) -> Result<Vec<u8>, ConvertError> {
    use std::process::Command;

    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
        $word = New-Object -ComObject Word.Application; \
        try { \
          $word.Visible = $false; $word.DisplayAlerts = 0; $word.AutomationSecurity = 3; \
          $doc = $word.Documents.Open($env:FERRIX_IN, $false, $true, $false); \
          $doc.ExportAsFixedFormat($env:FERRIX_OUT, 17); \
          $doc.Close(0) \
        } finally { $word.Quit() }";
    let scratch = crate::ScratchDir::new()?;
    let output = scratch.path().join("out.pdf");
    let mut command = Command::new("powershell");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            SCRIPT,
        ])
        // Paths travel in the environment, never spliced into the script.
        .env("FERRIX_IN", input)
        .env("FERRIX_OUT", &output);
    let finished = crate::process::run(command, "Microsoft Word", TIMEOUT)?;
    std::fs::read(&output).map_err(|_| ConvertError::Failed {
        tool: "Microsoft Word",
        message: crate::process::summary(&finished.output),
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn convert(_input: &Path) -> Result<Vec<u8>, ConvertError> {
    Err(ConvertError::NoConverter(false))
}
