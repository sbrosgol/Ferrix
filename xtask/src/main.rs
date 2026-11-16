//! Build automation for Ferrix. Run with `cargo xtask <task>`.

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

/// Pinned PDFium build. Must match the `pdfium_*` feature of `pdfium-render` in Cargo.toml.
const PDFIUM_RELEASE: &str = "chromium/8125";

/// SHA-256 of each release archive, from the GitHub release asset digests.
const PDFIUM_ARCHIVES: &[(&str, &str)] = &[
    (
        "mac-arm64",
        "4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d",
    ),
    (
        "mac-x64",
        "8a5193535b80c7215d8932f6647ae5097465b6dba2811602c21e120c2af112b0",
    ),
    (
        "linux-x64",
        "e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659",
    ),
    (
        "linux-arm64",
        "8b8818f2ec57d1b213d952ca6d7f0ffeca9d0110487d0e20ad83ba918bc92efa",
    ),
    (
        "win-x64",
        "9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b",
    ),
    (
        "win-arm64",
        "d394ba61991fb53f4d87f70988035f337d9e73f2bd2c3c1b966df8c43fcabb05",
    ),
];

const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// Download the pinned PDFium build into `vendor/pdfium/`.
    FetchPdfium {
        /// Platform such as `mac-arm64`, `linux-x64`, `win-x64`. Defaults to the host.
        #[arg(long)]
        platform: Option<String>,
        /// Re-download even if the pinned version is already present.
        #[arg(long)]
        force: bool,
    },
    /// Build the desktop app as `Ferrix.app` (macOS), so it carries its name and PDFium.
    Bundle {
        /// Build with the release profile.
        #[arg(long)]
        release: bool,
        /// Launch the bundle once built.
        #[arg(long)]
        open: bool,
        /// Documents to open with `--open`.
        files: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::FetchPdfium { platform, force } => {
            let platform = match platform {
                Some(p) => p,
                None => host_platform()?,
            };
            fetch_pdfium(&platform, force)
        }
        Task::Bundle {
            release,
            open,
            files,
        } => bundle(release, open, &files),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn host_platform() -> Result<String> {
    let os = match std::env::consts::OS {
        "macos" => "mac",
        "linux" => "linux",
        "windows" => "win",
        other => bail!("no PDFium build for OS {other:?}; pass --platform"),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => bail!("no PDFium build for architecture {other:?}; pass --platform"),
    };
    Ok(format!("{os}-{arch}"))
}

fn fetch_pdfium(platform: &str, force: bool) -> Result<()> {
    let Some(&(_, expected_sha)) = PDFIUM_ARCHIVES.iter().find(|(p, _)| *p == platform) else {
        let known: Vec<_> = PDFIUM_ARCHIVES.iter().map(|(p, _)| *p).collect();
        bail!("unknown platform {platform:?}; expected one of {known:?}");
    };

    let dest = workspace_root().join("vendor/pdfium");
    let stamp_path = dest.join(".ferrix-stamp");
    let stamp = format!("{PDFIUM_RELEASE} {platform} {expected_sha}\n");
    if !force && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        println!(
            "PDFium {PDFIUM_RELEASE} ({platform}) already present in {}",
            dest.display()
        );
        return Ok(());
    }

    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_RELEASE}/pdfium-{platform}.tgz"
    );
    println!("Downloading {url}");
    // Trust the OS certificate store so corporate TLS proxies work.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let mut archive = Vec::new();
    agent
        .get(&url)
        .call()
        .with_context(|| format!("download failed: {url}"))?
        .into_body()
        .into_reader()
        .take(MAX_ARCHIVE_BYTES)
        .read_to_end(&mut archive)
        .context("download interrupted")?;

    let actual_sha = hex(&Sha256::digest(&archive));
    if actual_sha != expected_sha {
        bail!("checksum mismatch for {url}\n  expected {expected_sha}\n  actual   {actual_sha}");
    }

    if dest.exists() {
        fs::remove_dir_all(&dest).with_context(|| format!("cannot clear {}", dest.display()))?;
    }
    fs::create_dir_all(&dest)?;
    tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()))
        .unpack(&dest)
        .context("cannot unpack PDFium archive")?;
    fs::write(&stamp_path, stamp)?;

    println!(
        "PDFium {PDFIUM_RELEASE} ({platform}) installed in {}",
        dest.display()
    );
    Ok(())
}

/// Assembles `target/<profile>/Ferrix.app`. Run unbundled, macOS names the app after its
/// executable (`ferrix-app`) in the menu bar and Dock; a bundle names it from its Info.plist.
fn bundle(release: bool, open: bool, files: &[PathBuf]) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("bundling is only implemented for macOS so far");
    }
    let root = workspace_root();
    let pdfium = root.join("vendor/pdfium/lib/libpdfium.dylib");
    if !pdfium.exists() {
        bail!("PDFium is missing; run `cargo xtask fetch-pdfium` first");
    }

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut build = Command::new(cargo);
    build.current_dir(&root).args(["build", "-p", "ferrix-app"]);
    if release {
        build.arg("--release");
    }
    if !build.status().context("cannot run cargo")?.success() {
        bail!("cargo build failed");
    }

    let target = env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let out = target.join(if release { "release" } else { "debug" });
    let app = out.join("Ferrix.app");
    if app.exists() {
        fs::remove_dir_all(&app).with_context(|| format!("cannot clear {}", app.display()))?;
    }
    let contents = app.join("Contents");
    for dir in ["MacOS", "Frameworks", "Resources"] {
        fs::create_dir_all(contents.join(dir))?;
    }
    fs::copy(out.join("ferrix-app"), contents.join("MacOS/Ferrix"))
        .context("cannot copy the ferrix-app binary")?;
    // The engine looks for PDFium in the bundle's Frameworks directory first.
    fs::copy(&pdfium, contents.join("Frameworks/libpdfium.dylib")).context("cannot copy PDFium")?;
    fs::write(contents.join("Info.plist"), info_plist())?;
    make_icns(
        &root.join("crates/ferrix-app/assets/app-icon.svg"),
        &out.join("Ferrix.iconset"),
        &contents.join("Resources/Ferrix.icns"),
    )?;

    // Ad-hoc signature, so the copied binary and library run as one app on Apple silicon.
    let signed = Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&app)
        .status();
    if !signed.is_ok_and(|s| s.success()) {
        println!("warning: ad-hoc signing failed; the bundle may not launch");
    }
    println!("Built {}", app.display());

    if open {
        let mut launch = Command::new("open");
        launch.arg("-n").arg(&app);
        if !files.is_empty() {
            launch.arg("--args").args(files);
        }
        if !launch.status().context("cannot run open")?.success() {
            bail!("couldn't launch {}", app.display());
        }
    }
    Ok(())
}

/// Renders the SVG app icon at every size macOS asks for and packs them with `iconutil`.
fn make_icns(svg: &Path, iconset: &Path, icns: &Path) -> Result<()> {
    use resvg::{tiny_skia, usvg};

    let data = fs::read(svg).with_context(|| format!("cannot read {}", svg.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .with_context(|| format!("cannot parse {}", svg.display()))?;
    if iconset.exists() {
        fs::remove_dir_all(iconset)?;
    }
    fs::create_dir_all(iconset)?;
    for points in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let pixels = points * scale;
            let mut pixmap =
                tiny_skia::Pixmap::new(pixels, pixels).context("icon size out of range")?;
            let zoom = pixels as f32 / tree.size().width();
            resvg::render(
                &tree,
                tiny_skia::Transform::from_scale(zoom, zoom),
                &mut pixmap.as_mut(),
            );
            pixmap
                .save_png(iconset.join(format!("icon_{points}x{points}{suffix}.png")))
                .context("cannot write icon PNG")?;
        }
    }
    let packed = Command::new("iconutil")
        .args(["--convert", "icns", "--output"])
        .arg(icns)
        .arg(iconset)
        .status()
        .context("cannot run iconutil")?;
    if !packed.success() {
        bail!("iconutil failed");
    }
    fs::remove_dir_all(iconset)?;
    Ok(())
}

fn info_plist() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Ferrix</string>
    <key>CFBundleDisplayName</key>
    <string>Ferrix</string>
    <key>CFBundleIconFile</key>
    <string>Ferrix</string>
    <key>CFBundleExecutable</key>
    <string>Ferrix</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.sbrosgol.ferrix</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
