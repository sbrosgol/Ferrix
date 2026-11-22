# ADR-0003: Edit model — commands, overlays, incremental save

- Status: **Accepted**
- Date: 2026-11-15

## Context
Users expect unlimited undo, instant visual feedback, crash recovery, and saves that don't break
existing digital signatures or corrupt files. Engines are slow to re-render after every mutation.

## Decision
1. **All mutations are typed `Command`s** that are serializable and return their inverse on apply.
   The `CommandStack` provides undo/redo; adjacent compatible commands coalesce (ink strokes, typing).
2. **Two-phase visual model:** in-progress edits render as **vector overlays** drawn by Ferrix.
   On commit, the command is pushed to the engine and only the affected page region is re-rendered.
3. **Commands are journaled** (append-only, `postcard` frames with checksums) for crash recovery
   by replay onto the original file.
4. **Save defaults to incremental update** (append-only), which preserves existing signatures and
   is fast. **Full rewrite** is used for "Save As → optimize", and is **mandatory after redaction**
   so removed content is physically purged.
5. **Saves are atomic and verified:** temp file → fsync → reopen and validate → atomic replace.

## Consequences
- + Undo, recovery, scripting (`ferrix-cli apply edits.json`) and testing share one mechanism.
- + Property tests: `apply ∘ inverse = id`, `save ∘ reopen` preserves model.
- − Every feature must implement a correct inverse — including page deletes (keep removed pages in
  the session until save) and redaction (undo allowed only before save).
- − Incremental saves grow files; we offer "Optimize" and suggest it when update sections pile up.

## Implementation notes (page organization, 2026-11-15)
- The edited document is a `PageList` in `ferrix-core`: entries pointing at (source document,
  page, extra rotation). Edits are a serializable `Edit` enum (`Rotate`, `Remove`, `Insert`,
  `Reorder`) whose `apply` returns the inverse; `History` provides undo/redo and dirty tracking.
  A randomized test checks that every edit sequence round-trips through its inverses.
- Rendering maps entries to open engine documents, so edits show instantly without touching
  the file. PDFium stays read-only.
- Saving goes through `ferrix-pdf` (lopdf 0.46): the new page tree is appended as an
  **incremental update** — the original bytes are preserved verbatim. Inherited page attributes
  (`Resources`, `MediaBox`, `CropBox`, `Rotate`) are copied onto each page before re-parenting.
  Inserted pages are copied with everything they reference.
- Every save is re-parsed by lopdf and re-opened by PDFium (page count must match) before
  `ferrix-io::write_atomic` replaces the file (temp file → fsync → rename → fsync dir).
- A save is refused if the file changed on disk since it was opened (size/mtime stamp).
- Known limits: undo history resets after a save; deleted pages' objects remain in the file
  until a future full rewrite ("Optimize"); bookmarks and form fields pointing at deleted pages
  are left dangling; form fields on inserted pages are not merged into the AcroForm.

## Implementation notes (crash recovery, 2026-11-15)
- While a document has unsaved changes, its session writes a journal
  (`<app data>/recovery/*.journal`; `~/Library/Application Support/Ferrix` on macOS,
  `%LOCALAPPDATA%\Ferrix` on Windows, `$XDG_STATE_HOME/ferrix` on Linux).
- Records are **page-list snapshots**, not individual commands: page lists are small, and a
  snapshot cannot diverge on replay. The undo history is not recovered — the recovered state
  opens as one unsaved change.
- Format (`ferrix-io::journal`): magic header, then `len | crc32 | JSON payload` frames, each
  `fsync`ed before the edit completes. Reading stops at the first torn or corrupt frame.
- Each live session holds an exclusive OS lock on its journal (`File::try_lock`, hence MSRV
  1.89+). On launch, journals nobody holds are offered back: Recover, Later, or Discard.
- In-memory documents (new or converted) are stored as blob files beside the journal.
- If the original file changed after the crash, recovery is refused rather than applying
  page edits to different content.
- Clean close, save, discard and quit delete the journal and its blobs.
- Privacy: the journal holds file paths and, for unsaved new/converted documents, their
  content, in the user's own app-data directory only.

## Encrypted documents (2026-11-22)
Encrypted PDFs are view-only in the app, whether or not they need a password to open. Ferrix
asks for a password when one is needed (ADR-0005's history never stores it). Files with only an
owner password open without asking. The engine reports encryption on open (`DocInfo::encrypted`,
from PDFium's security handler revision), page and annotation edits are off, Markup mode stays
off, and pages of an encrypted file can't be inserted elsewhere. Both places where edits land
(`App::apply` and `App::apply_annot`) refuse too, so nothing can be saved by another path. Writing an incremental update into an encrypted file would mean encrypting the
new objects with the file's keys, and a mistake there would leave content readable without the
password. Revisit when saving can keep the encryption and is tested against Acrobat and qpdf.

`ferrix-pdf` itself can still write an incremental update into an encrypted file (a test
organizes one), and the CLI's `organize` and `annotate` use that. Its check only re-reads the
output with the password and counts pages; whether the added objects are encrypted correctly
has not been verified against other readers.

