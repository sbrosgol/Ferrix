---
name: ferrix-verify
description: Full pre-commit check for Ferrix - formatting, clippy, the whole test suite with PDFium really running, and a licence check of new dependencies. Use before committing, before saying work is done, or when asked to "check everything".
---

# Verify Ferrix

Run these from the repository root and report actual results (counts, failures with output).

1. **PDFium present** — tests that need it otherwise skip silently:
   ```sh
   ls vendor/pdfium >/dev/null 2>&1 || cargo xtask fetch-pdfium
   ```
2. **Format and lint** (the only acceptable warning is the `block v0.1.6` future-incompat note):
   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets 2>&1 | grep -E "^(warning|error)" | grep -v "block v0.1.6\|future-incompat"
   ```
3. **Tests**, summed so nothing hides in the noise:
   ```sh
   cargo test --workspace 2>&1 | grep -E "test result|FAILED|panicked" \
     | awk '/panicked|FAILED/ {print} /test result/ {s+=$4; f+=$6} END {print "passed", s, "failed", f}'
   ```
   Don't filter with `grep -v "0 passed"`: it also hides "10 passed".
4. **PDFium tests really ran** (spot check one):
   ```sh
   cargo test -p ferrix-engine-pdfium --test engine -- --nocapture 2>&1 | grep -c skipping   # expect 0
   ```
5. **Licences** — `cargo-deny` may not be installed locally (`which cargo-deny`). If it is, run
   `cargo deny check`. If not, and dependencies changed, list non-permissive licences in the new
   crate's tree:
   ```sh
   cargo tree -p <crate> --target all -e normal --prefix none -f '{p} {l}' | sort -u \
     | grep -vE "MIT|Apache|BSD|Zlib|ISC|Unicode|BSL|CC0"
   ```
   Any output is a problem: Ferrix allows permissive licences only (`deny.toml`).
6. If UI code changed, also take a screenshot with the `ferrix-screenshot` skill and look at it.

Report: fmt/clippy status, `passed N failed M`, whether PDFium tests ran, licence result.
