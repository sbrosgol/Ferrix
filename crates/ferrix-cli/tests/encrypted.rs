//! The CLI on encrypted PDFs made by qpdf (`ferrix-pdf/tests/fixtures/encrypted`): saves keep
//! working with a user password, and a file's permissions bind unless its owner password is
//! given (ISO 32000-2 §7.6.4.2).
//!
//! Needs PDFium for the checks the CLI runs after writing; skipped locally without it, as the
//! engine tests are, and required on CI.

#![allow(clippy::unwrap_used)]

use std::{path::PathBuf, process::Command};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ferrix-pdf/tests/fixtures/encrypted")
        .join(format!("{name}.pdf"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ferrix-cli-encrypted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

/// Runs `ferrix` with `args`; `None` when PDFium is missing outside CI.
fn ferrix(args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_ferrix"))
        .args(args)
        .env_remove("FERRIX_PDF_PASSWORD")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stderr.contains("PDFium") && std::env::var_os("CI").is_none() && !out.status.success() {
        eprintln!("skipping: {stderr}");
        return None;
    }
    Some((out.status.success(), stderr))
}

#[test]
fn organize_works_on_files_that_need_a_user_password() {
    for cipher in ["rc4", "aes128", "aes256"] {
        let input = fixture(&format!("{cipher}-user"));
        let output = scratch(&format!("{cipher}-organized.pdf"));
        let Some((ok, err)) = ferrix(&[
            "organize",
            "--password",
            "user",
            "--pages",
            "2,1",
            "-o",
            output.to_str().unwrap(),
            input.to_str().unwrap(),
        ]) else {
            return;
        };
        assert!(ok, "{cipher}: {err}");
        // Still encrypted: it opens with the password, and only with it.
        let Some((ok, err)) = ferrix(&["info", "--password", "user", output.to_str().unwrap()])
        else {
            return;
        };
        assert!(ok, "{cipher}: {err}");
        let Some((ok, _)) = ferrix(&["info", output.to_str().unwrap()]) else {
            return;
        };
        assert!(!ok, "{cipher}: the output must still need its password");
    }
}

#[test]
fn permissions_bind_unless_the_owner_password_is_given() {
    for cipher in ["rc4", "aes128", "aes256"] {
        let locked = fixture(&format!("{cipher}-locked"));
        let locked = locked.to_str().unwrap();
        let out = scratch(&format!("{cipher}-out.pdf"));
        let out = out.to_str().unwrap();
        let note = [
            "--page", "1", "--kind", "note", "--at", "100,700", "-o", out,
        ];

        let Some((ok, err)) = ferrix(&["organize", "--pages", "2,1", "-o", out, locked]) else {
            return;
        };
        assert!(
            !ok && err.contains("permissions"),
            "{cipher} organize: {err}"
        );
        let mut add = vec!["annotate", "add"];
        add.extend(note);
        add.push(locked);
        let Some((ok, err)) = ferrix(&add) else {
            return;
        };
        assert!(
            !ok && err.contains("permissions"),
            "{cipher} annotate: {err}"
        );
        assert!(!std::path::Path::new(out).exists(), "nothing is written");

        // The owner password overrides them.
        let Some((ok, err)) = ferrix(&[
            "organize",
            "--password",
            "owner",
            "--pages",
            "2,1",
            "-o",
            out,
            locked,
        ]) else {
            return;
        };
        assert!(ok, "{cipher} organize as owner: {err}");

        // A file that allows notes but not page changes.
        let notes = fixture(&format!("{cipher}-notes"));
        let mut add = vec!["annotate", "add"];
        add.extend(note);
        add.push(notes.to_str().unwrap());
        let Some((ok, err)) = ferrix(&add) else {
            return;
        };
        assert!(ok, "{cipher} annotate where notes are allowed: {err}");
        let Some((ok, err)) = ferrix(&[
            "organize",
            "--pages",
            "1",
            "-o",
            out,
            notes.to_str().unwrap(),
        ]) else {
            return;
        };
        assert!(!ok && err.contains("permissions"), "{cipher}: {err}");
    }
}
