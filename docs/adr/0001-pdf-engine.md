# ADR-0001: PDF engine — PDFium behind an engine trait, pure-Rust track in parallel

- Status: **Accepted (pending license confirmation)**
- Date: 2026-11-15

## Context
The PDF engine (parse, render, text extraction, annotation/form/page APIs, save) is the
largest technical risk. Writing one from scratch to Chrome/Acrobat fidelity is multi-year work.

| Option | License | Fidelity | Editing APIs | Rust story | Notes |
|---|---|---|---|---|---|
| **PDFium** via `pdfium-render` | BSD-3 / Apache-2.0 | Excellent (Chrome) | Annotations, forms, page objects, import pages, incremental save | Mature bindings, dynamic lib | Not thread-safe → serialize per process |
| MuPDF via `mupdf` crate | **AGPL-3.0** or commercial | Excellent | Richest (redaction, text edit) | Decent bindings | Forces whole app to AGPL |
| `hayro` (pure Rust) | Apache/MIT | Good, improving; self-described experimental | Render only | Native, `forbid(unsafe)` | CPU-only, no editing |
| `lopdf` / `pdf` crates | MIT | n/a (no renderer) | Object-level read/write | Native | Good for structural edits |
| Poppler | GPL | Good | Limited | Weak | GPL, C++ build pain |

## Decision
1. Define `ferrix-engine` as a backend-neutral trait with Ferrix-owned DTOs.
2. Ship **PDFium** as the v1 backend (rendering, text, annotations, forms, page ops, save).
3. Maintain `ferrix-pdf` (built on `lopdf` or our own object layer) for operations PDFium lacks or
   does poorly: outline editing, XMP metadata, controlled incremental-update writing, signature
   byte-range handling, verification.
4. Run `hayro` as an **experimental second backend** in CI (render-only) to keep the trait honest
   and to provide a future pure-Rust path.

## Consequences
- + Production fidelity from day one; permissive licensing for the whole project.
- + Backend swap stays possible; differential testing between PDFium and hayro catches bugs.
- − C++ binary dependency: we must pin, fetch, verify and ship per-platform PDFium builds.
- − PDFium is single-threaded and C++ (memory-unsafe) → requires process isolation (ADR-0004).
- − Two writers (PDFium save and `ferrix-pdf`) must never interleave on one revision; each save
  is owned by exactly one writer, enforced in `ferrix-io`.
- If the project chooses AGPL instead, revisit: MuPDF would then be the stronger editing engine.
