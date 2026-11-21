# Ferrix design system

How Ferrix looks and behaves. It covers the PDF viewer, the organizer, the writer and the dialogs.
Code follows it through the tokens and styles in `crates/ferrix-app/src/theme.rs`, and the
helpers in `view.rs`. When something here changes, change the code in the same commit.

## Principles

1. **The document is the hero.** Chrome is quiet, neutral and compact. Colour is for state
   (selection, focus, the one primary action), not decoration.
2. **Show only what works now.** Controls that can't act in the current state are hidden when a
   whole area doesn't apply (no document: no page navigation or zoom). They are disabled when the
   user should still see that they exist (Undo with no history).
3. **Group by task, not by widget.** File, history, navigation, zoom and view settings each form
   a cluster. Spacing separates clusters; avoid separator lines.
4. **Native-feeling on every desktop.** We follow conventions shared by macOS, Windows 11 and
   GNOME. Primary action on the right of a dialog footer, destructive action on the far left.
   Esc closes dialogs. Keyboard shortcuts appear in tooltips.
5. **Accessible by default.** Text contrast is at least 4.5:1 (WCAG 2.2 AA), and icons and
   borders of controls at least 3:1. Hit targets are at least 28 px, with a visible focus ring.
   Nothing is conveyed by colour alone.

## Tokens

All colours come from `theme::Tokens`; never write a colour literal in a view.

| Token | Use |
|---|---|
| `chrome` | toolbar, sidebar, status bar |
| `canvas` | behind pages |
| `surface` | inputs, menus, dialogs, cards |
| `surface_sunken` | grouped controls (segmented groups, pills) |
| `border` / `border_strong` | hairlines / control outlines |
| `text` / `text_muted` / `text_faint` | primary / secondary / disabled and placeholder |
| `hover` / `pressed` | translucent overlays on any surface |
| `accent` / `accent_hover` / `accent_soft` / `on_accent` | fills: primary action, selection, focus |
| `accent_text` | accent as text or icon colour (4.5:1 on `surface` and `chrome` in both themes) |
| `danger` | destructive actions, errors |
| `focus` | focus ring |
| `page_shadow` | paper shadow |
| `page_selection` | selected text on a page, viewer and writer: `#3563E9` at 28 %, the same in both themes (paper stays white) |

## Scales

- **Spacing (4-pt grid):** 2, 4, 8, 12, 16, 24, 32. Clusters in a toolbar are 12 apart; buttons
  inside a cluster are 2 apart.
- **UI font:** Inter, Regular and SemiBold, bundled (`crates/ferrix-app/assets/fonts`, SIL
  OFL 1.1) and set as the default font, so headings and body share one face on every platform.
  Never rely on a synthesised or fallback weight. Document fonts (Liberation) are never used for
  chrome.
- **Type (px):**
  - 11: captions, keycaps
  - 12: status bar, tooltips, thumbnail labels
  - 13: body text and controls
  - 15: section titles
  - 17: dialog titles (semibold)
  - 22: empty-state headings (semibold)
- **Radius:** 6 for controls, 8 for groups and menus, 12 for dialogs, 4 for thumbnails.
- **Contrast:** text on `accent` must reach 4.5:1 in both themes. In dark mode the accent fill is
  darker (`#3D6BF0`) and accent text lighter (`#8DA6FF`).
- **Heights:**
  - toolbar 48
  - toolbar controls 30 (icons 16)
  - dialog controls 32
  - status bar 28
  - sidebar 220 wide
  - writer format bar 42
- **Elevation:**
  - pages: two-layer shadow, a tight one plus a soft one
  - menus: 0 4 16 at 18%
  - dialogs: 0 12 32 at 28%, over a backdrop dimmed 25% (light) or 50% (dark)

## Components

- **Toolbar, left to right:**
  - the macOS window controls' space
  - sidebar toggle (PDFs only)
  - Undo and Redo, then the document title with an "Edited" dot when unsaved. The title takes
    the room left and clips, so it can never push buttons out of a narrow window
  - centre: the page navigator pill, `‹ [ 3 ] / 12 ›` (PDFs; a written document shows its
    page in the status bar)
  - right: the zoom pill `− 125 % ▾ +` with fit-width and fit-page inside it; file (New,
    Open, Save); then the overflow menu `⋯`

  The overflow menu holds Print…, Appearance (Match System / Light / Dark) and, for written
  documents, Page Setup…. With no document open, only the title, New, Open and `⋯` show.
  Below 980 px New and Open move into `⋯` (`view::is_compact`); below 820 px the fit toggles
  do too (`view::is_narrow`), so toolbar icons never shrink.
- **Writer format bar:** one pill per cluster, 12 apart, no rules: Style · Font · Size |
  B I U S · Colour | alignment · Spacing | lists · picture · page break · Table | Clear
  formatting, Page setup. Selects inside pills are borderless (`theme::select_flat`); a select
  used as a menu (Table) shows its label in full `text`, not as a placeholder.
- **Icon button:** ghost (background only on hover or press), 30×30, icon 16. When toggled on,
  `accent_soft` background with an `accent` icon. Every icon button has a tooltip of the form
  "Name (shortcut)".
- **Pills:** related controls share one `surface_sunken` rounded container (8) instead of
  separate bordered boxes.
- **Buttons:**
  - primary: accent fill, used once per view
  - secondary: surface with a `border_strong` outline
  - destructive: danger text, only after a confirmation step
  - dialog buttons are 32 high and at least 80 wide
  - a label opening a dialog ends with “…” (one character)
- **Dialogs:**
  - title (17, semibold), with a close button `×` at the top right
  - content in sections with 15-size titles, separated by 24 of space, not lines
  - footer buttons right-aligned, primary last
  - width 480 for confirmations, 560 for forms
- **Sidebar:**
  - header with a segmented switch (Pages | Outline)
  - thumbnails with a 1 px `border`, current page with a 2 px `accent` ring and the page number in
    an accent pill
  - organizer bar at the bottom under a hairline: "Page 3 selected" / "4 pages selected" in 12
    `text_muted`, then rotate ×2 | move ×2 | insert, delete (destructive last)
- **Status bar:** 12 px `text_muted`, one line. Left: document facts, or a transient message.
  Right: what only it shows (a written document's page). It never repeats the toolbar, and it is
  hidden when nothing is open and there is nothing to report.
- **Empty state:**
  - centred icon tile, heading and one sentence
  - primary Open… and secondary New Document…
  - keyboard hints beside the buttons they belong to
- **Dropdowns (pick lists):** surface, `border` outline, thin chevron, menu with `accent_soft`
  selection (not a solid accent block).

## Annotations

Phases 2 and 3 of ADR-0009. Annotating is a mode, as in Preview: reading stays uncluttered, and
tools appear only when asked for.

- **Markup toggle:** an icon button (`pen-line`, "Markup (⇧⌘A)") in the toolbar's right side,
  before the zoom pill; toggled-on style while the mode is on. It never folds into `⋯`.
- **Annotation bar:** 42 high under the toolbar (same height, `chrome` fill and bottom hairline as
  the writer format bar), pills 12 apart, padding [0, 10]:
  1. two tool pills (icon buttons 30, 2 apart). Text: Select (V) · Highlight (H) · Underline (U)
     · Strikethrough (S) · Squiggly underline (W). Add: Text box (T) · Note (N) · Stamp (M)
     · Pen (P) · Rectangle (R) · Ellipse (O) · Line (L) · Arrow (A)
  2. colour pill: seven 20 px swatches in 28 px targets
  3. width pill: 1, 2, 4, 8 pt as icon buttons showing a line of that weight (lines, shapes, ink)
  4. font size select (text boxes, `select_flat`, 72 wide): 8, 10, 12, 14, 18, 24, 36 pt
  5. opacity select (`select_flat`, 72 wide): 100 %, 75 %, 50 %, 25 %
  6. right-aligned: Comment (`message-square-text`) and Delete (`trash-2`), only with a
     selection

  Properties (2–5) edit the selection, else the active tool's defaults (remembered per tool for
  the session). Controls that don't apply are hidden. Narrow windows fold controls, never shrink
  them: below 980 px (`is_compact`) the four text-markup tools fold into one 42-wide button (last
  used, with a chevron opening a popover of all four); below 820 px (`is_narrow`) the four shapes
  fold the same way and the colour pill becomes one swatch with a chevron. Tool keys work in
  Markup when no text has focus, folded or not. Highlight, the underlines and Pen stay active.
- **Icons** (Lucide, 16): `type`, `highlighter`, `underline`, `strikethrough`, `copy`,
  `stamp`, `image`, `message-square-text`, and our own `underline-wavy` (Lucide `underline`
  with a wavy rule, same grid and stroke).
- **Annotation palette** (`theme::ANNOT_COLORS`, paper colours, the same in both themes):
  Red `#E03131`, Orange `#E8590C`, Yellow `#F2C200`, Green `#2F9E44`, Blue `#1C7ED6`,
  Purple `#7048E8`, Black `#1B1D21`. All but yellow reach 3:1 on white; yellow is for notes,
  whose icon has a dark outline. Defaults: notes yellow, everything else red. Swatches always
  have a 1 px `border_strong` ring (black vanishes on dark chrome otherwise); the chosen one gets
  a 2 px `text` ring outside a 2 px gap. Each swatch has a tooltip with its name, so colour is
  never the only cue. A colour from a file that isn't in the palette shows as an eighth swatch.
- **Selection on the page:** 1 px `accent` box, 8 × 8 handles (`surface` fill, 1 px `accent`
  border, 16 px hit area); lines get two 10 px round end handles; notes have no handles.
  Annotations Ferrix can't edit get a dashed `text_muted` box and no handles.
- **Sidebar:** the header's segmented switch gains Comments (Pages | Outline | Comments; with
  three segments, the segments are icons with tooltips). Comment rows: 8 × 12 padding, radius 6,
  selected row `accent_soft`; grouped under "Page 3" overlines.
- **Note editor:** a 280-wide popover (`theme::popover`, menu elevation) beside the note.
- **Comment rows for text:** a text box shows its first two lines; text markup shows the text it
  covers (read from the page, in curly quotes, two lines, “…” if cut), both 13 `text_muted`.
  Empty state: "Turn on Markup to add notes, highlights and drawings."

### Find in PDFs

- **Bar:** ⌘F opens a 40-high chrome bar under the toolbar (the writer's find bar without
  Replace): search icon, a 260-wide field "Find in document" (prefilled with a short selected
  line), Match case and accents, Previous (⇧⌘G), Next (↩, ⌘G), a status, Close (Esc).
- **Status:** "Searching…" until a match is found; "3 of 27", with "+" while pages remain to
  be searched; "No matches" when all are searched.
- **On the page:** every match in `page_find` (yellow), the current one in `page_find_current`
  (orange), one quad per line. The first match at or after the current page is shown as soon
  as it is found; a match already in view doesn't scroll the page, otherwise it is placed a
  third of the way down.
- **Closing** leaves the current match selected, so ⌘F, Esc, ⌘C copies it.

### Text selection (every mode)

- **Pointer:** I-beam over text, arrow elsewhere. Drag selects in text order, across pages;
  double-click selects a word, triple-click a line, Shift-click extends. A press elsewhere or Esc
  clears it. Painted in `page_selection`, one rect per line.
- **Keys:** ⌘A selects the document's text (the sidebar's pages when it has focus). ⌘C copies
  plain text, lines joined by line breaks, end-of-line hyphens dropped. When Find closes, the
  current match stays selected: ⌘F, Esc, ⌘C is the keyboard path.
- **With Markup's Select:** the topmost thing under the press wins: an annotation, then text, then
  nothing (clears). A drag that starts on a highlight selects text; a click selects the highlight.
- **No text** (scans): no I-beam; a markup tool shows “not allowed” and the status bar says "This
  page has no selectable text. It may be a scan." **Copying forbidden** by the file: Copy is
  disabled, tooltip "The document’s author doesn’t allow copying."

### Text markup

- **Tool first:** drag (or double- or triple-click) with a markup tool; the result paints live in
  the tool's colour, and release adds one annotation, one quad per line.
- **Selection first:** releasing a selection shows the **selection bar**, 8 above the first line,
  centred, clamped to the page, flipped below when there's no room. `theme::popover`, height 32,
  padding 2: Yellow · Green · Blue · Red highlight swatches (tooltips "Highlight yellow"…), 8 gap,
  Underline · Strikethrough · Squiggly underline, 8 gap, Copy (⌘C); 28 targets. It appears in
  every mode (markup buttons hidden in certified files). In Markup, H U S W also apply to it.
- **Existing markup:** outside Markup a click opens the same bar, with swatches recolouring and
  Delete (`trash-2`) instead of the underline buttons. In Markup it selects, without handles.
- **Defaults:** highlight yellow at 50 % with Multiply (text stays black); the others red at
  100 %, rule a tenth of the line height, at least 1 pt.

### Text box

- **Placing:** a click starts an auto-width box that grows as you type (Return breaks lines); a
  drag sets the width (at least 24 pt). Height always fits the text.
- **Look:** Liberation Sans (metrics of the `/DA` Helvetica), 12 pt, black by default (filling
  in, not marking up). No border, fill or alignment control; boxes from other apps keep theirs.
- **Editing in place:** a transparent `text_editor` over the page at the current zoom, 1 px
  `accent` frame, no handles. Esc, Tab, ⌘Return or a click outside commits; a box left empty
  disappears. One session, one undo step. Double-click, or Return on a selected box, edits again.
- **Selected:** drag the body to move; two side handles change the width (an auto-width box
  becomes fixed). Bar: colour and font size.

### Stamps (phase 4)

- **Tool:** one fold button (42 wide: icon `stamp` + chevron, the same component as the folded
  text tools) in the Add pill after Note: Text box · Note · Stamp · Pen · shapes. Tooltip
  "Stamp (M)". The icon arms the last stamp used (Approved at first); the chevron or M while it's
  armed opens the picker. The bar grows by 42 only; existing fold widths still fit at 980 and 820.
- **Picker:** `theme::popover` under the button, 232 wide, padding 8, clamped to the window. Seven
  rows, 36 high, each a preview chip as placed (paper white, so it reads the same in dark mode).
  Then 8 of space, a "Recent images" overline and up to four 48 × 48 thumbnails (this session
  only), then "Image…" (`image` icon). ↑ ↓ move, Return picks, Esc closes.
- **Standard set** (`markup::STAMPS`; `/Name` from ISO 32000 Table 181, so other apps recognise
  them): Approved, Final, For Public Release in green `#2B7A3B`; Not Approved, Confidential in
  red `#C92A2A`; Draft, For Comment in blue `#1864AB`.
- **Look:** upper-case Liberation Sans Bold, 18 pt; a 2 pt rounded border (radius 4 pt) in the
  ink, 6 × 10 pt padding, no fill. Stamp inks are darker than the palette, so the label reaches
  4.5:1 on paper. They are part of the meaning, so the colour pill is hidden for stamps.
- **Placing:** a click places the default size (text at 18 pt), centred on the pointer; a drag
  sets the width (at least 36 pt), with height following the aspect. Return places it at the
  centre of the visible part of the page. Clamped inside the page. It then returns to Select.
- **Resizing:** four corner handles, always proportional; no side handles. **No rotation:** a
  stamp is upright as the page is shown when it's placed. Rotated stamps from other apps stay
  `Other`.
- **Image stamps:** "Image…" opens "Choose an Image" (PNG, JPEG). Placed at its size at 96 dpi,
  capped at 200 pt wide; transparency is kept. Errors: "Couldn’t open “{name}”. Choose a PNG or
  JPEG image." In PDF/A-1 files, images with transparency are refused: "This document is
  PDF/A-1, which doesn’t allow transparent images."
- **Selected:** bar shows opacity, Comment and Delete. **Comments row:** `stamp` icon in the ink;
  title "Stamp · {author}"; line two the label ("Approved") or "Image", 13 `text_muted`.

### Comments and replies (phase 4)

- **One popover for every annotation:** the note editor becomes the comment popover, 320 wide,
  at most 60 % of the window high (the thread scrolls). It opens on a note's click, on Comment
  (`message-square-text`, in the bar beside Delete, "Comment and replies") or double-click for
  other annotations (text boxes: Comment only), and on Return or double-click in the Comments list.

  ```
  ┌───────────────────────────────────────┐
  │ Ana Ruiz · 2 hours ago                │  13 semibold · 12 text_muted
  │ “energy-efficient products”           │  markup only: quote, 12 muted
  │ [ Check these figures              ]  │  text editor, 13
  │                                       │  12
  │   Ben Ode · Yesterday at 14:05 [✎][×] │  replies indented 12; edit/delete
  │   I checked them, they’re fine.       │  on hover or focus
  │   Ben Ode set the status to Accepted  │  12 muted, read-only
  │                                       │
  │ [ Reply                            ]  │  grows to 4 lines
  │                              [Reply]  │  secondary, disabled when empty
  │ [Delete]                      [Done]  │  primary
  └───────────────────────────────────────┘
  ```
- **Dates:** "Just now", "{n} min ago", "Today at 14:05", "Yesterday at 14:05", "21 Nov",
  "21 Nov 2025" (system time format); the full date in a tooltip; nothing without `/M`.
- **Own replies** (author is your Author name) can be edited in place (Cancel · Save) and
  deleted; others' only deleted. Each send, edit or delete is one undo step. Deleting a parent
  with replies says so: tooltip "Delete note and 2 replies".
- **Author name:** a reply needs one. With none set, the first reply shows "Add your name so
  others know who replied." with a "Your name" field and Save (stored as the ADR-0005 preference).
- **Review status:** shown, never set. Status replies from other apps appear as one muted line;
  the latest also shows in the Comments row.
- **Comments row:** replies are not rows of their own. A third line in 12 `text_muted`: "2
  replies · Ben Ode, yesterday" (and " · Accepted"). The date sits right of the title.
- **Keys:** ⌘Return (Ctrl+Enter) sends; Return is a new line; Esc cancels an edit, then closes.
  On open, focus goes to the text when it's empty, else to the reply field. Tab: text, replies'
  buttons, reply field, Reply, Delete, Done.
- **Popups:** Ferrix shows its popover instead of `/Popup` annotations and writes none.

### Phase 4, no UI

- **Tagged PDFs:** annotations join the structure tree silently. The notice "Screen readers won’t
  announce annotations in this document yet." goes away. If a file's structure tree can't take
  them, the save succeeds and the status bar says "Screen readers may not announce some
  annotations in this document."
- **Pen pressure:** out of scope. iced 0.14 reports no pressure or tilt, so ink keeps one width.

Why: a mode keeps reading uncluttered, and reusing the format bar's rules adds no chrome tokens,
only the paper palette, `page_selection` and handle sizes. Selection lives outside Markup
because copying is reading. Stamps add three inks as data (`markup::STAMPS`), not tokens: the
palette's green and blue fall below 4.5:1 for a label on paper. Threads reuse the one popover so
there is one place to read and write comments, whether or not the sidebar is open.

## Writing rules

Plain language and sentence case. Use curly quotes and “…”. Buttons are verbs ("Save",
"Recover"), and a confirmation's buttons name the outcome ("Discard Changes", not "No").

## Verifying

Use the `ferrix-ui` skill's checklist. Check light and dark, 1180×860 and 760×900, no document
and a document, and every dialog touched.
