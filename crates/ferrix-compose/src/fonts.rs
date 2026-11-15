//! Fonts available to documents (ADR-0008 §6).

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{FontSystem, fontdb};

pub const SANS: &str = "Liberation Sans";
pub const SERIF: &str = "Liberation Serif";
pub const MONO: &str = "Liberation Mono";

/// Fonts that ship with Ferrix (SIL Open Font License 1.1, see `fonts/README.md`).
static BUNDLED: [&[u8]; 12] = [
    include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../fonts/LiberationSans-Italic.ttf"),
    include_bytes!("../fonts/LiberationSans-BoldItalic.ttf"),
    include_bytes!("../fonts/LiberationSerif-Regular.ttf"),
    include_bytes!("../fonts/LiberationSerif-Bold.ttf"),
    include_bytes!("../fonts/LiberationSerif-Italic.ttf"),
    include_bytes!("../fonts/LiberationSerif-BoldItalic.ttf"),
    include_bytes!("../fonts/LiberationMono-Regular.ttf"),
    include_bytes!("../fonts/LiberationMono-Bold.ttf"),
    include_bytes!("../fonts/LiberationMono-Italic.ttf"),
    include_bytes!("../fonts/LiberationMono-BoldItalic.ttf"),
];

/// The fonts layout and export can use.
pub struct Fonts {
    system: FontSystem,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("faces", &self.system.db().len())
            .finish()
    }
}

impl Fonts {
    /// Only the bundled fonts: layout is identical on every machine.
    pub fn bundled() -> Self {
        Self::build(false)
    }

    /// The bundled fonts plus the fonts installed on this computer.
    pub fn with_system_fonts() -> Self {
        Self::build(true)
    }

    fn build(system: bool) -> Self {
        let mut db = fontdb::Database::new();
        for data in BUNDLED {
            db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
        }
        if system {
            db.load_system_fonts();
        }
        db.set_sans_serif_family(SANS);
        db.set_serif_family(SERIF);
        db.set_monospace_family(MONO);
        Self {
            system: FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
        }
    }

    /// Family names, sorted, without duplicates.
    pub fn families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    }

    pub fn has_family(&self, family: &str) -> bool {
        self.system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == family))
    }

    pub(crate) fn system(&mut self) -> &mut FontSystem {
        &mut self.system
    }

    /// The fonts a layout uses, ready to embed; can be sent to another thread.
    pub fn for_embedding(&self, layout: &crate::layout::Layout) -> EmbedFonts {
        let mut map = HashMap::new();
        for glyph in layout
            .pages
            .iter()
            .flat_map(|p| &p.lines)
            .flat_map(|l| &l.glyphs)
        {
            if let std::collections::hash_map::Entry::Vacant(slot) = map.entry(glyph.font)
                && let Some(face) = self.face_data(glyph.font)
            {
                slot.insert(face);
            }
        }
        EmbedFonts(map)
    }

    /// The bytes and collection index of a face, for embedding.
    pub(crate) fn face_data(&self, id: fontdb::ID) -> Option<(krilla::Data, u32)> {
        let db = self.system.db();
        let face = db.face(id)?;
        let index = face.index;
        let data = match &face.source {
            fontdb::Source::Binary(bytes) => krilla::Data::from(Arc::clone(bytes)),
            _ => db.with_face_data(id, |bytes, _| krilla::Data::from(bytes.to_vec()))?,
        };
        Some((data, index))
    }
}

/// Font data for the faces a layout uses.
#[derive(Clone, Default)]
pub struct EmbedFonts(HashMap<fontdb::ID, (krilla::Data, u32)>);

impl EmbedFonts {
    pub(crate) fn get(&self, id: fontdb::ID) -> Option<&(krilla::Data, u32)> {
        self.0.get(&id)
    }
}

impl std::fmt::Debug for EmbedFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbedFonts")
            .field("faces", &self.0.len())
            .finish()
    }
}
