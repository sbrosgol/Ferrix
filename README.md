# Ferrix

A fast, safe, open-source PDF reader and editor for macOS, Windows and Linux, written in Rust.

> **Status: pre-alpha.** Not ready for everyday use. Milestone M1 (viewer MVP) is in progress;
> see the [roadmap](docs/roadmap.md).

What works today:
- **Desktop app** (`ferrix-app`): open, view and navigate PDFs with zoom, fit width/page,
  thumbnails and light/dark themes
- **Page organization**: reorder, rotate, delete and insert pages from other files, with
  undo/redo and safe incremental save / Save As
- **Crash recovery**: unsaved edits are journaled and offered back after a crash
- **Printing** through the operating system's print dialog
- **Creating PDFs**: blank documents, or converted from images, Word, Excel, PowerPoint and
  OpenDocument files (Office formats need LibreOffice installed)
- **CLI** (`ferrix`): `info`, `render`, `text`, `outline`, `organize`, and `compose` /
  `source` for documents written from scratch
- **Writing documents** ([ADR-0008](docs/adr/0008-writing-documents.md)): type and format
  letters and reports (styles, fonts, bold/italic/underline, colour, alignment, spacing,
  bulleted and numbered lists, tables, pictures, page breaks, headers and footers with page
  numbers, find and replace, formatted copy and paste), saved as tagged, accessible PDFs (or
  PDF/A-3 for archiving) that stay editable in Ferrix

Not yet: annotations, forms, text selection and search in PDFs, the sandboxed engine process,
and spell check in written documents.

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
cargo run -p ferrix-cli -- info document.pdf
cargo run -p ferrix-cli -- render document.pdf --pages 1-3 --dpi 144 -o out/
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

The bundled Liberation fonts in `crates/ferrix-compose/fonts/` are under the
[SIL Open Font License 1.1](crates/ferrix-compose/fonts/LICENSE-OFL).
