//! Crash recovery for unsaved edits (ADR-0003 §3).
//!
//! While a document has unsaved changes, its session keeps a journal in the app data
//! directory: a header naming the document, one record per inserted source, and a snapshot
//! of the page list after every change. Documents that exist only in memory (new or converted)
//! are written beside the journal as blob files. A clean close deletes it all; a crash leaves it
//! for the next launch to offer back.

use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use ferrix_core::PageList;
use ferrix_io::{
    FileStamp,
    journal::{self, JournalWriter},
};
use serde::{Deserialize, Serialize};

use crate::document::{Document, Origin};

const FORMAT_VERSION: u32 = 1;
const EXTENSION: &str = "journal";

#[derive(Debug, Serialize, Deserialize)]
enum Record {
    Header {
        version: u32,
        base: SavedOrigin,
        /// Seconds since the Unix epoch, for display.
        started: u64,
    },
    Source(SavedOrigin),
    Pages(PageList),
    /// The annotations (ADR-0009), when there are any. Ids depend on load order, so the whole
    /// set is kept rather than edits; recovery is refused anyway if the file changed.
    Annotations(ferrix_core::AnnotSet),
    /// A written document (ADR-0008) as it was when journaling started...
    Written(ferrix_compose::Document),
    /// ...and each edit applied to it since, undo and redo included.
    WriteEdit(ferrix_compose::Edit),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SavedOrigin {
    File {
        path: PathBuf,
        len: u64,
        modified_ns: Option<u128>,
    },
    /// Bytes kept in a blob file beside the journal.
    Untitled {
        name: String,
        suggested: Option<PathBuf>,
        blob: String,
    },
    /// A written document that was never saved; its content is in the journal itself.
    NewDocument { name: String },
}

fn recovery_dir() -> Option<PathBuf> {
    ferrix_io::app_data_dir().map(|d| d.join("recovery"))
}

fn nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos())
}

/// The journal of the document being edited in this session.
#[derive(Debug)]
pub(crate) struct Journal {
    writer: JournalWriter,
    dir: PathBuf,
    id: String,
    blobs: Vec<PathBuf>,
    /// How many of the document's sources have been recorded.
    sources_recorded: usize,
}

impl Journal {
    /// Starts journaling `doc`, recording its current state.
    pub(crate) fn start(doc: &Document) -> io::Result<Self> {
        let dir = recovery_dir().ok_or_else(|| io::Error::other("no app data directory"))?;
        Self::start_in(dir, doc)
    }

    fn start_in(dir: PathBuf, doc: &Document) -> io::Result<Self> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        std::fs::create_dir_all(&dir)?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let id = format!(
            "{started}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let writer = JournalWriter::create(&dir.join(format!("{id}.{EXTENSION}")))?;
        let mut journal = Self {
            writer,
            dir,
            id,
            blobs: Vec::new(),
            sources_recorded: 1,
        };
        let base = journal.save_origin(&doc.origin)?;
        journal.write(&Record::Header {
            version: FORMAT_VERSION,
            base,
            started,
        })?;
        journal.record(doc)?;
        Ok(journal)
    }

    /// Records sources added since the last call, then the current page list.
    pub(crate) fn record(&mut self, doc: &Document) -> io::Result<()> {
        while self.sources_recorded < doc.sources.len() {
            let saved = self.save_origin(&doc.sources[self.sources_recorded].origin)?;
            self.write(&Record::Source(saved))?;
            self.sources_recorded += 1;
        }
        self.write(&Record::Pages(doc.state.pages.clone()))?;
        // Also when empty: an empty set of loaded pages records that everything was deleted,
        // where no record would bring the file's annotations back on recovery.
        if !doc.state.annots.is_empty() || doc.state.annots.has_loaded() {
            self.write(&Record::Annotations(doc.state.annots.clone()))?;
        }
        Ok(())
    }

    /// Ends the session cleanly, deleting the journal and its blobs.
    pub(crate) fn discard(self) {
        for blob in &self.blobs {
            let _ = std::fs::remove_file(blob);
        }
        if let Err(e) = self.writer.discard() {
            tracing::warn!("could not delete recovery journal: {e}");
        }
    }

    fn write(&mut self, record: &Record) -> io::Result<()> {
        let payload = serde_json::to_vec(record).map_err(io::Error::other)?;
        self.writer.append(&payload)
    }

    fn save_origin(&mut self, origin: &Origin) -> io::Result<SavedOrigin> {
        Ok(match origin {
            Origin::File(path) => {
                let stamp = FileStamp::of(path).ok();
                SavedOrigin::File {
                    path: path.clone(),
                    len: stamp.map_or(0, |s| s.len),
                    modified_ns: stamp.and_then(|s| s.modified).and_then(nanos),
                }
            }
            Origin::Untitled {
                name,
                suggested,
                bytes,
            } => {
                let blob = format!("{}-{}.pdf", self.id, self.blobs.len());
                let path = self.dir.join(&blob);
                ferrix_io::write_atomic(&path, bytes)?;
                self.blobs.push(path);
                SavedOrigin::Untitled {
                    name: name.clone(),
                    suggested: suggested.clone(),
                    blob,
                }
            }
        })
    }
}

/// The journal of a document being written: a snapshot, then every edit.
#[derive(Debug)]
pub(crate) struct WriterJournal {
    writer: JournalWriter,
}

impl WriterJournal {
    /// Starts journaling a written document saved at `path` (or never saved).
    pub(crate) fn start(
        name: &str,
        path: Option<&Path>,
        doc: &ferrix_compose::Document,
    ) -> io::Result<Self> {
        let dir = recovery_dir().ok_or_else(|| io::Error::other("no app data directory"))?;
        Self::start_in(&dir, name, path, doc)
    }

    fn start_in(
        dir: &Path,
        name: &str,
        path: Option<&Path>,
        doc: &ferrix_compose::Document,
    ) -> io::Result<Self> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        std::fs::create_dir_all(dir)?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let id = format!(
            "{started}-{}-w{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let writer = JournalWriter::create(&dir.join(format!("{id}.{EXTENSION}")))?;
        let mut journal = Self { writer };
        let base = match path {
            Some(path) => {
                let stamp = FileStamp::of(path).ok();
                SavedOrigin::File {
                    path: path.to_path_buf(),
                    len: stamp.map_or(0, |s| s.len),
                    modified_ns: stamp.and_then(|s| s.modified).and_then(nanos),
                }
            }
            None => SavedOrigin::NewDocument {
                name: name.to_owned(),
            },
        };
        journal.write(&Record::Header {
            version: FORMAT_VERSION,
            base,
            started,
        })?;
        journal.write(&Record::Written(doc.clone()))?;
        Ok(journal)
    }

    pub(crate) fn record(&mut self, edits: &[ferrix_compose::Edit]) -> io::Result<()> {
        for edit in edits {
            self.write(&Record::WriteEdit(edit.clone()))?;
        }
        Ok(())
    }

    pub(crate) fn discard(self) {
        if let Err(e) = self.writer.discard() {
            tracing::warn!("could not delete recovery journal: {e}");
        }
    }

    fn write(&mut self, record: &Record) -> io::Result<()> {
        let payload = serde_json::to_vec(record).map_err(io::Error::other)?;
        self.writer.append(&payload)
    }
}

/// Unsaved work left behind by a session that ended unexpectedly.
#[derive(Debug, Clone)]
pub struct Recoverable {
    pub(crate) journal: PathBuf,
    pub(crate) base: Origin,
    /// The original file changed after the crash, so the recovered edits no longer apply.
    pub(crate) base_changed: bool,
    pub(crate) sources: Vec<Origin>,
    pub(crate) pages: PageList,
    pub(crate) annots: Option<ferrix_core::AnnotSet>,
    /// For a written document: its content with every journaled edit replayed.
    pub(crate) written: Option<ferrix_compose::Document>,
    pub(crate) started: SystemTime,
    blobs: Vec<PathBuf>,
}

impl Recoverable {
    pub(crate) fn name(&self) -> String {
        self.base.file_name()
    }

    /// Deletes the journal and its blobs.
    pub(crate) fn discard(&self) {
        for path in self.blobs.iter().chain([&self.journal]) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Finds journals left behind by crashed sessions. Blocking.
pub(crate) fn scan() -> Vec<Recoverable> {
    recovery_dir().map(|dir| scan_in(&dir)).unwrap_or_default()
}

fn scan_in(dir: &Path) -> Vec<Recoverable> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Recoverable> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .filter(|p| journal::is_orphaned(p))
        .filter_map(|p| match load(dir, &p) {
            Ok(Some(found)) => Some(found),
            Ok(None) => {
                // Nothing worth offering (e.g. crashed before the first change was recorded).
                let _ = std::fs::remove_file(&p);
                None
            }
            Err(e) => {
                tracing::warn!("unreadable recovery journal {}: {e}", p.display());
                None
            }
        })
        .collect();
    found.sort_by_key(|r| std::cmp::Reverse(r.started));
    found
}

fn load(dir: &Path, path: &Path) -> io::Result<Option<Recoverable>> {
    let mut header = None;
    let mut sources = Vec::new();
    let mut pages = None;
    let mut annots = None;
    let mut written: Option<ferrix_compose::Document> = None;
    let mut replay_failed = false;
    for payload in journal::read(path)? {
        match serde_json::from_slice::<Record>(&payload).map_err(io::Error::other)? {
            Record::Header {
                version: FORMAT_VERSION,
                base,
                started,
            } => header = Some((base, started)),
            Record::Header { version, .. } => {
                return Err(io::Error::other(format!(
                    "journal format {version} is unknown"
                )));
            }
            Record::Source(origin) => sources.push(origin),
            Record::Pages(list) => pages = Some(list),
            // A journal is untrusted input: a set that fails validation is dropped, and the
            // annotations are read from the file again instead.
            Record::Annotations(set) => {
                annots = match set.check() {
                    Ok(()) => Some(set),
                    Err(e) => {
                        tracing::warn!("ignoring invalid journaled annotations: {e}");
                        None
                    }
                };
            }
            Record::Written(doc) => written = Some(doc),
            Record::WriteEdit(edit) => {
                // Stop at an edit that no longer applies: everything before it is recovered.
                if let Some(doc) = written.as_mut().filter(|_| !replay_failed) {
                    replay_failed = doc.apply(&edit).is_err();
                }
            }
        }
    }
    if written.is_some() {
        pages = Some(PageList::identity(0));
    }
    let (Some((base, started)), Some(pages)) = (header, pages) else {
        return Ok(None);
    };

    let mut blobs = Vec::new();
    let mut restore = |saved: SavedOrigin| -> io::Result<(Origin, bool)> {
        Ok(match saved {
            SavedOrigin::File {
                path,
                len,
                modified_ns,
            } => {
                let now = FileStamp::of(&path).ok();
                let changed =
                    now.is_none_or(|s| s.len != len || s.modified.and_then(nanos) != modified_ns);
                (Origin::File(path), changed)
            }
            SavedOrigin::Untitled {
                name,
                suggested,
                blob,
            } => {
                let blob = dir.join(blob);
                let bytes = std::fs::read(&blob)?;
                blobs.push(blob);
                (Origin::untitled(name, suggested, bytes), false)
            }
            SavedOrigin::NewDocument { name } => (Origin::untitled(name, None, Vec::new()), false),
        })
    };
    let (base, mut base_changed) = restore(base)?;
    let mut restored = Vec::with_capacity(sources.len());
    for saved in sources {
        let (origin, changed) = restore(saved)?;
        base_changed |= changed;
        restored.push(origin);
    }
    Ok(Some(Recoverable {
        journal: path.to_path_buf(),
        base,
        base_changed,
        sources: restored,
        pages,
        annots,
        written,
        started: UNIX_EPOCH + std::time::Duration::from_secs(started),
        blobs,
    }))
}

/// When the unsaved work was last touched, for the recovery dialog.
pub(crate) fn describe(found: &Recoverable) -> String {
    let age = SystemTime::now()
        .duration_since(found.started)
        .map_or(0, |d| d.as_secs());
    let when = match age {
        0..60 => "moments ago".to_owned(),
        60..3600 => format!("{} min ago", age / 60),
        3600..86_400 => format!("{} h ago", age / 3600),
        _ => format!("{} days ago", age / 86_400),
    };
    match &found.base {
        Origin::File(_) => format!("Last edited {when}."),
        Origin::Untitled { .. } => format!("Never saved · last edited {when}."),
    }
}

#[cfg(test)]
mod tests {
    use ferrix_core::{Edit, SourceId};
    use ferrix_engine::{DocInfo, DocMetadata};
    use ferrix_types::{DocId, PageIndex, Rotation};

    use super::*;
    use crate::document::{Source, pages_label};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferrix-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn info(id: u64, pages: u32) -> DocInfo {
        DocInfo {
            id: DocId(id),
            page_count: pages,
            pdf_version: None,
            metadata: DocMetadata::default(),
        }
    }

    /// A file-backed document with a converted (in-memory) document inserted and edits made.
    fn edited_document(dir: &Path) -> Document {
        let path = dir.join("report.pdf");
        std::fs::write(&path, b"%PDF-1.7 stand-in").expect("write base");
        let mut doc = Document::new(Origin::File(path), info(1, 3));
        doc.sources.push(Source::new(
            Origin::untitled("scan.pdf".into(), None, b"%PDF converted".to_vec()),
            DocId(2),
        ));
        let inserted = doc.state.pages.new_entries(SourceId(1), [PageIndex(0)]);
        doc.edit(
            Edit::Insert {
                pages: vec![(1, inserted[0])],
            },
            pages_label("Insert", 1),
        )
        .expect("insert");
        let first = doc.key_at(0).expect("page");
        doc.edit(
            Edit::Rotate {
                keys: vec![first],
                by: Rotation::R90,
            },
            pages_label("Rotate", 1),
        )
        .expect("rotate");
        doc
    }

    #[test]
    fn a_crashed_session_is_found_with_its_latest_state() {
        let dir = scratch("crash");
        let doc = edited_document(&dir);
        let mut journal = Journal::start_in(dir.join("recovery"), &doc).expect("start");
        assert!(
            scan_in(&dir.join("recovery")).is_empty(),
            "live sessions are not offered"
        );

        let last = doc.key_at(3).expect("page");
        let mut doc = doc;
        doc.edit(Edit::Remove { keys: vec![last] }, pages_label("Delete", 1))
            .expect("delete");
        journal.record(&doc).expect("record");
        drop(journal); // the process dies without discarding

        let found = scan_in(&dir.join("recovery"));
        assert_eq!(found.len(), 1);
        let found = &found[0];
        assert_eq!(found.pages, doc.state.pages);
        assert!(!found.base_changed);
        assert_eq!(found.name(), "report.pdf");
        match &found.sources[..] {
            [Origin::Untitled { name, bytes, .. }] => {
                assert_eq!(name, "scan.pdf");
                assert_eq!(bytes.as_slice(), b"%PDF converted");
            }
            other => panic!("unexpected sources {other:?}"),
        }

        found.discard();
        let left: Vec<_> = std::fs::read_dir(dir.join("recovery"))
            .expect("list")
            .collect();
        assert!(left.is_empty(), "discard removes the journal and its blobs");
    }

    /// Deleting every annotation is unsaved work too: the journal must say so, or recovery
    /// would read the file's annotations again.
    #[test]
    fn deleting_every_annotation_survives_a_crash() {
        let dir = scratch("annots");
        let mut doc = edited_document(&dir);
        let page = doc.key_at(0).expect("page");
        let mut square = ferrix_core::Annotation::new(
            ferrix_core::AnnotKind::Square,
            ferrix_types::PdfBox::default(),
        );
        square.origin = ferrix_core::AnnotOrigin::Existing {
            source: SourceId::BASE,
            object: ferrix_core::ObjectRef {
                number: 12,
                generation: 0,
            },
        };
        let id = doc.state.annots.insert_loaded(page, [square])[0];
        doc.edit(
            ferrix_core::AnnotEdit::Remove { id },
            "Delete Annotation".into(),
        )
        .expect("delete");
        assert!(doc.state.annots.is_empty());

        let mut journal = Journal::start_in(dir.join("recovery"), &doc).expect("start");
        journal.record(&doc).expect("record");
        drop(journal);

        let found = scan_in(&dir.join("recovery"));
        let annots = found[0].annots.as_ref().expect("annotations journaled");
        assert!(
            annots.is_empty() && annots.is_loaded(page),
            "the deletion is kept"
        );
    }

    #[test]
    fn edits_to_a_file_changed_since_the_crash_are_flagged() {
        let dir = scratch("changed");
        let doc = edited_document(&dir);
        drop(Journal::start_in(dir.join("recovery"), &doc).expect("start"));
        std::fs::write(dir.join("report.pdf"), b"replaced by someone else").expect("modify");

        let found = scan_in(&dir.join("recovery"));
        assert!(found[0].base_changed);
    }

    #[test]
    fn a_crashed_writing_session_replays_its_edits() {
        use ferrix_compose::{Editor, Fonts};

        let dir = scratch("writing");
        let mut editor = Editor::new(ferrix_compose::Document::new(), Fonts::bundled());
        editor.type_text("Dear team,");
        let mut journal =
            WriterJournal::start_in(&dir.join("recovery"), "Letter.pdf", None, editor.document())
                .expect("start");
        editor.take_applied();
        editor.enter();
        editor.type_text("Second paragraph");
        editor.undo();
        editor.type_text("Typed again");
        journal.record(&editor.take_applied()).expect("record");
        drop(journal); // the process dies without discarding

        let found = scan_in(&dir.join("recovery"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), "Letter.pdf");
        assert!(!found[0].base_changed);
        assert_eq!(found[0].written.as_ref(), Some(editor.document()));
        found[0].discard();
        assert!(scan_in(&dir.join("recovery")).is_empty());
    }

    #[test]
    fn a_clean_close_leaves_nothing_behind() {
        let dir = scratch("clean");
        let doc = edited_document(&dir);
        Journal::start_in(dir.join("recovery"), &doc)
            .expect("start")
            .discard();
        assert!(scan_in(&dir.join("recovery")).is_empty());
        assert_eq!(
            std::fs::read_dir(dir.join("recovery"))
                .expect("list")
                .count(),
            0
        );
    }

    #[test]
    fn recovered_pages_must_fit_the_reopened_files() {
        let dir = scratch("fit");
        let doc = edited_document(&dir);
        assert!(crate::document::pages_fit(&doc.state.pages, &[3, 1]));
        assert!(
            !crate::document::pages_fit(&doc.state.pages, &[2, 1]),
            "base lost a page"
        );
        assert!(
            !crate::document::pages_fit(&doc.state.pages, &[3]),
            "inserted source missing"
        );
    }
}
