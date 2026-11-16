# Ferrix — Architecture

Status: **Draft v0.1** · Owner: core team · Last updated: 2026-11-15

Ferrix is an open-source, cross-platform desktop PDF reader and editor written in Rust.
This document describes the target architecture. Significant decisions are recorded as
ADRs in [`docs/adr/`](adr/); the delivery plan is in [`roadmap.md`](roadmap.md).

---

## 1. Goals and non-goals

### Product goals
1. **Fast, faithful reading.** Opening a 1,000-page PDF shows page 1 in < 300 ms; scrolling and
   zooming stay at display refresh rate; rendering matches Chrome/Acrobat on real-world files.
2. **Everyday editing without Acrobat.** Annotate, fill and sign forms, organize pages
   (reorder/rotate/insert/delete/merge/split), redact, OCR. Later: edit existing text and images.
3. **Never corrupt a user's file.** Edits are non-destructive until saved, saves are atomic,
   incremental saves preserve existing digital signatures, and crashes are recoverable.
4. **Safe with hostile input.** PDFs are untrusted. A malformed file must not crash the app
   or execute code.
5. **Accessible and international.** Screen-reader support, full keyboard operation, RTL/CJK
   text, localizable UI.
6. **Cross-platform, native-feeling.** macOS, Windows and Linux are equally important: every
   platform is tier 1 and gates every merge in CI.

### Non-goals (for 1.0)
- PDF JavaScript execution (forms work without it; scripts are ignored, see §9).
- Cloud sync, accounts, telemetry by default.
- A full office suite. Ferrix writes letters, reports and forms ([ADR-0008](adr/0008-writing-documents.md)),
  but not tracked changes, footnotes, mail merge or `.docx` editing.
- Mobile / web targets (the core stays portable so this remains possible later).

### Quality attributes (prioritized)
Correctness of output > Security > Responsiveness > Rendering fidelity > Feature breadth > Binary size.

---

## 2. System context

```
           ┌──────────────────────────────────────────────┐
  user ───▶│                Ferrix desktop app             │───▶ printer (OS print APIs)
           │                                              │
  files ◀─▶│  UI shell ── App core ── Engine host process │───▶ OS keychain / cert store (signing)
           │                                              │
           └──────────────────────────────────────────────┘
                 ▲                                 ▲
        OS integration                     optional, user-initiated:
  (file assoc., drag&drop,             OCR language packs, update check
   recent docs, a11y APIs)
```

No network access is required for any core feature.

---

## 3. Architectural principles

1. **Headless core, thin UI.** All behavior lives in UI-agnostic crates; the UI renders state and
   dispatches intents. The core is exercised directly by a CLI (`ferrix-cli`) and tests.
2. **The PDF engine is a replaceable backend.** All engine access goes through the
   `ferrix-engine` trait boundary using our own types; no engine types leak upward ([ADR-0001](adr/0001-pdf-engine.md)).
3. **Untrusted parsing happens out of process.** The engine runs in a sandboxed worker process
   that can crash without taking the app down ([ADR-0004](adr/0004-engine-process-isolation.md)).
4. **Every edit is a command.** Mutations are typed, serializable commands with inverses →
   undo/redo, autosave journal, macro/batch, and testability for free ([ADR-0003](adr/0003-edit-and-save-model.md)).
5. **Never block the UI thread.** Rendering, parsing, search, save and OCR are async jobs with
   cancellation and priorities.
6. **Explicit memory budgets.** Caches are bounded by bytes, not item counts.

---

## 4. Logical architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Presentation        ferrix-app (iced, ADR-0002)                          │
│                      windows/tabs, toolbars, sidebars, dialogs,           │
│                      DocumentCanvas widget (GPU textured tiles + overlays)│
├──────────────────────────────────────────────────────────────────────────┤
│  Application         ferrix-core                                          │
│                      Workspace ─ DocumentSession ─ CommandStack (undo)    │
│                      ViewState (layout, zoom, selection, tool mode)       │
│                      Tools (select, highlight, ink, shape, text, redact)  │
├───────────────┬───────────────┬───────────────┬──────────────────────────┤
│ ferrix-render │ ferrix-text   │ ferrix-io     │ ferrix-annot / -forms /  │
│ layout, tiles,│ selection,    │ open, atomic  │ -sign / -ocr (features)  │
│ scheduler,    │ search index, │ save, journal,│                          │
│ caches        │ extraction    │ recovery      │                          │
├───────────────┴───────────────┴───────────────┴──────────────────────────┤
│  Engine boundary     ferrix-engine (traits + DTOs, IPC protocol)          │
├──────────────────────────────────────────────────────────────────────────┤
│  Engine backends     ferrix-engine-pdfium   (v1, in engine host process)   │
│                      ferrix-engine-hayro    (experimental, pure Rust)      │
│                      ferrix-pdf (lopdf-based object layer: incremental     │
│                      writer, outline/metadata/structure edits)             │
└──────────────────────────────────────────────────────────────────────────┘
```

### 4.1 Cargo workspace

```
ferrix/
├─ Cargo.toml                 # workspace, shared lints, [workspace.dependencies]
├─ crates/
│  ├─ ferrix-types/           # geometry (PageRect, Matrix), ids, colors, units — no deps
│  ├─ ferrix-engine/          # PdfEngine trait, DTOs, IPC protocol (serde + postcard)
│  ├─ ferrix-engine-pdfium/   # PDFium backend via pdfium-render
│  ├─ ferrix-engine-host/     # bin: sandboxed worker process hosting a backend
│  ├─ ferrix-pdf/             # object-level PDF read/write, incremental update writer
│  ├─ ferrix-render/          # page layout, tile grid, render scheduler, caches
│  ├─ ferrix-text/            # text model, selection, search, extraction
│  ├─ ferrix-core/            # sessions, commands, undo, tools, view state, events
│  ├─ ferrix-io/              # file I/O, atomic save, autosave journal, recovery
│  ├─ ferrix-store/           # settings.json + SQLite state.db (ADR-0005)
│  ├─ ferrix-print/           # native print dialogs per platform (ADR-0006)
│  ├─ ferrix-convert/         # images + office documents → PDF via Word/LibreOffice (ADR-0007)
│  ├─ ferrix-compose/         # documents written from scratch: model, edits, layout, export (ADR-0008)
│  ├─ ferrix-app/             # bin: desktop UI
│  └─ ferrix-cli/             # bin: headless ops (render, merge, split, annotate, diff)
├─ xtask/                     # cargo xtask: fetch-pdfium, corpus, bundle, icns, release
├─ tests/corpus/              # manifest of test PDFs (files fetched, not committed)
├─ fuzz/                      # cargo-fuzz targets
└─ docs/
```

Dependency rule: arrows point **down only**. `ferrix-core` must not depend on any UI crate or a
concrete engine backend. Enforced by `cargo-deny` bans + a CI check on `cargo metadata`.

---

## 5. Key components

### 5.1 Engine boundary (`ferrix-engine`)

A narrow, coarse-grained, serializable API (it crosses a process boundary):

```rust
pub trait PdfEngine: Send {
    fn open(&mut self, src: DocSource, password: Option<Secret>) -> Result<DocInfo, EngineError>;
    fn close(&mut self, doc: DocId);

    fn page_info(&mut self, doc: DocId, range: PageRange) -> Result<Vec<PageInfo>, EngineError>;
    fn render_tile(&mut self, req: TileRequest) -> Result<RenderedTile, EngineError>;
    fn text_page(&mut self, doc: DocId, page: PageIndex) -> Result<TextPage, EngineError>;
    fn outline(&mut self, doc: DocId) -> Result<Vec<OutlineItem>, EngineError>;
    fn links(&mut self, doc: DocId, page: PageIndex) -> Result<Vec<Link>, EngineError>;
    fn annotations(&mut self, doc: DocId, page: PageIndex) -> Result<Vec<Annotation>, EngineError>;
    fn form_fields(&mut self, doc: DocId) -> Result<Vec<FormField>, EngineError>;

    fn apply(&mut self, doc: DocId, edits: &[EngineEdit]) -> Result<ApplyOutcome, EngineError>;
    fn save(&mut self, doc: DocId, mode: SaveMode, sink: SaveTarget) -> Result<SaveReport, EngineError>;

    fn capabilities(&self) -> EngineCaps;   // lets the UI hide unsupported features
}
```

- `TextPage` is our own model (glyph quads, chars, reading-order runs, font info) so selection,
  search and a11y are engine-independent.
- `Annotation`, `FormField`, `EngineEdit` are Ferrix types mapped from/to engine specifics.
- The **client side** (`EngineClient`) is async: `async fn render_tile(..) -> Result<..>`, with
  request priority and cancellation tokens; it multiplexes requests over IPC to N host processes.

### 5.2 Engine host & process model

```
 ferrix-app (UI process)                          ferrix-engine-host × N (sandboxed)
 ┌───────────────────────┐   IPC: length-prefixed  ┌───────────────────────────┐
 │ EngineClient (pool)   │◀── postcard frames ───▶ │ backend (PDFium) on 1 thrd│
 │  • priority queue     │   + shared memory for   │ owns open docs (by DocId) │
 │  • cancellation       │     tile pixel buffers   │ no network, no fs write*  │
 │  • crash → respawn,   │                          └───────────────────────────┘
 │    reopen, retry once │      *saves stream bytes back to the app process
 └───────────────────────┘
```

- PDFium is not thread-safe; parallelism comes from **multiple processes** (default
  `min(4, cores/2)`), each with the document opened read-only. One "primary" host per document owns
  the editable state; secondary hosts serve render/text requests and reload after commits.
- Pixel data returns via shared memory (`shared_memory`/`memmap2`) to avoid copying large tiles.
- An in-process transport exists for tests, the CLI, and platforms where spawning is restricted.
- Sandbox: macOS `sandbox_init`/App Sandbox profile, Windows AppContainer/job objects,
  Linux seccomp-bpf + namespaces (landlock where available). Phased in; see roadmap.

### 5.3 Rendering pipeline (`ferrix-render`)

```
ViewState (scroll, zoom, rotation, layout mode)
   │
   ▼
Layout ──▶ visible page rects ──▶ TileGrid (256×256 device px tiles per zoom level)
   │                                   │
   │                       needed tiles ▼
   │                         RenderScheduler
   │     priority: visible > near-viewport prefetch > thumbnails > background index
   │     generation counter: requests from stale viewports are cancelled
   │                                   │
   ▼                                   ▼
Canvas draws, per page:  [best available cached tile, scaled] → [exact tile when ready]
                         + overlay layer (selection, search hits, live annotation drafts, cursors)
```

- **Progressive rendering:** low-res whole-page image first (cheap, also used for thumbnails),
  then crisp tiles. Zooming uses scaled existing tiles until exact ones arrive — no blank frames.
- **Caches:** `TileCache` (GPU textures, LRU, budget e.g. 512 MB VRAM-aware), `PageImageCache`
  (CPU, compressed), `TextPageCache`. All bytes-bounded via a shared `MemoryBudget`.
- **Overlays are vector-drawn by us**, not baked by the engine: annotations being edited, ink
  strokes in progress, selection highlights. Only committed changes trigger engine re-render
  of the affected page region.
- Color management: render in sRGB v1; ICC/output-intent aware later.
- High-DPI: tiles keyed by device-pixel scale; per-monitor DPI changes invalidate by key.

### 5.4 Document session & command model (`ferrix-core`)

```rust
pub struct DocumentSession {
    id: DocId,
    origin: FileOrigin,          // path, fingerprint (size+mtime+hash of head/tail), bookmark
    base_revision: Revision,     // what's on disk
    stack: CommandStack,         // applied commands, undo/redo cursor
    pending: Vec<EngineEdit>,    // not yet pushed to the engine (batched)
    view: ViewState,
    caps: EngineCaps,
}

pub trait Command: Serialize + DeserializeOwned {
    fn apply(&self, ctx: &mut EditCtx) -> Result<Applied, EditError>;   // returns inverse
    fn label(&self) -> Cow<'static, str>;                                  // "Delete page 4"
    fn merge(&mut self, next: &Self) -> bool { false }                     // coalesce ink/typing
}
```

- Commands: `AddAnnotation`, `UpdateAnnotation`, `DeleteAnnotation`, `SetFieldValue`,
  `MovePages`, `RotatePages`, `DeletePages`, `InsertPages{from: DocSource}`, `Redact`,
  `SetMetadata`, `EditOutline`, … — each maps to one or more `EngineEdit`s.
- Undo = apply inverse. Commands are journaled to disk (§5.6) for crash recovery.
- `Workspace` holds sessions and routes events; UI subscribes to a typed `Event` stream
  (`PagesChanged{range}`, `SelectionChanged`, `DirtyChanged`, `SaveProgress`, …).

### 5.5 Text, selection, search (`ferrix-text`)

- Per-page `TextPage` with glyph quads → selection by glyph index ranges, rendered as quads
  (handles rotation/skew), copy with reading-order heuristics and ligature/hyphen normalization.
- Search: background indexer extracts text page-by-page (low priority), builds an in-memory index
  (Unicode NFKC + case folding, diacritic-insensitive option). Results stream in as found.
- Also feeds **accessibility**: tagged PDF structure tree when present, otherwise heuristic
  reading order → exposed as an AccessKit subtree of the canvas.

### 5.6 I/O, saving, recovery (`ferrix-io`)

- **Open:** memory-map or stream; never hold an exclusive lock; detect external modification
  via fingerprint and offer reload.
- **Save modes:** `Incremental` (default — append an update section; preserves signatures,
  fast), `Full` (rewrite/compact, required after redaction to purge content), `SaveAs`.
- **Atomic save:** write to temp file in the same directory → `fsync` → atomic rename
  (`ReplaceFileW` on Windows to keep ACLs/attributes) → keep previous version in `.bak` until success.
- **Verify after save:** reopen the written file in a host process and check page count/xref
  before replacing the original.
- **Autosave journal:** commands appended to `~/.local/state/ferrix/journal/<doc-hash>.log`
  (platform equivalents via `directories`). On startup, unclean sessions offer recovery by
  replaying commands onto the original file.

### 5.7 Feature modules

| Module | Approach |
|---|---|
| Annotations | PDF-native annotation dictionaries (Highlight, Underline, StrikeOut, Squiggly, Text/Note, FreeText, Ink, Square, Circle, Line, Polygon, Stamp). Always generate appearance streams so other viewers render them identically. |
| Forms (AcroForm) | Fill, tab order, validation per field flags; formatting via a small built-in subset (date/number masks), no JS. XFA: display static fallback only. |
| Page organizer | Thumbnail grid with drag-reorder, multi-select, insert from file, extract, split by ranges/bookmarks. |
| Redaction | Mark → review → apply. Applying removes underlying text/image/vector content (not just a black box), scrubs metadata, forces Full save. Verified by re-extracting text in tests. |
| Signatures | Visual signatures (image/ink) v1. Digital PAdES (B-B/B-T) via RustCrypto/`cms` + OS cert stores later; validation of existing signatures with clear trust UI. |
| OCR | `ocrs` (pure Rust) default, optional Tesseract; adds invisible text layer. Runs as background job in a separate process. |
| Content editing | Last: object-level edit of text runs/images using engine page-object APIs; font subset handling is the hard part. |

---

## 6. Concurrency model

| Context | Runs | Notes |
|---|---|---|
| UI thread | event loop, layout, GPU submission | never does I/O or parsing |
| Core runtime | `tokio` multi-thread (small) | scheduling, IPC, file I/O, journal |
| Engine hosts | separate processes, single-threaded each | PDFium calls serialized per process |
| CPU jobs | `rayon` pool | search indexing, thumbnail downscale, image decode |

Communication via channels (`tokio::sync::mpsc`, `watch` for view state). Every job carries a
`CancellationToken` and a generation number; stale results are dropped, not rendered.

---

## 7. Cross-cutting concerns

- **Errors:** `thiserror` in libraries, `anyhow` only in bins. User-facing errors carry a stable
  code and a localized message; never surface raw engine strings.
- **Logging/diagnostics:** `tracing` with a rolling local file; "Export diagnostics" bundles logs
  without document contents. No telemetry by default; any future telemetry is opt-in.
- **Config & state:** preferences in `settings.json`, per-document state and history in SQLite
  (`state.db`) behind a `ferrix-store` crate — see [ADR-0005](adr/0005-settings-and-state-storage.md) (proposed).
- **i18n:** Fluent (`fluent-bundle`) for UI strings; ICU4X for segmentation/collation in search.
- **Accessibility:** AccessKit for UI chrome and the document canvas subtree; WCAG 2.2 AA
  contrast; every action reachable by keyboard; high-contrast & reduced-motion modes.
- **Theming:** light/dark/system; optional "invert page colors" reading mode done in the
  compositor shader, not by re-rendering.

---

## 8. Quality strategy

| Layer | Technique |
|---|---|
| Unit | Standard `#[test]`, `insta` snapshots for DTOs and layout math |
| Property | `proptest`: command apply/undo is identity; save→reopen preserves model; layout invariants |
| Corpus | Curated public PDFs (pdf.js test suite, PDFium testing corpus, veraPDF/Isartor, GovDocs1 sample) listed in `tests/corpus/manifest.toml` with checksums |
| Visual regression | Render corpus pages via `ferrix-cli render`, compare with perceptual diff (threshold), golden images in a separate LFS/artifact store |
| Round-trip | Each edit command: apply → save (incremental & full) → `qpdf --check` → reopen in PDFium and hayro → assert content |
| Fuzzing | `cargo-fuzz` on `ferrix-pdf` parser/writer and IPC decoding; OSS-Fuzz once public |
| UI | Core is tested headless; a small set of end-to-end smoke tests drive the app via AccessKit |
| Performance | `criterion` benches + CI perf budget on: open time, first paint, tile render p95, search throughput, RSS |

CI: GitHub Actions matrix (macOS arm64/x64, Windows x64/arm64, Ubuntu), `clippy -D warnings`,
`rustfmt`, `cargo-deny` (licenses/advisories), `cargo-semver-checks` for library crates, MSRV pin.

---

## 9. Security model

Threat: a crafted PDF attempts code execution, data exfiltration, or resource exhaustion.

- Parsing/rendering only in sandboxed engine hosts (no network, read-only access to the opened
  file handle, memory/time limits per request; watchdog kills hung renders).
- **No PDF JavaScript execution.** Launch actions, embedded file "open" actions and `URI` actions
  require explicit user confirmation and show the target.
- Embedded files are extracted only on request, never executed.
- Remote resources (e.g. remote GoTo) are never fetched automatically.
- Safe Rust in all first-party crates (`#![forbid(unsafe_code)]` except FFI/IPC shims which are
  reviewed and fuzzed).
- Supply chain: `cargo-deny`, `cargo-vet` (or `cargo-crev`) for new deps, reproducible release
  builds, signed artifacts, SBOM published with releases.

---

## 10. Packaging & distribution

- Build/package with `cargo-packager` (or `cargo-dist`) via `cargo xtask bundle`.
- **macOS:** universal `.app` in signed, notarized `.dmg`; Homebrew cask.
- **Windows:** MSIX + MSI, Authenticode signed; winget.
- **Linux:** Flatpak (Flathub) primary, AppImage secondary, `.deb`/`.rpm` from CI.
  Today the release `.tar.gz` carries `install.sh`, which puts the app, its desktop entry
  (`dist/linux/io.github.sbrosgol.ferrix.desktop`) and icon under `~/.local`; the window's
  app id is the same reverse-DNS name, so desktops match windows to the entry.
- PDFium shipped as a pinned prebuilt dynamic library (bblanchon/pdfium-binaries `chromium/8125`,
  matching the `pdfium_8125` feature of `pdfium-render`; bump both together), SHA-256-verified
  by `xtask fetch-pdfium`, loaded at runtime from the bundle.
- Updates: platform-native channels; in-app update *check* is opt-in.

---

## 11. Open questions

1. ~~Project license~~ — **decided: `MIT OR Apache-2.0`**.
2. ~~UI toolkit~~ — **decided: iced** (ADR-0002); accessibility tracked as the top risk.
3. Minimum OS versions (proposal: macOS 12+, Windows 10 22H2+, glibc 2.31+ / Flatpak runtime).
4. Do we need XFA dynamic forms at all? (Proposal: no; show a clear notice.)
5. Tabs vs. one window per document vs. both (proposal: tabs + "move to new window").
