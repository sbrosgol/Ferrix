//! LibreOffice, run headless with a private profile.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::Duration,
};

use crate::{ConvertError, ScratchDir, process};

/// Environment variable naming the `soffice` executable, overriding detection.
pub const SOFFICE_ENV: &str = "FERRIX_SOFFICE";
/// The first conversion also creates LibreOffice's profile, which is slow.
const TIMEOUT: Duration = Duration::from_secs(180);
/// Conversions share one profile directory, so they run one at a time.
static PROFILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone)]
pub struct LibreOffice {
    program: PathBuf,
}

impl LibreOffice {
    pub fn detect() -> Option<Self> {
        let mut candidates: Vec<PathBuf> = env::var_os(SOFFICE_ENV)
            .map(PathBuf::from)
            .into_iter()
            .collect();
        if cfg!(target_os = "macos") {
            candidates.push("/Applications/LibreOffice.app/Contents/MacOS/soffice".into());
            if let Some(home) = env::var_os("HOME") {
                candidates.push(
                    PathBuf::from(home).join("Applications/LibreOffice.app/Contents/MacOS/soffice"),
                );
            }
        }
        if cfg!(windows) {
            for var in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(dir) = env::var_os(var) {
                    candidates.push(PathBuf::from(dir).join(r"LibreOffice\program\soffice.exe"));
                }
            }
        }
        if cfg!(all(unix, not(target_os = "macos"))) {
            candidates.push("/usr/lib/libreoffice/program/soffice".into());
            candidates.push("/opt/libreoffice/program/soffice".into());
        }
        let names: &[&str] = if cfg!(windows) {
            &["soffice.exe"]
        } else {
            &["soffice", "libreoffice"]
        };
        if let Some(path) = env::var_os("PATH") {
            for dir in env::split_paths(&path) {
                candidates.extend(names.iter().map(|n| dir.join(n)));
            }
        }
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .map(|program| Self { program })
    }

    /// Converts `input` to PDF. Blocking.
    pub fn convert(&self, input: &Path) -> Result<Vec<u8>, ConvertError> {
        let _one_at_a_time = PROFILE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let input = std::path::absolute(input)?;
        let out = ScratchDir::new()?;
        let profile = env::temp_dir().join("ferrix-libreoffice-profile");

        let mut command = Command::new(&self.program);
        command
            .args([
                "--headless",
                "--norestore",
                "--nologo",
                "--nodefault",
                "--nolockcheck",
                "--nofirststartwizard",
            ])
            // A private profile: never touches, or waits for, the user's running LibreOffice.
            .arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args(["--convert-to", "pdf", "--outdir"])
            .arg(out.path())
            .arg(&input);
        let finished = process::run(command, "LibreOffice", TIMEOUT)?;

        let produced = std::fs::read_dir(out.path())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
        match produced {
            Some(pdf) => Ok(std::fs::read(pdf)?),
            None => Err(ConvertError::Failed {
                tool: "LibreOffice",
                message: if finished.success {
                    "no PDF was produced".into()
                } else {
                    process::summary(&finished.output)
                },
            }),
        }
    }
}

/// A `file://` URL for a local path, as LibreOffice expects in `-env:` options.
fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from(if path.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(byte as char);
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_escape_spaces_and_unicode() {
        assert_eq!(
            file_url(Path::new("/tmp/My Files/ü")),
            "file:///tmp/My%20Files/%C3%BC"
        );
    }

    #[test]
    fn converts_a_document_when_libreoffice_is_installed() {
        let Some(office) = LibreOffice::detect() else {
            eprintln!("skipping: LibreOffice not installed");
            return;
        };
        let dir = ScratchDir::new().expect("scratch");
        let input = dir.path().join("note.txt");
        std::fs::write(&input, "Hello from LibreOffice\n").expect("write");
        let pdf = office.convert(&input).expect("converts");
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
