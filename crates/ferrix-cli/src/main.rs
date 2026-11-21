//! `ferrix` — headless access to the Ferrix engine for scripting, debugging and tests.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use ferrix_core::{
    AnnotEdit, AnnotKind, AnnotSet, Annotation, DocEdit, DocState, LineEnd, NoteIcon, PageKey,
    PageList, Rgb, SourceId,
};
use ferrix_engine::{DocInfo, DocSource, OutlineItem, PdfEngine, RenderFlags, Secret, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{DocId, PageIndex, PdfBox, PdfPoint, PixelRect, Rotation};
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
    /// Write a document (Ferrix JSON source) as a PDF with the source embedded.
    Compose {
        /// Document source (`.json`), or a PDF written by Ferrix.
        input: PathBuf,
        /// Output PDF.
        #[arg(short, long)]
        output: PathBuf,
        /// Also use fonts installed on this computer (layout then depends on the machine).
        #[arg(long)]
        system_fonts: bool,
    },
    /// Print the document source embedded in a PDF written by Ferrix.
    Source {
        /// PDF file.
        file: PathBuf,
    },
    /// List, add or remove annotations. Coordinates are PDF points from the page's
    /// bottom-left corner, as stored in the file (unrotated).
    Annotate {
        #[command(subcommand)]
        action: AnnotateAction,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant, reason = "parsed once at startup")]
enum AnnotateAction {
    /// List the annotations of every page.
    List {
        #[command(flatten)]
        doc: DocArgs,
    },
    /// Write a copy with one annotation added.
    Add {
        #[command(flatten)]
        doc: DocArgs,
        #[command(flatten)]
        annot: AnnotArgs,
        /// Output file; the input is never modified.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Write a copy with one annotation removed.
    Remove {
        #[command(flatten)]
        doc: DocArgs,
        /// Page number (1-based).
        #[arg(long)]
        page: u32,
        /// Annotation number on that page, as `list` shows it (1-based).
        #[arg(long)]
        index: usize,
        /// Output file; the input is never modified.
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum AnnotKindArg {
    Note,
    Square,
    Circle,
    Line,
    Arrow,
    Ink,
}

#[derive(Debug, Args)]
struct AnnotArgs {
    /// Page number (1-based).
    #[arg(long)]
    page: u32,
    #[arg(long, value_enum)]
    kind: AnnotKindArg,
    /// Box for square and circle: `left,bottom,right,top`.
    #[arg(long)]
    rect: Option<String>,
    /// Top-left corner of a note: `x,y`.
    #[arg(long)]
    at: Option<String>,
    /// Start of a line or arrow: `x,y`.
    #[arg(long)]
    from: Option<String>,
    /// End of a line, or the arrow's tip: `x,y`.
    #[arg(long)]
    to: Option<String>,
    /// Ink strokes: points `x,y` separated by spaces, strokes separated by `;`.
    #[arg(long)]
    points: Option<String>,
    /// Stroke colour, `#rrggbb`.
    #[arg(long, default_value = "#d81b1b")]
    color: String,
    /// Fill colour for squares, circles and closed arrow heads, `#rrggbb`.
    #[arg(long)]
    fill: Option<String>,
    /// Line width in points.
    #[arg(long, default_value_t = 2.0)]
    width: f32,
    /// Opacity from 0 to 1.
    #[arg(long, default_value_t = 1.0)]
    opacity: f32,
    /// Comment text.
    #[arg(long)]
    contents: Option<String>,
    /// Author name; omitted unless given.
    #[arg(long)]
    author: Option<String>,
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
    // These work without PDFium.
    match cli.command {
        Command::Compose {
            input,
            output,
            system_fonts,
        } => return compose(&input, &output, system_fonts),
        Command::Source { file } => return source(&file),
        Command::Annotate {
            action: AnnotateAction::List { doc },
        } => return annotate_list(&doc),
        _ => {}
    }
    let mut engine = PdfiumEngine::new()?;
    match cli.command {
        Command::Compose { .. }
        | Command::Source { .. }
        | Command::Annotate {
            action: AnnotateAction::List { .. },
        } => unreachable!("handled above"),
        Command::Annotate {
            action: AnnotateAction::Add { doc, annot, output },
        } => annotate_add(&mut engine, &doc, &annot, &output),
        Command::Annotate {
            action:
                AnnotateAction::Remove {
                    doc,
                    page,
                    index,
                    output,
                },
        } => annotate_remove(&mut engine, &doc, page, index, &output),
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

/// The file's bytes and its annotation editing state, as the app builds it on open.
fn annotation_state(args: &DocArgs) -> Result<(Vec<u8>, DocState)> {
    let bytes = std::fs::read(&args.file)
        .with_context(|| format!("cannot read {}", args.file.display()))?;
    let doc = ferrix_pdf::load(ferrix_pdf::Source {
        bytes: &bytes,
        password: args.password.as_deref(),
    })?;
    let per_page = ferrix_annot::read_annotations(&doc, SourceId::BASE);
    let pages = PageList::identity(per_page.len() as u32);
    let mut annots = AnnotSet::new();
    for (entry, list) in pages.entries().iter().zip(per_page) {
        annots.insert_loaded(entry.key, list);
    }
    Ok((bytes, DocState::new(pages, annots)))
}

fn annotate_list(args: &DocArgs) -> Result<()> {
    let (_, state) = annotation_state(args)?;
    for (n, entry) in state.pages.entries().iter().enumerate() {
        for (i, (_, a)) in state.annots.page(entry.key).iter().enumerate() {
            let r = a.props.rect;
            let mut line = format!(
                "page {} #{}: {} [{:.1} {:.1} {:.1} {:.1}]",
                n + 1,
                i + 1,
                a.kind.subtype(),
                r.left,
                r.bottom,
                r.right,
                r.top
            );
            if !a.is_editable() {
                line.push_str(" (read-only)");
            }
            if let Some(author) = &a.props.author {
                line.push_str(&format!(" by {author:?}"));
            }
            if let Some(contents) = &a.props.contents {
                line.push_str(&format!(": {contents:?}"));
            }
            println!("{line}");
        }
    }
    Ok(())
}

fn annotate_add(
    engine: &mut impl PdfEngine,
    args: &DocArgs,
    annot: &AnnotArgs,
    output: &Path,
) -> Result<()> {
    let (bytes, mut state) = annotation_state(args)?;
    let page = page_key(&state, annot.page)?;
    let new = build_annotation(annot)?;
    let id = state.annots.new_id();
    let index = state.annots.page(page).len();
    state.apply(&DocEdit::Annot(AnnotEdit::Add {
        page,
        index,
        id,
        annot: Box::new(new),
    }))?;
    save_annotations(engine, args, &bytes, &state, output)
}

fn annotate_remove(
    engine: &mut impl PdfEngine,
    args: &DocArgs,
    page: u32,
    index: usize,
    output: &Path,
) -> Result<()> {
    let (bytes, mut state) = annotation_state(args)?;
    let key = page_key(&state, page)?;
    let Some((id, annot)) = index
        .checked_sub(1)
        .and_then(|i| state.annots.page(key).get(i))
    else {
        bail!("page {page} has no annotation #{index} (see `ferrix annotate list`)");
    };
    if !annot.is_deletable() {
        bail!("annotation #{index} on page {page} is locked");
    }
    // A note's replies go with it, as in other readers (they must stay on its page).
    let thread = state.annots.thread(*id);
    if thread.len() > 1 {
        eprintln!("also removing {} replies", thread.len() - 1);
    }
    let edits = thread
        .into_iter()
        .map(|id| DocEdit::Annot(AnnotEdit::Remove { id }))
        .collect();
    state.apply(&DocEdit::Batch(edits))?;
    save_annotations(engine, args, &bytes, &state, output)
}

fn page_key(state: &DocState, page: u32) -> Result<PageKey> {
    let count = state.pages.len();
    page.checked_sub(1)
        .and_then(|i| state.pages.get(i as usize))
        .map(|e| e.key)
        .with_context(|| format!("page {page} is out of range (document has {count} pages)"))
}

/// Writes the annotation changes as an incremental update, checks the result with PDFium and
/// only then writes `output`.
fn save_annotations(
    engine: &mut impl PdfEngine,
    args: &DocArgs,
    bytes: &[u8],
    state: &DocState,
    output: &Path,
) -> Result<()> {
    let source = ferrix_pdf::Source {
        bytes,
        password: args.password.as_deref(),
    };
    let mut revision = ferrix_pdf::Revision::new(source, &[])?;
    let now = ferrix_annot::pdf_date(std::time::SystemTime::now());
    let report = ferrix_annot::write_annotations(&mut revision, &state.pages, &state.annots, &now)?;
    let out = revision.finish()?;
    ferrix_annot::verify(&out, args.password.as_deref(), &state.pages, &state.annots)?;

    // PDFium must see exactly the annotations we meant to write, page by page.
    let password = args.password.as_deref().map(Secret::new);
    let check = engine.open(DocSource::Bytes(out.clone()), password.as_ref())?;
    for (n, entry) in state.pages.entries().iter().enumerate() {
        let seen: Vec<String> = engine
            .annotations(check.id, PageIndex(n as u32))?
            .into_iter()
            .map(|a| a.subtype)
            .filter(|s| !matches!(s.as_str(), "Popup" | "Link" | "Widget"))
            .collect();
        let expected: Vec<&str> = state
            .annots
            .page(entry.key)
            .iter()
            .map(|(_, a)| a.kind.subtype())
            .collect();
        if seen != expected {
            bail!(
                "verification failed on page {}: PDFium sees {seen:?}, expected {expected:?}",
                n + 1
            );
        }
    }
    engine.close(check.id);
    ferrix_io::write_atomic(output, &out)
        .with_context(|| format!("cannot write {}", output.display()))?;
    eprintln!(
        "{} added, {} changed, {} removed → {} ({} bytes)",
        report.added,
        report.changed,
        report.removed,
        output.display(),
        out.len()
    );
    Ok(())
}

fn build_annotation(args: &AnnotArgs) -> Result<Annotation> {
    let need = |value: &Option<String>, flag: &str| -> Result<String> {
        value
            .clone()
            .with_context(|| format!("--kind {:?} needs {flag}", args.kind))
    };
    let kind = match args.kind {
        AnnotKindArg::Note => AnnotKind::Note {
            icon: NoteIcon::Comment,
            open: false,
        },
        AnnotKindArg::Square => AnnotKind::Square,
        AnnotKindArg::Circle => AnnotKind::Circle,
        AnnotKindArg::Line | AnnotKindArg::Arrow => AnnotKind::Line {
            from: parse_point(&need(&args.from, "--from")?)?,
            to: parse_point(&need(&args.to, "--to")?)?,
            ends: if args.kind == AnnotKindArg::Arrow {
                [LineEnd::None, LineEnd::OpenArrow]
            } else {
                [LineEnd::None, LineEnd::None]
            },
        },
        AnnotKindArg::Ink => {
            let strokes = need(&args.points, "--points")?
                .split(';')
                .map(|stroke| {
                    stroke
                        .split_whitespace()
                        .map(parse_point)
                        .collect::<Result<Vec<_>>>()
                })
                .filter(|s| !matches!(s, Ok(points) if points.is_empty()))
                .collect::<Result<Vec<_>>>()?;
            AnnotKind::Ink { strokes }
        }
    };
    let rect = match args.kind {
        AnnotKindArg::Note => ferrix_annot::note_rect(parse_point(&need(&args.at, "--at")?)?),
        AnnotKindArg::Square | AnnotKindArg::Circle => {
            let v = parse_numbers(&need(&args.rect, "--rect")?, 4)?;
            PdfBox::from_corners(PdfPoint::new(v[0], v[1]), PdfPoint::new(v[2], v[3]))
        }
        // Computed from the geometry when written.
        _ => PdfBox::default(),
    };
    let mut annot = Annotation::new(kind, rect);
    let props = &mut annot.props;
    props.color = Some(parse_color(&args.color)?);
    props.interior = args.fill.as_deref().map(parse_color).transpose()?;
    props.border_width = args.width;
    props.opacity = args.opacity;
    props.contents = args.contents.clone();
    props.author = args.author.clone();
    props.name = Some(ferrix_annot::unique_name());
    Ok(annot)
}

fn parse_numbers(s: &str, count: usize) -> Result<Vec<f32>> {
    let values = s
        .split(',')
        .map(|v| {
            v.trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .with_context(|| format!("invalid number {v:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if values.len() != count {
        bail!("expected {count} comma-separated numbers, got {s:?}");
    }
    Ok(values)
}

fn parse_point(s: &str) -> Result<PdfPoint> {
    let v = parse_numbers(s, 2)?;
    Ok(PdfPoint::new(v[0], v[1]))
}

fn parse_color(s: &str) -> Result<Rgb> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let channel = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .map(|v| f32::from(v) / 255.0)
    };
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Rgb::new(r, g, b)),
        _ => bail!("invalid colour {s:?}; use #rrggbb"),
    }
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

/// Reads a document from JSON, or from the source embedded in a Ferrix PDF.
fn read_document(path: &Path) -> Result<ferrix_compose::Document> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if bytes.starts_with(b"%PDF") {
        return match ferrix_compose::open_pdf(&bytes)? {
            ferrix_compose::Opened::Editable(doc) => Ok(doc),
            ferrix_compose::Opened::Changed(_) => {
                bail!(
                    "{} was changed by another application since Ferrix wrote it",
                    path.display()
                )
            }
            ferrix_compose::Opened::Plain => bail!("{} was not written by Ferrix", path.display()),
        };
    }
    Ok(ferrix_compose::Document::from_json(&bytes)?)
}

fn compose(input: &Path, output: &Path, system_fonts: bool) -> Result<()> {
    let doc = read_document(input)?;
    let mut fonts = if system_fonts {
        ferrix_compose::Fonts::with_system_fonts()
    } else {
        ferrix_compose::Fonts::bundled()
    };
    let layout = ferrix_compose::Layout::new(&doc, &mut fonts);
    let pdf = ferrix_compose::export_pdf(&doc, &layout, &fonts)?;
    ferrix_io::write_atomic(output, &pdf)
        .with_context(|| format!("cannot write {}", output.display()))?;
    eprintln!(
        "{} paragraphs, {} pages → {} ({} bytes)",
        doc.paragraphs.len(),
        layout.pages.len(),
        output.display(),
        pdf.len()
    );
    Ok(())
}

fn source(file: &Path) -> Result<()> {
    let doc = read_document(file)?;
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
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
    fn parses_annotation_arguments() {
        assert_eq!(
            parse_point(" 10, 20.5").expect("valid point"),
            PdfPoint::new(10.0, 20.5)
        );
        assert!(parse_point("10").is_err());
        assert!(parse_point("1,inf").is_err());
        assert_eq!(
            parse_color("#ff8000").expect("valid colour"),
            Rgb::new(1.0, 128.0 / 255.0, 0.0)
        );
        assert!(parse_color("#fff").is_err());
        assert!(parse_color("#gg0000").is_err());
    }

    #[test]
    fn rejects_bad_page_specs() {
        for spec in ["0", "4", "3-1", "a", "", "1-x"] {
            assert!(parse_pages(spec, 3).is_err(), "{spec:?} should fail");
        }
    }
}
