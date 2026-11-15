# ADR-0006: Printing through each platform's native print system

- Status: **Accepted**
- Date: 2026-11-15

## Context
Users expect the operating system's own print dialog (printer choice, page ranges, copies,
duplex, "Save as PDF", preview). There is no cross-platform Rust printing API, and each OS offers
a different level of support for printing a PDF directly.

## Decision
A `ferrix-print` crate wraps one native path per platform behind a small API
(`PrintJob` → `PrintOutcome`). It is the only crate besides engine backends allowed to use
`unsafe`, confined to FFI calls with `SAFETY` comments.

| Platform | Dialog | Who renders | Output |
|---|---|---|---|
| macOS | AppKit print **sheet** with live preview (`PDFDocument.printOperation`) | PDFKit | vector |
| Linux / BSD | xdg-desktop-portal `org.freedesktop.portal.Print` (works in Flatpak) | CUPS (PDF is its native job format) | vector |
| Windows | `PrintDlgEx` common dialog | **PDFium** (our engine) at printer DPI, capped at 300 dpi | raster via GDI |

- Pages are auto-rotated to match the paper orientation and shrunk (never enlarged) to the
  printable area. Page ranges and copies come from the dialog.
- What is printed is the document **as saved on disk**. Once editing lands (M2), printing an
  unsaved document first writes the current state to a temporary file through the normal save
  pipeline, so all three paths keep printing a real PDF.

## Alternatives considered
- **Rasterize everywhere with PDFium** for identical output: loses vector quality and native
  preview on macOS/Linux, and still needs three native dialog/spool implementations.
- **GTK print dialog on Linux:** heavy dependency, does not work inside Flatpak without the portal.
- **Windows `PrintManager` (WinRT):** modern UI, but requires an `IPrintDocumentSource`
  implementation and COM plumbing far beyond the GDI path; revisit for Windows on ARM polish.

## Consequences
- + Native, familiar dialogs on every platform; vector output on two of three.
- − On macOS and Linux, print output is rendered by PDFKit/CUPS rather than PDFium, so rare
  rendering differences between screen and paper are possible.
- − Windows output is raster (large spool files for long documents; 300 dpi ceiling).
- − Wayland windows get an unparented portal dialog until we export an `xdg-foreign` handle.
