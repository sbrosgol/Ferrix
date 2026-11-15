# ADR-0008: Writing documents from scratch

- Status: **Accepted**
- Date: 2026-11-15
- Supersedes: the "not a PDF authoring tool" non-goal in `architecture.md` §1

## Context
Ferrix can create blank PDFs (ADR-0007) but offers no way to put anything on them. Users want to
write a letter, a report or a form in Ferrix and get a PDF, without a separate word processor.

A PDF is a finished printout: text is placed glyph by glyph, with no paragraphs to reflow. Typing a
word into the middle of a PDF page cannot push the rest of the paragraph along. Editing therefore
needs a document model of its own, from which the PDF is produced.

## Decision
1. **A new headless crate, `ferrix-compose`,** owns written documents: the model, the edits, the
   layout and the PDF export. It depends on no UI crate and is exercised by the CLI and tests.
2. **The model is ours and versioned:** page setup and a list of blocks. Phase 1 has paragraphs
   only: a paragraph style (Normal, Title, Heading 1–3), paragraph formatting (alignment, spacing,
   line spacing) and runs of text with character formatting (font, size, bold, italic, underline,
   strikethrough, colour). Formatting is stored as overrides of the paragraph style, as word
   processors do. It is serialized as JSON with a format version.
3. **One edit, `Replace { range, with }`, covers typing, deleting, splitting and joining
   paragraphs, pasting and formatting.** It replaces a range with a fragment (a list of
   paragraphs) and returns the `Replace` that undoes it, so ADR-0003's undo/redo and recovery
   carry over unchanged. Each resulting paragraph takes the properties of its fragment paragraph,
   which is what makes the inverse exact. Page setup changes are a second edit.
4. **Layout:** cosmic-text (already used by iced) shapes each paragraph and breaks it into lines,
   including bidirectional text and font fallback. `ferrix-compose` stacks the lines into pages,
   keeping headings with the paragraph that follows and avoiding single orphan and widow lines.
   All units are PDF points.
5. **One layout feeds both the screen and the PDF.** The layout result is a list of positioned
   glyphs per page. The editor draws it; the exporter writes it with **krilla**, which embeds and
   subsets the fonts. What the editor shows is what the PDF contains.
6. **Fonts:** Liberation Sans, Serif and Mono (SIL Open Font License 1.1) ship with Ferrix, so a
   document lays out identically on every machine. They are metric-compatible with Arial, Times New
   Roman and Courier New. Installed system fonts can be added, at the cost of that guarantee.
7. **One file.** Saving writes an ordinary PDF and embeds the document's source in it as an
   associated file (`ferrix-document.json`, relationship `Source`). The source records a digest of
   the PDF's page content. On open, Ferrix recomputes the digest: if it matches and no other
   application added annotations, the document opens for writing; otherwise it opens as a normal
   PDF, so changes made elsewhere are never silently overwritten.

## Consequences
- + Real word-processor editing (reflow, styles, undo) with output any PDF reader can open.
- + Saved files stay editable in Ferrix without a sidecar file.
- + Layout is deterministic with the bundled fonts, so it is testable byte-for-byte.
- − Saving writes the PDF twice: once to compute the content digest, once with the source
  embedded. Layout is computed once and reused.
- − The embedded source is visible as an attachment in other PDF readers.
- − Bundling fonts adds about 4.4 MB to the app.
- − Out of scope: tracked changes, comments, footnotes, mail merge, and opening `.docx`
  (conversion through ADR-0007 remains the way to bring Word files in).

## Phases
1. Model, edits, layout, pagination and PDF export; `ferrix compose` CLI; tests. *(this ADR)*
2. Editor view in `ferrix-app`: caret, selection, typing with input methods, clipboard,
   undo/redo, formatting toolbar, page setup, save/reopen, crash recovery.
3. Lists, images, page breaks, headers and footers with page numbers, find and replace.
4. Tables, pasting formatted text, spell check, tagged PDF, PDF/A.

## Implementation notes (phase 2, 2026-11-15)
- `ferrix_compose::Editor` holds the caret, selection, cursor movement (grapheme, word, line,
  page, document), hit testing, formatting commands and undo history. It has no UI dependency,
  so its behaviour is unit-tested. Consecutive typing or deleting in one paragraph is one undo
  step, recorded as a whole-paragraph replace so merged steps stay exact inverses.
- Each keystroke re-lays out only the changed paragraphs (`LayoutCache`, keyed by paragraph
  content and page width); pagination reruns in full.
- The editor shows pages rasterized by swash from the layout the PDF is written from, cut into
  512-pixel tiles like rendered PDF pages, and only for pages near the visible area. A
  transparent overlay widget above them draws the caret and selection and turns mouse,
  keyboard and input-method events into editor calls.
- Saving runs `ExportJob` (document, layout and the used font data, detached from the editor)
  on a background thread, then writes atomically; a file changed by another app since it was
  opened is not overwritten.
- Opening a PDF first looks for `ferrix-document.json` in its bytes; only then is it parsed
  for the source. Unchanged Ferrix PDFs open for writing, changed ones as ordinary PDFs with
  a notice.
- Crash recovery journals a snapshot of the document followed by every applied edit, undo and
  redo included; recovery replays them.
- Fonts (bundled plus system fonts, for other scripts) load on a background thread at startup
  and are reused between documents.
- Known gaps: the caret does not blink; text being composed with an input method is drawn in
  the UI font, not the document's; the font list shows every installed family.
