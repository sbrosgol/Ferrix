# ADR-0007: Creating new PDFs and converting other formats

- Status: **Accepted**
- Date: 2026-11-15

## Context
Users want to start a document from scratch and to turn Word documents, spreadsheets,
presentations and images into PDFs without leaving Ferrix. Converting office formats means laying
them out (fonts, pagination, tables), which needs a full office suite; there is no Rust layout
engine for `.docx` of usable fidelity.

## Decision
1. **Blank documents** and **images** are created in-process by `ferrix-pdf`
   (`blank_document`, `image_document`). JPEGs are embedded byte-for-byte (`DCTDecode`) unless they
   carry an EXIF orientation, which is applied and re-encoded; other images are stored losslessly
   (`FlateDecode`) with an `SMask` for transparency. Images are placed at 96 dpi and shrunk to fit
   A4 in their own orientation.
2. **Office formats** are converted by `ferrix-convert`, which drives an installed suite as a
   background process with a timeout:
   - **Microsoft Word** for word-processing formats when installed — AppleScript on macOS,
     COM via PowerShell on Windows (with `AutomationSecurity = ForceDisable`, because COM
     automation otherwise runs document macros). On macOS Word is sandboxed: it reads the file in
     place and writes the PDF as a hidden file beside it, which Ferrix reads and deletes.
   - **LibreOffice** (`soffice --headless --convert-to pdf`) for everything else, and as the
     fallback when Word is missing or fails. It runs with a private profile
     (`-env:UserInstallation`) so it never interferes with a LibreOffice the user has open;
     conversions are serialized because they share that profile.
3. Converted and new documents open **untitled** (in memory, marked unsaved); saving asks for a
   location, proposing `<name>.pdf` beside the source file. Insert Pages accepts the same formats.
4. Ferrix does not bundle an office suite. Without one, office formats are hidden from file
   dialogs and the New Document dialog explains how to get LibreOffice.

## Consequences
- + Good fidelity (Word for `.docx`, LibreOffice otherwise) with no layout code of our own.
- + Same behavior on all platforms through LibreOffice; Word is an optional upgrade.
- − Office conversion depends on third-party software the user must install.
- − Office suites are large attack surfaces for untrusted documents. Conversion is always
  user-initiated, runs out of process with a timeout, and macros are disabled (explicitly for
  Word on Windows; by LibreOffice's default macro security elsewhere).
- − Word on macOS can show its own "Grant File Access" dialog and macOS asks once for permission to
  automate Word; if that is refused or times out, Ferrix falls back to LibreOffice.
- − PDF/A or tagged-PDF output options are not exposed yet.
