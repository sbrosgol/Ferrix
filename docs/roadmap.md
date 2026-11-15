# Ferrix — Roadmap

Durations assume ~2–3 core contributors. Each milestone ends with a tagged pre-release and a
demo. Exit criteria are the definition of done, not the feature list.

## M0 — Foundations (weeks 1–4)
- Cargo workspace per `architecture.md` §4.1, shared lints, MSRV, `rustfmt`/`clippy`/`cargo-deny` in CI
  on macOS, Windows, Linux.
- `xtask fetch-pdfium` (pinned, hash-verified), `xtask corpus` (download test PDFs from manifest).
- `ferrix-types`, `ferrix-engine` trait + DTOs, `ferrix-engine-pdfium` (open, page info, render, text).
- `ferrix-cli render <file> --page N --dpi D -o out.png` and `ferrix-cli text`.
- Visual-regression harness running on the corpus.
- `ferrix-app` skeleton on iced (ADR-0002): window, open file, page navigation; built on all platforms in CI.
- Repo hygiene: LICENSE, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY.md, issue templates.

**Exit:** CI green on 3 OSes; CLI renders 95% of corpus without error; app skeleton runs on all 3 OSes.

## M1 — Viewer MVP (weeks 5–12) → `0.1`
- App shell: open (dialog, drag & drop, file association, recent files), tabs.
- `ferrix-render`: continuous/single/two-page layouts, tiled progressive rendering, zoom
  (fit width/page, pinch, ctrl+wheel), rotation, HiDPI, memory budget.
- Thumbnails sidebar, outline/bookmarks, internal & external links (with confirmation).
- Text selection + copy, find-in-document with streaming results.
- Password-protected PDFs, light/dark/system theme, page-inversion reading mode.
- Keyboard navigation and basic a11y for chrome.
- `ferrix-store` (ADR-0005): preferences, recent files, per-document last page and zoom.

**Exit:** first paint < 300 ms on 1,000-page doc; 60+ fps scroll; zero crashes on corpus in a
1-hour soak; screen reader can operate all chrome.

## M2 — Annotate & save (weeks 13–20) → `0.2`
- ~~Command stack, undo/redo~~ (done: `ferrix-core`); autosave journal + crash recovery.
- ~~Page organizer: reorder, rotate, delete, insert from file~~ — delivered early (see ADR-0003
  notes); remaining: drag-and-drop reordering, extract/split, blank pages.
- Highlight / underline / strikeout / squiggly, sticky notes, free text, ink (pressure), shapes, stamps.
- Annotation list panel; edit/delete existing annotations from other apps.
- ~~Incremental + atomic + verified save; Save As~~ (done: `ferrix-pdf`, `ferrix-io`).
- Engine moved out of process (ADR-0004) with crash recovery.

**Exit:** round-trip tests pass for every annotation type in PDFium, hayro, Acrobat-equivalent check
(`qpdf --check`); kill-9 during edit loses ≤ 1 s of work.

## M3 — Organize, forms, print (weeks 21–28) → `0.3` (public beta)
- Page organizer: reorder, rotate, delete, insert blank/from file, extract, merge, split.
- AcroForm fill (text, checkbox, radio, combo, list), tab order, flatten.
- Visual signature (draw/type/image), reusable.
- ~~Printing via OS dialogs~~ — delivered early in `ferrix-print` (ADR-0006); remaining: print
  the unsaved in-memory state once editing lands.
- Sandboxing of engine hosts on all platforms; packaging + signing; Flathub, winget, Homebrew.
- Localization infrastructure + 3 initial languages.

**Exit:** signed installers on all platforms; public beta.

## Delivered ahead of plan
- New blank documents; images, Word, Excel, PowerPoint and OpenDocument files to PDF (ADR-0007).

## M4 — Pro features (weeks 29–40) → `1.0`
- True redaction (search & redact patterns; purge content; full save).
- OCR (`ocrs`) for scanned documents → searchable/selectable text layer.
- Digital signatures: validate existing (PAdES), sign with OS cert store / PKCS#12.
- Optimize / compress, metadata editor, PDF/A conversion (basic).
- Accessibility of page content (tagged PDF structure → AccessKit); a11y audit.

**Exit:** external security review of engine host + IPC; a11y audit with screen-reader users; 1.0.

## Post-1.0
- Edit existing text and images (page-object level).
- Compare documents (visual + text diff).
- Plugin/scripting API on top of the command model.
- Pure-Rust backend (hayro) promoted to default for viewing if fidelity parity is reached.
- Mobile/web targets reusing `ferrix-core`.

## Top risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| iced lacks upstream screen-reader support | High | High | Contribute upstream; AccessKit fork via `[patch]` by M3; canvas a11y tree is toolkit-independent; toolkit confined to `ferrix-app` |
| PDFium API gaps for editing | Med | Med | `ferrix-pdf` object layer; contribute upstream |
| File corruption on save | Low | Critical | Atomic + verified saves, journal, round-trip & fuzz tests |
| Malicious PDFs | High | High | Out-of-process sandboxed engine, no JS, fuzzing |
| Cross-platform packaging/signing burden | High | Med | Automate early (M0/M3), cargo-packager, CI release pipeline |
| Contributor onboarding on a large codebase | Med | Med | Headless core, CLI, good-first-issue labeling, architecture docs |
