# ADR-0004: Run the PDF engine out of process

- Status: **Accepted** (in-process transport allowed in M0–M1, out-of-process required by beta)
- Date: 2026-11-15

## Context
PDFium is C++ parsing hostile input; crashes and memory-safety bugs are historically common.
It is also not thread-safe — its authors recommend parallel *processes*, and `pdfium-render`
serializes calls behind a mutex.

## Decision
- Engine backends run in `ferrix-engine-host` worker processes, talking to the app over a
  length-prefixed `postcard` protocol on pipes/Unix sockets, with shared memory for tile pixels.
- A pool of N hosts gives render parallelism; one primary host per document owns edit state.
- Hosts are sandboxed per platform and have per-request time and memory limits; a watchdog
  kills and respawns stuck hosts, then the client re-opens documents and retries once.
- The same `PdfEngine` trait has an in-process transport for tests, the CLI and early milestones.

## Consequences
- + A malicious or broken PDF crashes a worker, not the app; unsaved edits survive (journal + core state).
- + Real multi-core rendering despite PDFium's global lock.
- − IPC protocol design, versioning, and shared-memory lifecycle are extra engineering.
- − Memory overhead: each host holds its own copy of document structures (mitigated by small pool
  and lazy loading).
