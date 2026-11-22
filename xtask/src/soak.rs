//! The soak test (`cargo xtask soak`): runs every corpus file through `ferrix stress`, each in
//! its own process, round after round until the time is up. A refused file is fine; a crash
//! (signal), panic (exit 101) or hang (killed after the timeout) fails the soak, naming the file.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Default)]
struct FileReport {
    runs: u32,
    refused: u32,
    failures: Vec<String>,
    slowest_ms: f64,
}

pub(crate) fn soak(root: &Path, minutes: f64, timeout_secs: u64, only: Option<&str>) -> Result<()> {
    let corpus = crate::corpus::corpus_dir(root);
    let mut files = list_pdfs(&corpus)?;
    if let Some(filter) = only {
        files.retain(|f| f.to_string_lossy().contains(filter));
    }
    if files.is_empty() {
        bail!(
            "no PDFs in {}; run `cargo xtask corpus` first",
            corpus.display()
        );
    }
    let ferrix = build_cli(root)?;
    let deadline = Instant::now() + Duration::from_secs_f64(minutes * 60.0);
    let timeout = Duration::from_secs(timeout_secs);
    let mut reports: BTreeMap<PathBuf, FileReport> = BTreeMap::new();
    let mut rounds = 0;
    let started = Instant::now();
    'soak: loop {
        rounds += 1;
        for file in &files {
            if rounds > 1 && Instant::now() >= deadline {
                break 'soak;
            }
            let report = reports.entry(file.clone()).or_default();
            report.runs += 1;
            let start = Instant::now();
            match run_one(&ferrix, file, timeout)? {
                Outcome::Ran { refused } => {
                    if refused {
                        report.refused += 1;
                    }
                }
                Outcome::Failed(why) => {
                    eprintln!("FAIL {}: {why}", rel(&corpus, file));
                    report.failures.push(why);
                }
            }
            report.slowest_ms = report
                .slowest_ms
                .max(start.elapsed().as_secs_f64() * 1000.0);
        }
        // One round always completes, so every file is tried at least once.
        if Instant::now() >= deadline {
            break;
        }
    }

    let failed: Vec<_> = reports
        .iter()
        .filter(|(_, r)| !r.failures.is_empty())
        .collect();
    let runs: u32 = reports.values().map(|r| r.runs).sum();
    let refused = reports.values().filter(|r| r.refused > 0).count();
    let mut slowest: Vec<_> = reports.iter().collect();
    slowest.sort_by(|a, b| b.1.slowest_ms.total_cmp(&a.1.slowest_ms));
    println!(
        "\nsoak: {} files, {rounds} rounds, {runs} runs in {:.1} min",
        files.len(),
        started.elapsed().as_secs_f64() / 60.0
    );
    println!("refused by the engine or parser (fine): {refused} files");
    println!("slowest files:");
    for (file, r) in slowest.iter().take(5) {
        println!("  {:>8.0} ms  {}", r.slowest_ms, rel(&corpus, file));
    }
    let report_path = root.join("target/soak-report.json");
    let json: serde_json::Value = reports
        .iter()
        .map(|(f, r)| {
            (
                rel(&corpus, f),
                serde_json::json!({
                    "runs": r.runs,
                    "refused": r.refused,
                    "failures": r.failures,
                    "slowest_ms": r.slowest_ms,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    fs::write(&report_path, serde_json::to_vec_pretty(&json)?)?;
    println!("report: {}", report_path.display());
    if failed.is_empty() {
        println!("PASS: no crashes, panics or hangs");
        Ok(())
    } else {
        for (file, r) in &failed {
            println!("FAIL {}: {}", rel(&corpus, file), r.failures.join("; "));
        }
        bail!(
            "{} of {} files crashed, panicked or hung",
            failed.len(),
            files.len()
        )
    }
}

enum Outcome {
    Ran { refused: bool },
    Failed(String),
}

fn run_one(ferrix: &Path, file: &Path, timeout: Duration) -> Result<Outcome> {
    let mut child = Command::new(ferrix)
        .arg("stress")
        .arg(file)
        .env_remove("FERRIX_PDF_PASSWORD")
        .envs(pdfium_dir(ferrix))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", ferrix.display()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Outcome::Failed(format!(
                "hung (killed after {}s)",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !status.success() {
        let why = match status.code() {
            Some(101) => format!("panicked: {}", last_line(&stderr)),
            Some(code) => format!("exited with {code}: {}", last_line(&stderr)),
            None => format!("crashed ({status})"),
        };
        return Ok(Outcome::Failed(why));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let refused = stdout.contains("\"ok\":false");
    Ok(Outcome::Ran { refused })
}

/// Release builds don't look in the checkout's `vendor/pdfium`; point them at it unless the
/// caller already chose a PDFium.
pub(crate) fn pdfium_dir(ferrix: &Path) -> Vec<(String, PathBuf)> {
    const VAR: &str = "FERRIX_PDFIUM_DIR";
    if std::env::var_os(VAR).is_some() {
        return Vec::new();
    }
    let vendor = ferrix
        .ancestors()
        .nth(3)
        .map(|root| root.join("vendor/pdfium"));
    vendor
        .filter(|v| v.is_dir())
        .map(|v| vec![(VAR.to_owned(), v)])
        .unwrap_or_default()
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(300)
        .collect()
}

fn rel(base: &Path, file: &Path) -> String {
    file.strip_prefix(base)
        .unwrap_or(file)
        .display()
        .to_string()
}

pub(crate) fn list_pdfs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(files);
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(list_pdfs(&path)?);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Builds `ferrix` in release mode (what users run) and returns its path.
pub(crate) fn build_cli(root: &Path) -> Result<PathBuf> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args(["build", "--release", "-p", "ferrix-cli"])
        .status()?;
    if !status.success() {
        bail!("building ferrix-cli failed");
    }
    let exe = root
        .join("target/release/ferrix")
        .with_extension(std::env::consts::EXE_EXTENSION);
    Ok(exe)
}
