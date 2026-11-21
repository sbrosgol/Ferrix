---
name: ferrix-ui
description: Design and build Ferrix desktop UI (iced 0.14) to the Ferrix design system - tokens, spacing, type, components, states, accessibility and copy - and review it with screenshots. Use whenever adding or changing a view, toolbar, sidebar, dialog, menu, theme colour or icon in ferrix-app.
---

# Ferrix UI work

The rules live in `docs/design-system.md`; read it first. This skill is how to apply them.

## Before you change anything
1. Screenshot the current state with the `ferrix-screenshot` skill: light and dark, 1180x860 and
   760x900, and with no document as well as with one. Use a **fresh `FERRIX_DATA_DIR` for every
   run**: a run that types into the writer leaves a recovery journal, and the next run then opens
   on a recovery dialog.
2. Write down what is wrong in design terms (hierarchy, grouping, states, contrast, alignment,
   copy), not just "looks old".

## Building it in iced
- **Colours** only from `theme::tokens(theme)`; add a token (in both `LIGHT` and `DARK`) rather
  than a literal. Styles are functions in `theme.rs` (`fn(&Theme, Status) -> Style`). Views pick
  styles; they never build them inline.
- **Sizes** come from the scales in the design doc. Shared helpers in `view.rs`
  (`icon_button`, pills, dialog frame, section titles) keep them consistent. Extend a helper
  rather than copying layout code.
- **Icons:** Lucide SVGs vendored in `crates/ferrix-app/assets/icons` (ISC). To add one, copy the
  SVG at the pinned version, add a variant in `icon.rs`, and keep the 1.75 stroke. No icon fonts,
  no new icon sets.
- **Fonts:** the UI uses iced's default sans. Get semibold with `Font { weight: Weight::Semibold,
  ..Font::DEFAULT }`. Never use the bundled document fonts (Liberation) for chrome.
- **States:** every interactive element needs hover, pressed, disabled and (where it can toggle)
  active styles. Hide whole areas that can't apply; disable single controls that temporarily
  can't act.
- **Keyboard:** each new action gets a shortcut where platforms have a convention, shown in its
  tooltip ("Zoom In (⌘+)" on macOS, "Ctrl++" elsewhere, via the existing shortcut-label helper
  if there is one).
- **Headless first** still applies: layout decisions that depend on state (what is visible,
  enabled or labelled) belong in small pure functions with unit tests, not buried in view code.

## Traps
- The screenshot emulator uses a software renderer. **Shadows and blur may not appear** there
  even though they render on the GPU. Judge elevation on a real run (`cargo run -p ferrix-app`),
  and don't "fix" a missing shadow by adding borders everywhere.
- Raster images must stay cut into ≤512 px tiles (`render::tiled_image`), or pages go blank in
  screenshots.
- Overlays (caret, selection, search hits, annotation drafts) go in a layer above the page images.
- `pick_list` handles are glyphs, not SVGs. Use `pick_list::Handle` with the chevron the design
  doc specifies, not the default filled triangle.

## Review checklist
- [ ] Matches the toolbar, dialog, sidebar and status-bar anatomy in the design doc.
- [ ] Spacing is on the 4-pt grid; clusters 12 apart, buttons in a cluster 2 apart.
- [ ] Type uses the scale (11/12/13/15/17/22); one semibold title per surface.
- [ ] Light and dark both look intentional; text contrast ≥ 4.5:1, control outlines ≥ 3:1.
- [ ] No control looks enabled when it can't act; nothing appears with no document that needs
      one.
- [ ] Every icon button has a tooltip with its shortcut; targets ≥ 28 px; Esc closes dialogs.
- [ ] Copy: sentence case, curly quotes, “…” for commands that ask for more input, verbs on
      buttons.
- [ ] Narrow window (760 wide): nothing clips or overlaps; lower-priority controls fold into `⋯`.
- [ ] `cargo clippy` clean and the app's unit tests pass.

Finish by having the `ui-verifier` agent check the screenshots independently, or the
`ux-designer` agent for a design critique.
