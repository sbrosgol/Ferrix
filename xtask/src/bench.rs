//! `cargo xtask bench`: the M1 exit criteria (docs/roadmap.md) measured on the corpus, with a
//! pass or fail for each. Builds and runs `ferrix-app`'s `bench` example in release mode.

use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Target: first paint of a 1 000-page document.
const FIRST_PAINT_MS: f64 = 300.0;
/// Target: 60 fps; the CPU side of a frame must fit in one.
const FRAME_MS: f64 = 1000.0 / 60.0;

/// The documents measured: the generated ones the criteria name, then the largest real ones.
const GENERATED: [&str; 5] = [
    "text-1000.pdf",
    "text-5000.pdf",
    "mixed-sizes.pdf",
    "huge-page.pdf",
    "annotations-2000.pdf",
];
const REAL: usize = 5;

pub(crate) fn bench(root: &Path) -> Result<()> {
    let corpus = crate::corpus::corpus_dir(root);
    let mut files: Vec<_> = GENERATED
        .iter()
        .map(|f| corpus.join("generated").join(f))
        .filter(|p| p.is_file())
        .collect();
    if files.is_empty() {
        bail!("no corpus; run `cargo xtask corpus` first");
    }
    let mut real = crate::soak::list_pdfs(&corpus.join("real"))?;
    real.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map_or(0, |m| m.len())));
    files.extend(real.into_iter().take(REAL));

    let data = tempdir(root)?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "--release",
            "-p",
            "ferrix-app",
            "--example",
            "bench",
            "--",
        ])
        .args(&files)
        // Never touch the user's settings, history or recovery journals.
        .env("FERRIX_DATA_DIR", &data)
        .envs(crate::soak::pdfium_dir(&root.join("target/release/ferrix")))
        .output()
        .context("running the bench example")?;
    let _ = std::fs::remove_dir_all(&data);
    if !output.status.success() {
        bail!(
            "the bench example failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    println!(
        "{:<36} {:>11} {:>11} {:>11} {:>12}",
        "document", "first paint", "frame p95", "view p95", "render p95"
    );
    let mut failures = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let file = v["file"].as_str().unwrap_or("?");
        let name = Path::new(file)
            .strip_prefix(&corpus)
            .map_or(file.to_owned(), |p| p.display().to_string());
        let paint = v["first_paint_ms"].as_f64();
        let frame = v["full_frame_ms"]["p95"].as_f64().unwrap_or(f64::NAN);
        let view = v["frame_ms"]["p95"].as_f64().unwrap_or(f64::NAN);
        let render = v["render_ms"]["p95"].as_f64().unwrap_or(f64::NAN);
        let paint_text = paint.map_or("none".to_owned(), |p| format!("{p:.0} ms"));
        println!("{name:<36} {paint_text:>11} {frame:>8.2} ms {view:>8.2} ms {render:>9.1} ms");
        match paint {
            None => failures.push(format!("{name}: never painted")),
            Some(p) if name.ends_with("text-1000.pdf") && p > FIRST_PAINT_MS => {
                failures.push(format!(
                    "{name}: first paint {p:.0} ms > {FIRST_PAINT_MS} ms"
                ));
            }
            _ => {}
        }
        if frame > FRAME_MS {
            failures.push(format!("{name}: frame {frame:.1} ms > {FRAME_MS:.1} ms"));
        }
    }
    println!(
        "\ntargets: first paint of text-1000.pdf < {FIRST_PAINT_MS} ms; frame (update, view, \
         layout, draw; not rasterizing) < {FRAME_MS:.1} ms at p95"
    );
    if failures.is_empty() {
        println!("PASS");
        Ok(())
    } else {
        for f in &failures {
            println!("FAIL {f}");
        }
        bail!("{} target(s) missed", failures.len())
    }
}

fn tempdir(root: &Path) -> Result<std::path::PathBuf> {
    let dir = root
        .join("target")
        .join(format!("bench-data-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
