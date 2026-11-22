# Ferrix

A fast, safe, open-source PDF reader and editor for macOS, Windows and Linux, written in Rust.

> **Status: pre-alpha.** Not ready for everyday use. Milestone M1 (viewer MVP) is in progress;
> see the [roadmap](docs/roadmap.md).

What works today:
- **Desktop app** (`ferrix-app`): open, view and navigate PDFs with zoom (also ⌘/Ctrl + wheel),
  fit width/page, continuous, single-page and two-page layouts, thumbnails, the outline, links
  with Back/Forward, password-protected files, light/dark themes and Dark Pages for reading at
  night
- **Remembers your place** ([ADR-0005](docs/adr/0005-settings-and-state-storage.md)): Open
  Recent, each document's last page, zoom and layout, and your preferences
- **Page organization**: reorder, rotate, delete and insert pages from other files, with
  undo/redo and safe incremental save / Save As
- **Text selection**: select text on PDF pages (drag, double-click a word, triple-click a
  line) and copy it
- **Find in PDFs** (⌘F): results stream in as pages are searched; case- and accent-insensitive
  unless asked, line breaks and end-of-line hyphens ignored; ⌘G / ⇧⌘G step through matches
- **Annotations** ([ADR-0009](docs/adr/0009-annotations.md)): Markup mode (⇧⌘A) adds notes,
  text boxes, highlights, underlines, strikethrough, freehand ink, rectangles, ellipses, lines,
  arrows and stamps (standard or a picture); select, move, resize, recolour and delete them,
  including other apps' annotations; reply to any annotation in a comment thread; a Comments
  list; saved as standard PDF annotations that look the same in every reader, and added to
  the accessibility tags of tagged PDFs
- **Crash recovery**: unsaved edits are journaled and offered back after a crash
- **Printing** through the operating system's print dialog
- **Creating PDFs**: blank documents, or converted from images, Word, Excel, PowerPoint and
  OpenDocument files (Office formats need LibreOffice installed)
- **CLI** (`ferrix`): `info`, `render`, `text`, `outline`, `organize`, `annotate` (list, add
  and remove notes, shapes, lines, arrows and ink), and `compose` / `source` for documents
  written from scratch
- **Writing documents** ([ADR-0008](docs/adr/0008-writing-documents.md)): type and format
  letters and reports (styles, fonts, bold/italic/underline, colour, alignment, spacing,
  bulleted and numbered lists, tables, pictures, page breaks, headers and footers with page
  numbers, find and replace, formatted copy and paste), saved as tagged, accessible PDFs (or
  PDF/A-3 for archiving) that stay editable in Ferrix

Not yet: forms, the sandboxed engine process, and spell check in written
documents.

## Goals
- Fast, faithful rendering of real-world PDFs
- Everyday editing: annotate, fill & sign forms, organize pages, redact, OCR
- Create PDFs: blank documents, or from Word, Excel, PowerPoint and image files
- Never corrupt your files; recover from crashes
- Safe with untrusted documents: no PDF JavaScript, sandboxed engine
- Accessible, keyboard-friendly, localizable; no telemetry by default

## Design docs
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Architecture Decision Records](docs/adr/)

## Building

```sh
cargo xtask fetch-pdfium
cargo run -p ferrix-app
cargo xtask bundle --open    # macOS: build and launch Ferrix.app (named “Ferrix” in the menu bar)
cargo run -p ferrix-cli -- info document.pdf
cargo run -p ferrix-cli -- render document.pdf --pages 1-3 --dpi 144 -o out/
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

On Linux, run `./install.sh` from an unpacked release to add Ferrix to your applications menu
and offer it for PDFs (it installs into `~/.local`; `./install.sh --uninstall` removes it).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

The bundled Liberation fonts in `crates/ferrix-compose/fonts/` are under the
[SIL Open Font License 1.1](crates/ferrix-compose/fonts/LICENSE-OFL).
