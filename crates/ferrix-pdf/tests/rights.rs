//! Permissions of encrypted PDFs, against files encrypted by qpdf (not by lopdf, which reads
//! them), with RC4 (R3), AES-128 (R4) and AES-256 (R6). See `fixtures/encrypted/make.py`.

#![allow(clippy::unwrap_used)]

use ferrix_pdf::{EditRights, Source, edit_rights};

const CIPHERS: [&str; 3] = ["rc4", "aes128", "aes256"];

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/encrypted/{name}.pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn rights(name: &str, password: Option<&str>) -> EditRights {
    let bytes = fixture(name);
    edit_rights(Source {
        bytes: &bytes,
        password,
    })
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_locked_file_allows_nothing_without_its_owner_password() {
    for cipher in CIPHERS {
        let name = format!("{cipher}-locked");
        let r = rights(&name, None);
        assert!(
            !r.owner && !r.modify && !r.annotate && !r.assemble,
            "{name}: {r:?}"
        );
        // The owner password lifts every limit.
        let r = rights(&name, Some("owner"));
        assert!(
            r.owner && r.modify && r.annotate && r.assemble,
            "{name}: {r:?}"
        );
        // A wrong password is no owner password.
        assert!(!rights(&name, Some("nope")).owner, "{name}");
    }
}

#[test]
fn notes_can_be_allowed_without_changing_pages() {
    for cipher in CIPHERS {
        let name = format!("{cipher}-notes");
        let r = rights(&name, None);
        assert!(r.annotate && !r.assemble && !r.modify, "{name}: {r:?}");
    }
}

#[test]
fn unrestricted_and_unencrypted_files_allow_everything() {
    for cipher in CIPHERS {
        let name = format!("{cipher}-open");
        let r = rights(&name, None);
        assert!(
            r.modify && r.annotate && r.assemble && !r.owner,
            "{name}: {r:?}"
        );
        // With a user password: the user password is not the owner's.
        let r = rights(&format!("{cipher}-user"), Some("user"));
        assert!(
            r.modify && r.annotate && r.assemble && !r.owner,
            "{cipher}-user: {r:?}"
        );
    }
    let plain = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[]/Count 0>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
    let r = edit_rights(Source::new(plain)).unwrap();
    assert!(r.owner && r.modify && r.annotate && r.assemble);
}
