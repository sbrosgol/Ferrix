# Ferrix

A fast, safe, open-source PDF reader and editor for macOS, Windows and Linux, written in Rust.

> Status: early development (milestone M0). The CLI works; there is no desktop app yet.

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
cargo run -p ferrix-cli -- info document.pdf
cargo run -p ferrix-cli -- render document.pdf --pages 1-3 --dpi 144 -o out/
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
