# Ferrix

Open-source PDF reader and editor for macOS, Windows and Linux, in Rust (edition 2024, MSRV in
`Cargo.toml`). Design lives in `docs/architecture.md`, `docs/roadmap.md` and `docs/adr/`; read the
relevant ADR before changing a subsystem, and add implementation notes to it when you change one.

## Commands

```sh
cargo xtask fetch-pdfium                 # once per checkout: PDFium into vendor/pdfium (git-ignored)
cargo run -p ferrix-app [-- file.pdf]    # desktop app
cargo xtask bundle [--release] [--open]  # macOS: target/<profile>/Ferrix.app, named “Ferrix” in the menu bar
cargo run -p ferrix-cli -- --help        # `ferrix`: info, render, text, outline, organize, compose, source
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace
```

PDFium-backed tests skip locally when PDFium is missing (they print `skipping:` only with
`--nocapture`) and fail on CI (`CI` set). Run with PDFium present before claiming tests pass.
The only expected warning is the `block v0.1.6` future-incompatibility note from iced → wgpu → metal.

## Workspace

| Crate | Role |
|---|---|
| `ferrix-types` | geometry, ids |
| `ferrix-engine` / `-pdfium` | `PdfEngine` trait, PDFium backend (rendering, text) |
| `ferrix-pdf` | lopdf object layer: incremental page-layout saves, blank/image PDFs, embedded files, content digest |
| `ferrix-core` | page-organization edit model (`PageList`, `Edit`, `History`) |
| `ferrix-io` | atomic writes, `FileStamp`, crash-recovery journal frames, `app_data_dir` (`FERRIX_DATA_DIR` overrides) |
| `ferrix-print` | native print dialogs; the only crate with `unsafe` (confined platform modules) |
| `ferrix-convert` | images and Office files → PDF via Word/LibreOffice |
| `ferrix-compose` | written documents (ADR-0008): model, edits, layout, raster, export, HTML clipboard, `Editor` |
| `ferrix-app` | iced desktop app; `writer*.rs` is the document editor |
| `ferrix-cli` | headless CLI |

## Code map

Written documents (`ferrix-compose`):
- Model: [`Document`](crates/ferrix-compose/src/model.rs:10), [`Paragraph`](crates/ferrix-compose/src/model.rs:269) (runs, image, list, page break, table `Cell`)
- Edits: [`Edit`](crates/ferrix-compose/src/edit.rs:78) and [`replace`](crates/ferrix-compose/src/edit.rs:218), the one primitive whose inverse is exact
- Editor state, undo merging, tables: [`Editor`](crates/ferrix-compose/src/editor.rs:137), [`commit`](crates/ferrix-compose/src/editor.rs:336), [`table_op`](crates/ferrix-compose/src/editor.rs:1112)
- Layout: [`Layout::cached`](crates/ferrix-compose/src/layout.rs:233), [`break_lines`](crates/ferrix-compose/src/layout.rs:502), [`paginate`](crates/ferrix-compose/src/layout.rs:800)
- Pixels and PDF: [`Rasterizer::page`](crates/ferrix-compose/src/raster.rs:59), [`export_with`](crates/ferrix-compose/src/export.rs:78), [tag tree](crates/ferrix-compose/src/export.rs:309), [`open_pdf`](crates/ferrix-compose/src/export.rs:638)
- Clipboard HTML: [`fragment_from_html`](crates/ferrix-compose/src/html.rs:163)

App (`ferrix-app`):
- [`App::update`](crates/ferrix-app/src/app.rs:520); writer actions [`App::write`](crates/ferrix-app/src/app.rs:1579), [`save_writer`](crates/ferrix-app/src/app.rs:1767), [`check_writing`](crates/ferrix-app/src/app.rs:1559)
- Keys → commands: [`key_command`](crates/ferrix-app/src/writer.rs:213); page images: [`Writer::refresh`](crates/ferrix-app/src/writer.rs:99)
- Input/caret overlay widget: [`Canvas::update`](crates/ferrix-app/src/writer_canvas.rs:172); views: [`writer_view`](crates/ferrix-app/src/view.rs:822), [`format_bar`](crates/ferrix-app/src/view.rs:893)
- Recovery: [`WriterJournal`](crates/ferrix-app/src/recovery.rs:172); clipboard: [`RichClipboard`](crates/ferrix-app/src/clipboard.rs:17)

PDF viewing/organizing: [`PdfEngine`](crates/ferrix-engine/src/lib.rs:24), [`write_page_layout`](crates/ferrix-pdf/src/layout.rs:30), [`content_digest`](crates/ferrix-pdf/src/inspect.rs:89), [`PageList::apply`](crates/ferrix-core/src/pages.rs:159), [`write_atomic`](crates/ferrix-io/src/lib.rs:35).

Line numbers drift; search by name if one is off.

## Conventions

- **Headless first.** Behaviour lives in UI-free crates with unit tests; `ferrix-app` turns input into calls and draws state.
- **Every document change is an edit with an exact inverse.** In `ferrix-compose` that is `Edit::Replace` (or `Batch` for multi-part operations like Replace All); structural changes (tables) rebuild the affected paragraphs in one `Replace`. Keep the randomized round-trip tests passing.
- **Never corrupt files.** Saves are atomic (`ferrix_io::write_atomic`), verified, and refused if the file changed on disk since it was opened (`FileStamp`).
- **Licences:** permissive only (see `deny.toml`). Check new dependency trees before adding them.
- `unsafe` is denied workspace-wide; only `ferrix-print`'s platform modules opt out.
- Comments explain why; user-facing strings use curly quotes and plain language.
- Lints: `clippy::all`, `unwrap_used`, `unreachable_pub`, `missing_debug_implementations` are warnings and must stay clean.

## Testing UI changes

Use the headless screenshot example (`cargo run -p ferrix-app --example screenshot -- out.png …`)
and look at the image; see `.claude/skills/ferrix-screenshot`. Always set `FERRIX_DATA_DIR` to a
scratch directory: the real recovery directory may hold the user's crash journals.

## Git

Feature work goes on a branch off `dev`; `main` is the release branch. Don't push or open PRs
unless asked.
