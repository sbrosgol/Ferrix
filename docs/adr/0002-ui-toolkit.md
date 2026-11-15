# ADR-0002: UI toolkit — iced

- Status: **Accepted**
- Date: 2026-11-15

## Context
Requirements, in priority order:
1. A custom, GPU-backed document canvas: textured tiles, vector overlays, 120 Hz scroll/zoom,
   pen/touchpad gestures.
2. **Accessibility** (screen readers) for chrome *and* document content — a hard requirement for a
   PDF reader (education/government users).
3. Native feel: menus (macOS global menu), file dialogs, drag & drop, IME, HiDPI, multi-window.
4. Productivity for contributors; a permissive license with no commercial tiers, so the project
   stays `MIT OR Apache-2.0` end to end; active maintenance.

## Candidates

| | iced | Slint | gpui | Tauri (web UI) | egui |
|---|---|---|---|---|---|
| Canvas | `shader` widget on wgpu — excellent | wgpu texture import — good | excellent | tiles over IPC/custom protocol — costly | good |
| Accessibility | **not upstream** (forks only) | AccessKit built-in | in progress | best (browser a11y) | AccessKit |
| Native look | custom theme | Fluent/Cupertino/Material styles | custom | web | tool-like |
| License | MIT | GPLv3 / royalty-free / commercial | Apache-2.0 | MIT/Apache | MIT/Apache |
| Risk | a11y gap | DSL + license tiers | API churn, docs | two stacks, perf | polish |

## Decision
Use **iced** (MIT) for `ferrix-app`, without a comparative trial.

- Permissive and single-licensed, matching the project; no commercial license tiers.
- Elm-style architecture fits our design: `ferrix-core` owns state and commands, the UI maps
  messages to intents.
- wgpu-native with a `shader` widget, giving us full control of the tiled document canvas.
- Proven in a full desktop environment (COSMIC), on macOS, Windows and Linux.

## Consequences
- **Accessibility is the main risk.** Mitigations, in order:
  1. Track and contribute to upstream AccessKit integration in iced.
  2. If it isn't upstream by M3 (public beta), carry an AccessKit-enabled iced fork
     (forks already exist) behind a workspace `[patch]`, rebased each iced release.
  3. The document canvas exposes its own AccessKit subtree from `ferrix-text` regardless of
     toolkit, so that work is never wasted.
  A screen-reader audit remains an exit criterion for 1.0.
- Native menus and dialogs come from separate crates (`rfd` for file dialogs; `muda` for menus
  if needed), since iced does not provide them.
- iced makes breaking releases; we pin a minor version and upgrade deliberately.
- The toolkit stays confined to `ferrix-app`, so changing it later only touches that crate.
