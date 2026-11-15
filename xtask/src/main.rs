//! Build automation for Ferrix. Run with `cargo xtask <task>`.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
