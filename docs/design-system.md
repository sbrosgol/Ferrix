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
| `accent` / `accent_hover` / `accent_soft` / `on_accent` | primary action, selection, focus |
| `danger` | destructive actions, errors |
| `focus` | focus ring |
| `page_shadow` | paper shadow |

## Scales

- **Spacing (4-pt grid):** 2, 4, 8, 12, 16, 24, 32. Clusters in a toolbar are 12 apart; buttons
  inside a cluster are 2 apart.
- **Type (UI font, px):**
  - 11: captions, keycaps
  - 12: status bar, tooltips, thumbnail labels
  - 13: body text and controls
  - 15: section titles
  - 17: dialog titles (semibold)
  - 22: empty-state headings (semibold)
- **Radius:** 6 for controls, 8 for groups and menus, 12 for dialogs, 4 for thumbnails.
- **Heights:**
  - toolbar 48
  - toolbar controls 30 (icons 16)
  - dialog controls 32
  - status bar 28
  - sidebar 220 wide
- **Elevation:**
  - pages: two-layer shadow, a tight one plus a soft one
  - menus: 0 4 16 at 18%
  - dialogs: 0 12 32 at 28%, over a backdrop dimmed 25% (light) or 50% (dark)

## Components

- **Toolbar, left to right:**
  - the macOS window controls' space
  - sidebar toggle (PDFs only)
  - document title, with an "Edited" dot when unsaved
  - centre: the page navigator pill, `‹ [ 3 ] / 12 ›` (PDFs; a written document shows its
    page in the status bar)
  - right: history (Undo, Redo); zoom pill `− 125 % ▾ +` with the fit toggles; file (New,
    Open, Save, Print); then the overflow menu `⋯`

  The overflow menu holds Appearance (Match System / Light / Dark) and, for written documents,
  Page Setup…. With no document open, only the title, New, Open and `⋯` show. Below 980 px,
  New, Open and Print move into `⋯` (`view::is_compact`).
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
  - organizer actions in a compact bar at the bottom, with "N selected"
- **Status bar:** 12 px `text_muted`. Left: document facts. Right: transient messages. It never
  repeats what the toolbar already shows.
- **Empty state:**
  - centred icon tile, heading and one sentence
  - primary Open… and secondary New Document…
  - keyboard hints beside the buttons they belong to
- **Dropdowns (pick lists):** surface, `border` outline, thin chevron, menu with `accent_soft`
  selection (not a solid accent block).

## Writing rules

Plain language and sentence case. Use curly quotes and “…”. Buttons are verbs ("Save",
"Recover"), and a confirmation's buttons name the outcome ("Discard Changes", not "No").

## Verifying

Use the `ferrix-ui` skill's checklist. Check light and dark, 1180×860 and 760×900, no document
and a document, and every dialog touched.
