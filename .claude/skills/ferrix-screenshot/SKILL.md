---
name: ferrix-screenshot
description: Render the Ferrix desktop app headlessly to a PNG and inspect it, optionally after scripted actions (writing a document, opening the New dialog, zooming). Use to check a UI change visually without a display.
---

# Headless screenshots of ferrix-app

The example `crates/ferrix-app/examples/screenshot.rs` runs the app in iced_test's emulator
(software renderer), replays scripted messages, and saves a PNG at 2× scale.

```sh
S=<scratch dir>                     # never the repo, never the user's data dir
rm -rf "$S/data" && mkdir -p "$S/data"
FERRIX_DATA_DIR="$S/data" SHOT_SECS=6 SHOT_SIZE=1180x860 \
  cargo run -q -p ferrix-app --example screenshot -- "$S/shot.png" [document.pdf] [flags]
```
Then Read `$S/shot.png`.

Flags: `--dark`, `--new` (New Document dialog), `--rotate`, `--recover`, `--write` (types a
formatted letter), `--features` (lists, picture, page break, table, footer, find bar),
`--fit-page`, `--fit-width`, `--zoom-in`, `--next` (three pages forward). A tall `SHOT_SIZE` (e.g. `760x1700`) shows several
pages. Increase `SHOT_SECS` for long scripts (each scripted step waits a little).

To script something new, add a flag and a `Vec<Message>` builder next to `writing_script()` /
`features_script()`; `ferrix_app::{WriteAction, KeyCommand, writer::*}` are public for this.

## Traps

- **Always isolate `FERRIX_DATA_DIR`.** The real recovery directory
  (`~/Library/Application Support/Ferrix/recovery` on macOS) may hold the user's crash journals:
  a recovery dialog blocks every scripted keystroke, and a run killed with unsaved changes leaves
  a journal behind. Never delete files there.
- **Blank pages ≠ broken rendering.** The software renderer drops raster images that extend past
  the clip area unless they are cut into ≤512 px tiles (`render::tiled_image`). Keep page images
  tiled.
- Overlays (caret, selection, search hits) must be drawn in a layer above images; within one
  layer images draw after quads.
- The emulator runs at scale factor 1 and screenshots at 2×, so text looks softer than on a
  real Retina display.
