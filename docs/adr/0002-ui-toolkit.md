# ADR-0002: UI toolkit

- Status: **Proposed** — decided after a 2-week spike (roadmap M0)
- Date: 2026-11-15

## Context
Requirements, in priority order:
1. A custom, GPU-backed document canvas: textured tiles, vector overlays, 120 Hz scroll/zoom,
   pen/touchpad gestures.
2. **Accessibility** (screen readers) for chrome *and* document content — a hard requirement for a
   PDF reader (education/government users).
3. Native feel: menus (macOS global menu), file dialogs, drag & drop, IME, HiDPI, multi-window.
4. Productivity for contributors; stable, permissively usable license; active maintenance.

## Candidates

| | iced | Slint | gpui | Tauri (web UI) | egui |
|---|---|---|---|---|---|
| Canvas | `shader` widget on wgpu — excellent | wgpu texture import — good | excellent | tiles over IPC/custom protocol — costly | good |
| Accessibility | **not upstream** (forks only) | AccessKit built-in | in progress | best (browser a11y) | AccessKit |
| Native look | custom theme | Fluent/Cupertino/Material styles | custom | web | tool-like |
| License | MIT | GPLv3 / royalty-free / commercial | Apache-2.0 | MIT/Apache | MIT/Apache |
| Risk | a11y gap | DSL + license nuance | API churn, docs | two stacks, perf | polish |

## Proposed decision
Spike **Slint** and **iced** on the same scope: canvas with tiled scrolling of a 500-page PDF,
text selection overlay, toolbar + sidebar, VoiceOver/NVDA reading toolbar and page text.

- Lean: **Slint**, because accessibility is a hard requirement and upstream iced lacks it.
- Pick iced if the Slint canvas integration cannot hit frame-time targets or custom a11y nodes for
  page text are not achievable.

Regardless of the outcome, the toolkit lives only in `ferrix-app`; `ferrix-core` exposes state +
intents, so the cost of changing toolkits later is bounded to the presentation layer.

## Exit criteria for the spike
- p95 frame time < 8 ms while flinging through pages on a 2020-era laptop.
- Screen reader can read toolbar controls and the selected page's text in reading order.
- IME input works in a form-field text box.
- Written comparison appended to this ADR, status → Accepted.
