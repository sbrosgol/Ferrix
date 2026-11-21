# ADR-0009: Annotations

- Status: **Accepted**
- Date: 2026-11-21

## Context
Annotating is the first thing people expect from a PDF editor: notes, highlights, drawing with a pen,
boxes and arrows. Ferrix can show annotations that other apps made (PDFium draws them) but cannot
add, change or remove any.

The constraints:

- **Other readers must show what Ferrix shows.** An annotation is a dictionary on the page
  (`/Annots`). Its look comes from an appearance stream (`/AP`), a small piece of page content.
  Without `/AP`, every reader invents its own look, and some (most mobile readers, Preview for
  some types, PDF/A validators) show nothing at all.
- **Other apps' annotations must survive untouched.** Files arrive with replies, popups,
  rich text, custom keys and types Ferrix does not understand. Saving must not lose or rewrite any of
  it unless the user changed that annotation.
- **Saves stay incremental, atomic and verified** (ADR-0003). Appending keeps existing digital
  signatures intact at the byte level.
- **Undo is exact, and recovery replays without diverging** (ADR-0003).
- **One writer per revision** (ADR-0001). Page organization already saves through lopdf
  (`ferrix-pdf`). Annotation saves must use the same writer, and both kinds of change must be able
  to land in one save.
- **The engine is not yet out of process** (ADR-0004). That is required for the beta, not for
  annotations.
- **Text markup needs text selection.** Highlighting, underlining and striking out are drawn over
  the glyph boxes of selected text. Text selection in PDFs is still an open M1 item.

## Decision
1. **Ferrix owns the annotation model; the PDF is written from it.** We do not mutate
   documents through PDFium's annotation API. `ferrix-core` gains an `annots` module next to
   `PageList`:
   - `Annotation { id: AnnotId, page: PageKey, origin, props, kind }`.
   - `AnnotId` is a session key like `PageKey`. `origin` is `New` or
     `Existing { source: SourceId, object: ObjectId }`. That is the annotation's object in its source
     document, so imported pages keep their annotations editable.
   - `props` holds what every type shares: `rect`, colour (`/C`), interior colour (`/IC`),
     opacity (`/CA`), border width and dashes (`/BS`), author (`/T`), contents (`/Contents`),
     modified date (`/M`), flags (`/F`).
   - `kind` is an enum: `Note { icon }`, `FreeText { text, size, align }`,
     `Ink { strokes }`, `Square`, `Circle`, `Line { from, to, ends }`, `Polygon`/`PolyLine`,
     `Markup { style: Highlight|Underline|StrikeOut|Squiggly, quads }`, `Stamp`, and
     `Other { subtype }` for everything we only display.
   - Geometry is stored in the source page's **default user space** (PDF points, unrotated, origin
     wherever the file put it, not the crop box corner). The view converts for `/Rotate`,
     the crop box and `/UserUnit`. Most hand-rolled annotation code gets this conversion wrong, so it
     lives in `ferrix-types` and has tests.
2. **Annotations hang off `PageKey`, not page numbers.** Moving, rotating or importing pages carries
   their annotations along for free. Removing a page leaves its annotations attached to the removed
   key, so undoing the removal brings them back. Annotations on pages that are not in the final
   list are not written.
3. **Three edits with exact inverses, in the same history as page edits.** `ferrix-core`'s edit
   becomes `DocEdit { Pages(Edit), Annot(AnnotEdit), Batch(Vec<DocEdit>) }`, where
   `AnnotEdit` is `Add { index, annot }`, `Remove { id }` and `Set { annot }`:
   - `Add` and `Remove` invert each other. `Remove` records the annotation's position in
     `/Annots`, because that order is the z-order.
   - `Set` replaces the whole annotation and returns a `Set` holding the previous value. Whole
     snapshots keep the inverse trivially exact.
   - Consecutive `Set`s on one annotation within one gesture (dragging, resizing) merge.
   - The randomized round-trip test covers mixed page and annotation edits.
4. **Foreign annotations are preserved byte for byte unless edited.**
   - On open, `ferrix-annot` reads every page's `/Annots` with lopdf. Types we understand become
     editable `kind`s. Everything else becomes `Other`: listed, selectable and deletable, never
     rewritten.
   - Widgets (form fields), `Link`, `Popup` and anything with the `Locked` flag are not
     offered for editing. Popups follow their parent: deleting a note removes its popup.
   - Editing an existing annotation clones its original dictionary and overwrites only the keys
     we model. Unknown keys (`/IRT`, `/RT`, `/Popup`, `/NM`, vendor keys) and the object number stay
     as they are.
   - `/AP` is regenerated only when a change affects the look. If `/Contents` changes and
     `/RC` (rich text) exists, `/RC` is rewritten as a plain XHTML paragraph of the new text, so
     the two cannot disagree.
   - `LockedContents` makes the text read-only while still allowing the annotation to move.
5. **Every annotation Ferrix writes has an appearance stream, built from one display list.**
   - `ferrix-annot` turns an `Annotation` into an `Appearance`: a list of paths, fills, strokes,
     opacity and blend mode, plus laid-out text for `FreeText`.
   - The same list is drawn as a vector overlay while the user drags or draws (ADR-0003 §2), and is
     written as the `/AP /N` form XObject with a correct `/BBox` and `/Matrix`. What the overlay shows is
     what other readers show.
   - Highlights use `/BM /Multiply` with opacity in an ExtGState, as Acrobat does, so the text
     under them stays black.
   - Notes draw a Ferrix-made icon and set `NoZoom|NoRotate`.
   - `QuadPoints` are written in Acrobat's order (top-left, top-right, bottom-left, bottom-right),
     which is what readers actually expect, and read in either order.
   - **`FreeText` text is laid out by `ferrix-compose` and written through krilla.** krilla
     embeds and subsets the font, so any script works. The resulting page content becomes the
     annotation's form XObject, copied in the way page imports already are. `/DA` names Helvetica
     at the same size and colour, so readers that regenerate the look have something sensible to
     use.
   - New annotations get `/NM` (unique), `/CreationDate`, `/M`, `/P` and the `Print` flag. `/T`
     comes from an "Author name" preference (ADR-0005). It is empty until the user sets it and is
     never filled in silently from the OS account, because files get shared.
6. **Saving is one incremental revision built by `ferrix-pdf`.** `write_page_layout` grows into a
   revision builder that takes the page layout and, per page, the final `/Annots` array plus new or
   replaced objects (annotations, appearance XObjects, fonts). `ferrix-annot` produces those
   objects. `ferrix-pdf` never depends on it, so lopdf stays the only writer. Verification gains
   checks: PDFium reopens the file and sees, on each page, the same number of annotations with the
   same subtypes and rects we wrote.
7. **What is on screen is the revision that would be saved.** PDFium never edits; it only renders
   bytes.
   - After each committed edit, `ferrix-annot` builds the annotation-only update for each affected
     source. The base lopdf document is parsed once per session, when annotations are first edited,
     and kept.
   - PDFium reopens the original bytes followed by that update, through a chained reader with no
     copy (`FPDF_LoadCustomDocument`). Only that document's tiles are re-rendered.
   - In-progress drawing is a vector overlay until it commits.
   - This avoids mapping PDFium's annotation indices to object numbers. PDFium may add generated
     popups to its own list, so its indices are not reliable. It also tests our writer on every
     edit, not just at save time.
8. **Signed, encrypted, PDF/A and Ferrix-written documents:**
   - **Certified documents** (`/Perms /DocMDP` with `P` = 1 or 2) do not allow adding
     annotations. Ferrix shows them read-only with an explanation, instead of producing a file
     whose certification breaks.
   - For other signed files, the signature stays valid. Validators will report "changed after
     signing", and Ferrix says so before the first edit.
   - **Encrypted files:** lopdf 0.46 re-encrypts appended objects with the file's key. A test must
     cover RC4 and AES files before this is relied on.
   - **PDF/A files** (XMP `pdfaid`): what we write already follows PDF/A-2 rules (appearance
     present, `Print` set, no forbidden types). PDF/A-1 forbids transparency, so highlights in an
     A-1 file are written opaque with Multiply blending. Full handling waits for the PDF/A work in
     M4.
   - **Documents written in Ferrix** (ADR-0008) stop opening for writing once they have annotations.
     Ferrix asks before the first annotation is added, rather than silently detaching the source.
9. **Recovery** extends the ADR-0003 snapshot: each journal frame holds the `PageList` and the
   annotations that differ from the file (added, changed, removed). It does not hold every
   annotation in the document. Replay is "load file, apply the snapshot". It refuses, as today, if
   the file changed on disk.
10. **Crates:** model and edits in `ferrix-core` (serde only). A new crate, **`ferrix-annot`**
    (the name `architecture.md` already reserved), holds reading, the display list, appearance and
    object writing; it depends on `ferrix-core`, `ferrix-pdf` and `ferrix-compose`. The tools,
    overlay and panel go in `ferrix-app`. A CLI command `ferrix annotate list|add|remove` exercises
    everything headlessly.
11. **ADR-0004 does not block this work.** Annotation code parses with lopdf in the app process,
    exactly as page-organization saves already do. By the beta, `ferrix-pdf` reads of untrusted
    files move into the engine host along with PDFium. The interfaces above pass plain data
    (`Annotation`, byte buffers), so they can move without redesign.

## Alternatives considered
- **PDFium's annotation API and PDFium save.** This is less code at first. But it means two writers
  for one file (ADR-0001), PDFium's own appearance generation (which differs from what other readers
  draw and from our overlay), weak control over which keys survive, and saves we cannot verify the
  same way. Rejected.
- **Render the committed state ourselves on top of PDFium.** This needs PDFium to hide edited
  foreign annotations by index, which is unreliable (see 7). It also means we draw arbitrary
  appearance streams. Rejected in favour of re-rendering the would-be revision.
- **Write appearances with the standard Helvetica font only.** Simpler, but limited to Latin-1;
  Cyrillic, Greek and CJK notes would break. Rejected for `FreeText`. It is still used for `/DA`,
  which only matters to readers that regenerate the look.

## Consequences
- + Annotations look the same in Ferrix, Acrobat, Preview, browsers and mobile readers, because
  every one we write carries its own appearance.
- + Other apps' annotations and unknown data are never damaged; only what the user touched changes.
- + Page organization and annotations share one history, one journal and one save; both are
  covered by a single round-trip test.
- + The screen shows the exact bytes that would be saved, so writer bugs show up immediately.
- − Each committed edit costs an update build and a PDFium reopen. The update is small, and PDFium
  loads lazily, but documents in the hundreds of megabytes need measuring. Edits are batched while
  a gesture is in progress.
- − The base document stays parsed in memory once annotation editing starts (lopdf parses
  eagerly).
- − **Deleted annotations stay in the file's earlier revision**, as deleted pages already do
  (ADR-0003). Someone with a PDF tool can recover the text of a deleted comment. Until a
  full-rewrite save exists ("Optimize", or redaction in M4), the UI says so when an annotation with
  contents is deleted.
- − Pen pressure is not stored in PDF ink (`/InkList` is a centre line). Pressure, when added, will
  exist only in the appearance and be lost if another app regenerates it.
- − Not covered: replies and review states, file attachments, sound and movie, 3D, redaction
  annotations (M4), measurement annotations, and annotation structure elements in tagged PDFs
  (phase 4).
- − `ferrix-annot` depends on `ferrix-compose` (for `FreeText`), which pulls cosmic-text and krilla
  into anything that writes annotations, the CLI included. Both are already in the app.

## Phases
1. **Headless core.**
   - `ferrix-core` annotation model and `DocEdit`, with randomized round-trip tests over mixed edits.
   - `ferrix-annot`: reading, appearances and writing for Note, Ink, Square, Circle, Line/Arrow.
   - The `ferrix-pdf` revision builder and `ferrix annotate`.
   - Each type round-trips through PDFium and `qpdf --check`, including encrypted, signed and
     rotated-page cases.
2. **Editing in the app.**
   - Tools (note, pen, rectangle, ellipse, line, arrow), select/move/resize/delete, colour, opacity
     and width.
   - An annotation list panel (keyboard-navigable), a note editor, rendering of the would-be
     revision, undo/redo and recovery.
   - Annotating other apps' annotations: move, recolour, edit text, delete.
3. **Text.** `FreeText` through ferrix-compose and krilla. Highlight, underline, strike-out and
   squiggly once text selection in PDFs exists (M1).
4. **Polish.**
   - Stamps (standard and image; the same path serves visual signatures in M3), replies and
     popups.
   - Structure elements for annotations in tagged PDFs, and pen pressure.

## Implementation notes (phase 1, headless core, 2026-11-21)
- **Model** (`ferrix-core`): `annots.rs` holds `Annotation`, `AnnotSet` and `AnnotEdit`;
  `state.rs` holds `DocState`/`DocEdit`. `History` is now generic over an `Undoable` state
  (default `PageList`, so the app is unchanged) and gains `perform_grouped`, which merges a
  gesture's `Set`s (and an `Add` followed by `Set`s) into one undo step, never across a save.
- **Annotations of removed pages are frozen.** The randomized round-trip test found that
  deleting an annotation of a removed page produced an inverse `Add` that could not apply.
  `DocState` now refuses `Add`, `Remove` and `Set` on annotations whose page is not in the list.
- `Other` annotations can be deleted but never `Set`, and Ferrix can't create one. Replies
  (`/IRT`) read as `Other` until phase 4. Popups, widgets and links are not loaded at all; they
  stay in `/Annots` untouched.
- **Reading** (`ferrix-annot::read`) never fails on a malformed annotation: missing or invalid
  geometry, or more than `MAX_STROKES`/`MAX_STROKE_POINTS`, makes it `Other`. Colours accept
  gray, RGB and CMYK; text strings decode PDFDocEncoding, UTF-16BE and UTF-8.
- **Writing** (`ferrix-annot::write`): a page is touched only if something on it changed.
  Its new `/Annots` keeps the original entries in order, minus deleted annotations and their
  popups, then appends new annotations. Z-order changes to existing annotations are not
  written yet. Whether an existing annotation changed is decided by re-reading the original and
  comparing models, so no baseline copy is kept.
- **`ferrix-pdf::Revision`** replaces the body of `write_page_layout` (now a thin wrapper). It
  maps imported annotation objects through lopdf's renumbering by pairing each page's `/Annots`
  references before and after. Without a layout, only pages with changed annotations get a new
  dictionary. A certified base (`DocMDP` P=1 or 2) makes `write_annotations` fail with
  `NotAllowed`.
- **Bug fixed on the way:** `Revision::finish` (and so every page-organization save) verified
  its output without the document's password, so saves of password-protected files always
  failed with "output has 0 pages". It now verifies with the password; lopdf re-encrypts the
  appended objects, which is tested for RC4-128 and AES-128.
- **Engine:** `PdfEngine::annotations` returns PDFium's view (`AnnotSummary`: subtype and
  rect). `ferrix annotate add|remove` refuses to write unless PDFium sees exactly the intended
  subtypes on every page.
- **Not yet:** `ferrix-compose` is not a dependency yet (it arrives with `FreeText` in phase
  3); the note icon is the same bubble for every `/Name`; the preview-revision rendering
  (decision 7) and recovery snapshots (decision 9) land with the app in phase 2.
- **Edit-model audit (same day)** found four blocking problems, now fixed with regression tests:
  - A `Set` could change an annotation's type (to `Other`), so its inverse was refused. `Set`
    now keeps the type.
  - A failed `Add` left an empty page entry behind.
  - `Add` moved the id counter, so inverses were not exact. Ids are now reserved with
    `AnnotSet::new_id` outside the history (like page keys), and `Add` refuses unreserved ids,
    which also rules out the `u64::MAX` overflow.
  - Annotations loaded from files were not validated, so undoing a deletion could be refused.
    Loading now sanitizes them (`Annotation::sanitized`), turning whatever can't be kept into
    `Other`.

  Validation now also bounds coordinates (±1e6 pt), refuses all-zero dash patterns and invalid
  icon names, and caps ink at 1 million points. For annotations from the file, the model
  enforces `Locked` and `LockedContents`, and no edit may change those flags. The randomized
  test compares the whole state after failed edits and after undoing everything.
- **For phase 2 recovery:** journal snapshots should identify file annotations by their
  `Existing` object, not by `AnnotId`, which depends on load order, and should be validated on
  replay, because a journal is untrusted input.
- **PDF review (same day)** found three blocking problems, now fixed with tests:
  - **Broken references in imported files** (`Revision::new`): lopdf renumbers only objects
    that exist, so a reference to a missing object kept its number and then pointed at an
    unrelated object, such as the import's catalog. Missing references are turned into `null`
    before renumbering, as the spec says they read. This also affected page imports before
    annotations.
  - **Replies left behind:** deleting a note left replies whose `/IRT` pointed at an
    annotation no longer on the page. `Annotation::reply_to` records `/IRT`, and
    `AnnotSet::thread` lists a note with all its replies, which callers delete in one `Batch`.
    The writer refuses to orphan a reply.
  - **PDF/A:**
    - For files claiming PDF/A with a CMYK output intent, appearances are written in
      DeviceCMYK.
    - PDF/A-1 files refuse annotations below full opacity, with a message. This replaces
      decision 8's "write highlights opaque", which would hide the text.
- **Also from the review:**
  - Below full opacity, shapes are drawn opaque in an inner transparency-group form, painted at
    the given opacity. Overlaps (border on fill, crossing ink, arrowheads) no longer come out
    darker, and PDFium and Poppler now agree. `/CA` stays in the dictionary as well. ISO
    32000-1 Table 170 says readers ignore it when there is an appearance; it is there for
    readers that rebuild the appearance.
  - Lines with leader lines, captions, intents or measurements, and cloudy-bordered shapes,
    read as `Other`. `/RD` is applied on read, so an edited shape keeps its place.
  - When the text of a note changes, `/RC` is removed instead of rewritten (this supersedes
    decision 4's rewrite): Acrobat's XFA-flavoured rich text isn't worth rebuilding. Stale
    `/ca` and `/BM` are removed or updated, and notes get no `/BS`.
  - Notes get `NoZoom|NoRotate` from `Annotation::new`, not only from the CLI.
  - `AnnotSet` records which pages were loaded, so a page that was never read can't lose its
    annotations.
  - Reader-extended files (`/Perms /UR3`) count as signed for the warning.
  - `ferrix_annot::verify` re-reads the output with lopdf and checks each page's subtypes; the
    CLI then also checks with PDFium.
- **Still open:** annotations are not added to the structure tree of tagged PDFs (phase 4), so
  the app should say so when annotating a tagged file.
