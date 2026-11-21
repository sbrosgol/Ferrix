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

Phase 2 of ADR-0009. Annotating is a mode, as in Preview: reading stays uncluttered, and tools
appear only when asked for.

- **Markup toggle:** an icon button (`pen-line`, "Markup (⇧⌘A)") in the toolbar's right side,
  before the zoom pill; toggled-on style while the mode is on. It never folds into `⋯`.
- **Annotation bar:** 42 high under the toolbar (same height, `chrome` fill and bottom hairline as
  the writer format bar), pills 12 apart, padding [0, 10]:
  1. tools pill: Select · Note · Pen · Rectangle · Ellipse · Line · Arrow (icon buttons 30, 2 apart)
  2. colour pill: seven 20 px swatches in 28 px targets
  3. width pill: 1, 2, 4, 8 pt as icon buttons showing a line of that weight (lines, shapes, ink)
  4. opacity select (`select_flat`, 72 wide): 100 %, 75 %, 50 %, 25 %
  5. right-aligned: Delete (`trash-2`), only with a selection

  The properties (2–4) edit the selection if there is one, otherwise the active tool's defaults
  (remembered per tool for the session). A control that doesn't apply is hidden (width for notes).
  Below 820 px the colour pill becomes one swatch with a chevron that opens the seven in a popover.
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

Why: a mode keeps the reading toolbar unchanged; reusing the format bar's height and pill rules
means no new chrome tokens. The only new constants are the paper palette and handle sizes.

## Writing rules

Plain language and sentence case. Use curly quotes and “…”. Buttons are verbs ("Save",
"Recover"), and a confirmation's buttons name the outcome ("Discard Changes", not "No").

## Verifying

Use the `ferrix-ui` skill's checklist. Check light and dark, 1180×860 and 760×900, no document
and a document, and every dialog touched.
