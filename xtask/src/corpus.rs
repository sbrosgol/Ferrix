//! The test corpus (`cargo xtask corpus`): real-world PDFs downloaded from a pinned, hashed
//! manifest (`tests/corpus/manifest.txt`), and generated ones that stress size, structure and
//! malformation. Everything lands in `tests/corpus/files/` (git-ignored).

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn corpus_dir(root: &Path) -> PathBuf {
    root.join("tests/corpus/files")
}

/// Downloads the manifest's files (skipping ones already present and correct) and writes the
/// generated set.
pub(crate) fn fetch(root: &Path, offline: bool) -> Result<()> {
    let dir = corpus_dir(root);
    if !offline {
        download(root, &dir.join("real"))?;
    }
    generate(&dir.join("generated"))?;
    println!("corpus ready in {}", dir.display());
    Ok(())
}

fn download(root: &Path, dest: &Path) -> Result<()> {
    let manifest = fs::read_to_string(root.join("tests/corpus/manifest.txt"))
        .context("reading tests/corpus/manifest.txt")?;
    fs::create_dir_all(dest)?;
    let mut base = None;
    let (mut fetched, mut kept) = (0, 0);
    for line in manifest.lines().map(str::trim) {
        if let Some(url) = line.strip_prefix("# base ") {
            base = Some(url.trim().to_owned());
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let (Some(sha), Some(_size), Some(name)) = (parts.next(), parts.next(), parts.next())
        else {
            bail!("bad manifest line: {line}");
        };
        // Names come from the manifest, which we control; still refuse anything path-like.
        if name.contains('/') || name.contains('\\') || name.starts_with('.') {
            bail!("refusing manifest name {name:?}");
        }
        let path = dest.join(name);
        if fs::read(&path).is_ok_and(|b| hex(&Sha256::digest(&b)) == sha) {
            kept += 1;
            continue;
        }
        let Some(base) = &base else {
            bail!("manifest has no `# base` URL before its files");
        };
        let url = format!("{base}/{}", name.replace(' ', "%20"));
        let mut bytes = Vec::new();
        ureq::get(&url)
            .call()
            .with_context(|| format!("downloading {url}"))?
            .into_body()
            .into_reader()
            .take(MAX_FILE_BYTES)
            .read_to_end(&mut bytes)?;
        let got = hex(&Sha256::digest(&bytes));
        if got != sha {
            bail!("{name}: SHA-256 {got}, expected {sha}");
        }
        fs::write(&path, &bytes)?;
        fetched += 1;
    }
    println!("real-world files: {fetched} downloaded, {kept} already present");
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A PDF assembled from object bodies (object `n` is `objects[n - 1]`), with a correct xref.
fn assemble(objects: &[String], root: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(content: &str) -> String {
    format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    )
}

/// A text document: `pages` pages of lines, an outline entry every ten pages and links on the
/// first page. Object 1 catalog, 2 page tree, 3 font, 4 outline root; then per page: page,
/// content; then outline items.
fn text_document(pages: usize, sizes: impl Fn(usize) -> (f32, f32, i32)) -> Vec<u8> {
    let mut objects = vec![String::new(); 4];
    objects[2] = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into();
    let page_obj = |i: usize| 5 + 2 * i;
    let mut kids = Vec::with_capacity(pages);
    for i in 0..pages {
        let (w, h, rotate) = sizes(i);
        let mut content = format!("BT /F1 28 Tf 72 {} Td (Page {}) Tj ET\n", h - 100.0, i + 1);
        for line in 0..30 {
            let y = h - 150.0 - line as f32 * 18.0;
            if y < 60.0 {
                break;
            }
            content.push_str(&format!(
                "BT /F1 11 Tf 72 {y} Td (Line {line} of page {} - the quick brown fox jumps over the lazy dog.) Tj ET\n",
                i + 1
            ));
        }
        content.push_str("0.2 0.4 0.8 rg 72 40 200 12 re f\n");
        let annots = if i == 0 && pages > 1 {
            let targets: Vec<String> = (1..pages.min(20))
                .map(|t| {
                    let y = h - 160.0 - t as f32 * 18.0;
                    format!(
                        "<< /Type /Annot /Subtype /Link /Rect [72 {y} 300 {}] /Border [0 0 0] /Dest [{} 0 R /XYZ 0 {h} 0] >>",
                        y + 14.0,
                        page_obj(t)
                    )
                })
                .collect();
            format!(" /Annots [{}]", targets.join(" "))
        } else {
            String::new()
        };
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate}{annots} /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            page_obj(i) + 1
        ));
        objects.push(stream(&content));
        kids.push(format!("{} 0 R", page_obj(i)));
    }
    objects[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {pages} >>",
        kids.join(" ")
    );
    let chapters: Vec<usize> = (0..pages).step_by(10).collect();
    let first_item = objects.len() + 1;
    for (n, &page) in chapters.iter().enumerate() {
        let item = first_item + n;
        let prev = if n > 0 {
            format!(" /Prev {} 0 R", item - 1)
        } else {
            String::new()
        };
        let next = if n + 1 < chapters.len() {
            format!(" /Next {} 0 R", item + 1)
        } else {
            String::new()
        };
        objects.push(format!(
            "<< /Title (Chapter {}) /Parent 4 0 R{prev}{next} /Dest [{} 0 R /Fit] >>",
            n + 1,
            page_obj(page)
        ));
    }
    objects[3] = if chapters.is_empty() {
        "<< /Type /Outlines /Count 0 >>".into()
    } else {
        format!(
            "<< /Type /Outlines /First {first_item} 0 R /Last {} 0 R /Count {} >>",
            first_item + chapters.len() - 1,
            chapters.len()
        )
    };
    objects[0] = "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".into();
    assemble(&objects, 1)
}

fn letter(_: usize) -> (f32, f32, i32) {
    (612.0, 792.0, 0)
}

/// A deterministic generator (xorshift), so broken files are the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn generate(dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("text-1000.pdf".into(), text_document(1000, letter)),
        ("text-5000.pdf".into(), text_document(5000, letter)),
        (
            "mixed-sizes.pdf".into(),
            text_document(200, |i| {
                let sizes = [
                    (420.0, 595.0),
                    (612.0, 792.0),
                    (842.0, 1191.0),
                    (2384.0, 3370.0),
                    (200.0, 200.0),
                ];
                let (w, h) = sizes[i % sizes.len()];
                let (w, h) = if i % 3 == 1 { (h, w) } else { (w, h) };
                (w, h, [0, 90, 180, 270][i % 4])
            }),
        ),
        // The largest page PDF allows (200 in, 14 400 pt): tiled rendering at any zoom.
        (
            "huge-page.pdf".into(),
            text_document(2, |_| (14_400.0, 14_400.0, 0)),
        ),
        (
            "tiny-pages.pdf".into(),
            text_document(50, |_| (3.0, 3.0, 0)),
        ),
    ];

    // Many annotations: 2 000 squares and notes over 20 pages.
    let base = text_document(20, letter);
    files.push((
        "annotations-2000.pdf".into(),
        with_annotations(&base, 20, 100),
    ));

    // Malformed variants of a small document.
    let small = text_document(30, letter);
    for cut in [10, 50, 90, 99] {
        files.push((
            format!("broken-truncated-{cut}.pdf"),
            small[..small.len() * cut / 100].to_vec(),
        ));
    }
    let xref_at = find_last(&small, b"xref\n").unwrap_or(0);
    let mut no_xref = small.clone();
    no_xref.truncate(xref_at);
    no_xref.extend_from_slice(b"%%EOF\n");
    files.push(("broken-no-xref.pdf".into(), no_xref));
    let mut bad_offsets = small.clone();
    for b in &mut bad_offsets[xref_at..] {
        if b.is_ascii_digit() {
            *b = b'9';
        }
    }
    files.push(("broken-xref-offsets.pdf".into(), bad_offsets));
    files.push((
        "broken-no-root.pdf".into(),
        assemble(&["<< /Type /Pages /Kids [] /Count 0 >>".into()], 7),
    ));
    // A page tree that contains itself.
    files.push((
        "broken-cyclic-tree.pdf".into(),
        assemble(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 2 >>".into(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
            ],
            1,
        ),
    ));
    // A page count that lies, and a page tree a million levels deep.
    files.push((
        "broken-count.pdf".into(),
        assemble(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 2147483647 >>".into(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
            ],
            1,
        ),
    ));
    let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    files.push((
        "broken-deep-nesting.pdf".into(),
        assemble(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Junk {deep} >>"),
            ],
            1,
        ),
    ));
    // Seeded byte flips: the fuzzing a soak can afford.
    let mut rng = Rng(0x5eed_f3e1_c0de_cafe);
    for n in 0..40 {
        let mut bytes = small.clone();
        for _ in 0..(1 + rng.below(30)) {
            let at = rng.below(bytes.len());
            bytes[at] = (rng.next() & 0xff) as u8;
        }
        files.push((format!("fuzz-{n:02}.pdf"), bytes));
    }

    for (name, bytes) in &files {
        fs::write(dest.join(name), bytes)?;
    }
    // Encrypted files made by qpdf, from the library's test fixtures.
    let encrypted =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/ferrix-pdf/tests/fixtures/encrypted");
    let mut copied = 0;
    for entry in fs::read_dir(&encrypted)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "pdf")
            && let Some(name) = path.file_name()
        {
            fs::copy(
                &path,
                dest.join(format!("encrypted-{}", name.to_string_lossy())),
            )?;
            copied += 1;
        }
    }
    println!(
        "generated files: {} written, {copied} encrypted copied",
        files.len()
    );
    Ok(())
}

/// `pdf` with `per_page` squares and notes on each of its first `pages` pages, as an
/// incremental update.
fn with_annotations(pdf: &[u8], pages: usize, per_page: usize) -> Vec<u8> {
    // Page i is object 5 + 2i (see `text_document`); rewrite each page dictionary with /Annots.
    let mut out = pdf.to_vec();
    let mut next = count_objects(pdf) + 1;
    let mut objects = Vec::new();
    for i in 0..pages {
        let page = 5 + 2 * i;
        let mut ids = Vec::new();
        for n in 0..per_page {
            let (x, y) = (40.0 + (n % 10) as f32 * 52.0, 60.0 + (n / 10) as f32 * 64.0);
            let body = if n % 2 == 0 {
                format!(
                    "<< /Type /Annot /Subtype /Square /Rect [{x} {y} {} {}] /C [1 0 0] /Contents (Square {n}) >>",
                    x + 40.0,
                    y + 40.0
                )
            } else {
                format!(
                    "<< /Type /Annot /Subtype /Text /Rect [{x} {y} {} {}] /Contents (Note {n} on page {}) /T (Soak) >>",
                    x + 20.0,
                    y + 20.0,
                    i + 1
                )
            };
            objects.push((next, body));
            ids.push(format!("{next} 0 R"));
            next += 1;
        }
        objects.push((
            page,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                ids.join(" "),
                page + 1
            ),
        ));
    }
    let prev = find_last(pdf, b"startxref\n")
        .and_then(|at| {
            std::str::from_utf8(&pdf[at + 10..])
                .ok()?
                .lines()
                .next()?
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);
    let mut offsets = Vec::new();
    for (id, body) in &objects {
        offsets.push((*id, out.len()));
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    offsets.sort_unstable();
    for (id, offset) in &offsets {
        out.extend_from_slice(format!("{id} 1\n{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {next} /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

fn count_objects(pdf: &[u8]) -> usize {
    find_last(pdf, b"/Size ")
        .and_then(|at| {
            let rest = std::str::from_utf8(&pdf[at + 6..]).ok()?;
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<usize>()
                .ok()
        })
        .map_or(0, |size| size - 1)
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}
