//! `ferrix` — headless access to the Ferrix engine for scripting, debugging and tests.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use ferrix_engine::{DocInfo, DocSource, OutlineItem, PdfEngine, RenderFlags, Secret, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PixelRect, Rotation};
use image::RgbaImage;

/// Tiles are rendered at most this large and stitched, exercising the same path as the viewer.
const TILE: u32 = 2048;

#[derive(Debug, Parser)]
#[command(
    name = "ferrix",
    version,
    about = "Inspect, render and extract PDF documents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show document information.
    Info {
        #[command(flatten)]
        doc: DocArgs,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Render pages to PNG.
    Render {
        #[command(flatten)]
        doc: DocArgs,
        /// Pages to render, e.g. `1`, `2-5`, `1,3,7-9`. Defaults to the first page.
        #[arg(short, long, default_value = "1")]
        pages: String,
        /// Resolution in dots per inch.
        #[arg(long, default_value_t = 144.0)]
        dpi: f32,
        /// Extra clockwise rotation in degrees (multiple of 90).
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        rotate: i32,
        /// Output file (one page) or directory (several pages).
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Print the text of pages.
    Text {
        #[command(flatten)]
        doc: DocArgs,
        /// Pages to extract; defaults to all.
        #[arg(short, long)]
        pages: Option<String>,
    },
    /// Print the document outline (bookmarks).
    Outline {
        #[command(flatten)]
        doc: DocArgs,
    },
    /// Write a copy with pages reordered, removed or rotated, e.g. `--pages 3,1-2`.
    Organize {
        #[command(flatten)]
        doc: DocArgs,
        /// The pages to keep, in output order.
        #[arg(short, long)]
        pages: String,
        /// Extra clockwise rotation for every kept page (multiple of 90).
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        rotate: i32,
        /// Output file; the input is never modified.
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct DocArgs {
    /// PDF file to open.
    file: PathBuf,
    /// Password for encrypted documents.
    #[arg(long, env = "FERRIX_PDF_PASSWORD", hide_env_values = true)]
    password: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("FERRIX_LOG"))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let mut engine = PdfiumEngine::new()?;
    match cli.command {
        Command::Info { doc, json } => info(&mut engine, &doc, json),
        Command::Render {
            doc,
            pages,
            dpi,
            rotate,
            output,
        } => {
            let rotation = Rotation::from_degrees(rotate)
                .with_context(|| format!("--rotate must be a multiple of 90, got {rotate}"))?;
            render(&mut engine, &doc, &pages, dpi, rotation, &output)
        }
        Command::Text { doc, pages } => text(&mut engine, &doc, pages.as_deref()),
        Command::Outline { doc } => outline(&mut engine, &doc),
        Command::Organize {
            doc,
            pages,
            rotate,
            output,
        } => {
            let rotation = Rotation::from_degrees(rotate)
                .with_context(|| format!("--rotate must be a multiple of 90, got {rotate}"))?;
            organize(&mut engine, &doc, &pages, rotation, &output)
        }
    }
}

fn open(engine: &mut impl PdfEngine, args: &DocArgs) -> Result<DocInfo> {
    let password = args.password.as_deref().map(Secret::new);
    engine
        .open(DocSource::Path(args.file.clone()), password.as_ref())
        .with_context(|| format!("cannot open {}", args.file.display()))
}

fn info(engine: &mut impl PdfEngine, args: &DocArgs, json: bool) -> Result<()> {
    let doc = open(engine, args)?;
    let first = (doc.page_count > 0)
        .then(|| engine.page_info(doc.id, PageIndex(0)))
        .transpose()?;

    if json {
        let value = serde_json::json!({ "document": doc, "first_page": first });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let m = &doc.metadata;
    let rows = [
        ("File", Some(args.file.display().to_string())),
        ("PDF version", doc.pdf_version.clone()),
        ("Pages", Some(doc.page_count.to_string())),
        ("Title", m.title.clone()),
        ("Author", m.author.clone()),
        ("Subject", m.subject.clone()),
        ("Keywords", m.keywords.clone()),
        ("Creator", m.creator.clone()),
        ("Producer", m.producer.clone()),
        (
            "Page 1 size",
            first.map(|p| {
                format!(
                    "{:.1} × {:.1} pt, rotated {}°",
                    p.size.width,
                    p.size.height,
                    p.rotation.degrees()
                )
            }),
        ),
    ];
    for (label, value) in rows {
        if let Some(value) = value {
            println!("{label:<12} {value}");
        }
    }
    Ok(())
}

fn render(
    engine: &mut impl PdfEngine,
    args: &DocArgs,
    pages: &str,
    dpi: f32,
    rotation: Rotation,
    output: &Path,
) -> Result<()> {
    if !(dpi.is_finite() && dpi > 0.0) {
        bail!("--dpi must be positive");
    }
    let doc = open(engine, args)?;
    let pages = parse_pages(pages, doc.page_count)?;
    let scale = dpi / 72.0;

    let single = pages.len() == 1 && !output.is_dir();
    if !single {
        std::fs::create_dir_all(output)
            .with_context(|| format!("cannot create {}", output.display()))?;
    }
    for page in pages {
        let image = render_page(engine, doc.id, page, scale, rotation)?;
        let path = if single {
            output.to_path_buf()
        } else {
            output.join(format!("page-{:04}.png", page.number()))
        };
        image
            .save(&path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        eprintln!(
            "page {page} → {} ({}×{})",
            path.display(),
            image.width(),
            image.height()
        );
    }
    Ok(())
}

fn render_page(
    engine: &mut impl PdfEngine,
    doc: DocId,
    page: PageIndex,
    scale: f32,
    rotation: Rotation,
) -> Result<RgbaImage> {
    let size = engine.page_info(doc, page)?.size;
    let (mut width, mut height) = size.to_pixels(scale);
    if rotation.is_sideways() {
        std::mem::swap(&mut width, &mut height);
    }
    let mut image = RgbaImage::new(width, height);
    for y in (0..height).step_by(TILE as usize) {
        for x in (0..width).step_by(TILE as usize) {
            let tile = engine.render_tile(&TileRequest {
                doc,
                page,
                scale,
                rotation,
                region: PixelRect::new(x, y, TILE, TILE),
                flags: RenderFlags::default(),
            })?;
            let r = tile.region;
            let tile_image = RgbaImage::from_raw(r.width, r.height, tile.pixels)
                .context("engine returned a malformed tile")?;
            image::imageops::replace(&mut image, &tile_image, i64::from(r.x), i64::from(r.y));
        }
    }
    Ok(image)
}

fn text(engine: &mut impl PdfEngine, args: &DocArgs, pages: Option<&str>) -> Result<()> {
    let doc = open(engine, args)?;
    let pages = match pages {
        Some(spec) => parse_pages(spec, doc.page_count)?,
        None => (0..doc.page_count).map(PageIndex).collect(),
    };
    for (i, page) in pages.into_iter().enumerate() {
        if i > 0 {
            print!("\u{c}"); // form feed between pages, like pdftotext
        }
        println!("{}", engine.text_page(doc.id, page)?.text());
    }
    Ok(())
}

fn outline(engine: &mut impl PdfEngine, args: &DocArgs) -> Result<()> {
    fn print_items(items: &[OutlineItem], depth: usize) {
        for item in items {
            let page = item.page.map(|p| format!("  … p.{p}")).unwrap_or_default();
            println!("{:indent$}{}{page}", "", item.title, indent = depth * 2);
            print_items(&item.children, depth + 1);
        }
    }
    let doc = open(engine, args)?;
    print_items(&engine.outline(doc.id)?, 0);
    Ok(())
}

fn organize(
    engine: &mut impl PdfEngine,
    args: &DocArgs,
    pages: &str,
    rotation: Rotation,
    output: &Path,
) -> Result<()> {
    let doc = open(engine, args)?;
    let layout: Vec<_> = parse_pages(pages, doc.page_count)?
        .into_iter()
        .map(|page| ferrix_pdf::LayoutPage {
            source: 0,
            page: page.0,
            rotate_by: rotation,
        })
        .collect();
    let bytes = std::fs::read(&args.file)
        .with_context(|| format!("cannot read {}", args.file.display()))?;
    let source = ferrix_pdf::Source {
        bytes: &bytes,
        password: args.password.as_deref(),
    };
    let out = ferrix_pdf::write_page_layout(source, &[], &layout)?;

    // Check the result with the rendering engine before writing it anywhere.
    let check = engine.open(DocSource::Bytes(out.clone()), None)?;
    if check.page_count as usize != layout.len() {
        bail!(
            "verification failed: wrote {} pages, expected {}",
            check.page_count,
            layout.len()
        );
    }
    ferrix_io::write_atomic(output, &out)
        .with_context(|| format!("cannot write {}", output.display()))?;
    eprintln!(
        "{} pages → {} ({} bytes)",
        layout.len(),
        output.display(),
        out.len()
    );
    Ok(())
}

/// Parses a one-based page list such as `1,3,5-7` into indices, validating against `count`.
fn parse_pages(spec: &str, count: u32) -> Result<Vec<PageIndex>> {
    let number = |s: &str| -> Result<u32> {
        let n: u32 = s
            .trim()
            .parse()
            .with_context(|| format!("invalid page number {s:?}"))?;
        if n == 0 || n > count {
            bail!("page {n} is out of range (document has {count} pages)");
        }
        Ok(n)
    };
    let mut pages = Vec::new();
    for part in spec.split(',').filter(|p| !p.trim().is_empty()) {
        let (start, end) = match part.split_once('-') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => (number(part)?, number(part)?),
        };
        if start > end {
            bail!("invalid page range {part:?}");
        }
        pages.extend((start..=end).map(|n| PageIndex(n - 1)));
    }
    if pages.is_empty() {
        bail!("no pages selected");
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(spec: &str, count: u32) -> Vec<u32> {
        parse_pages(spec, count)
            .expect("valid spec")
            .into_iter()
            .map(PageIndex::number)
            .collect()
    }

    #[test]
    fn parses_page_lists_and_ranges() {
        assert_eq!(numbers("1", 3), [1]);
        assert_eq!(numbers("1,3-5, 7", 10), [1, 3, 4, 5, 7]);
    }

    #[test]
    fn rejects_bad_page_specs() {
        for spec in ["0", "4", "3-1", "a", "", "1-x"] {
            assert!(parse_pages(spec, 3).is_err(), "{spec:?} should fail");
        }
    }
}
