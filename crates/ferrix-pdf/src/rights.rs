//! What an encrypted PDF's permissions allow (ISO 32000-2 §7.6.4.2, Table 22).
//!
//! A file with only an owner password opens for anyone, but its `/P` entry says what a reader
//! may change. Those limits bind unless the owner password is given. lopdf keeps the security
//! handler's values after decrypting but drops the `/Encrypt` dictionary itself, so the owner
//! password is checked against a dictionary rebuilt from those values.

use lopdf::{Document, Object, encryption::Permissions};

use crate::{PdfError, Source, layout::load};

/// The changes a file's permissions allow with the password given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditRights {
    /// Change the content: bit 4.
    pub modify: bool,
    /// Add or change annotations: bit 6.
    pub annotate: bool,
    /// Insert, rotate or delete pages: bit 11 (revision 3 and later), or bit 4.
    pub assemble: bool,
    /// The given password is the owner password, so no limit applies.
    pub owner: bool,
}

impl EditRights {
    const ALL: Self = Self {
        modify: true,
        annotate: true,
        assemble: true,
        owner: true,
    };
}

/// The rights the permissions of `source` grant with `source.password`. Unencrypted files,
/// and encrypted ones opened with the owner password, may be changed in every way.
pub fn edit_rights(source: Source<'_>) -> Result<EditRights, PdfError> {
    let doc = load(source)?;
    let Some(state) = doc.encryption_state.clone() else {
        return Ok(EditRights::ALL);
    };
    if is_owner(&doc, &state, source.password.unwrap_or("")) {
        return Ok(EditRights::ALL);
    }
    let p = state.permissions();
    let modify = p.contains(Permissions::MODIFIABLE);
    // Revision 2 has no assembly bit; changing pages is then a change to the content.
    let assemble = if state.revision() >= 3 {
        modify || p.contains(Permissions::ASSEMBLABLE)
    } else {
        modify
    };
    Ok(EditRights {
        modify,
        annotate: p.contains(Permissions::ANNOTABLE),
        assemble,
        owner: false,
    })
}

/// Whether `password` is the owner password of the document `state` decrypted.
fn is_owner(doc: &Document, state: &lopdf::EncryptionState, password: &str) -> bool {
    // A scratch document with the same /ID and a rebuilt /Encrypt is enough to check it.
    let mut scratch = Document::with_version("1.7");
    if let Ok(id) = doc.trailer.get(b"ID") {
        scratch.trailer.set("ID", id.clone());
    } else {
        scratch.trailer.set("ID", Object::Array(Vec::new()));
    }
    scratch.encrypt(state).is_ok() && scratch.authenticate_owner_password(password).is_ok()
}
