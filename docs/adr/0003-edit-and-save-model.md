# ADR-0003: Edit model — commands, overlays, incremental save

- Status: **Accepted**
- Date: 2026-11-15

## Context
Users expect unlimited undo, instant visual feedback, crash recovery, and saves that don't break
existing digital signatures or corrupt files. Engines are slow to re-render after every mutation.

## Decision
1. **All mutations are typed `Command`s** that are serializable and return their inverse on apply.
   The `CommandStack` provides undo/redo; adjacent compatible commands coalesce (ink strokes, typing).
2. **Two-phase visual model:** in-progress edits render as **vector overlays** drawn by Ferrix.
   On commit, the command is pushed to the engine and only the affected page region is re-rendered.
3. **Commands are journaled** (append-only, `postcard` frames with checksums) for crash recovery
   by replay onto the original file.
4. **Save defaults to incremental update** (append-only), which preserves existing signatures and
   is fast. **Full rewrite** is used for "Save As → optimize", and is **mandatory after redaction**
   so removed content is physically purged.
5. **Saves are atomic and verified:** temp file → fsync → reopen and validate → atomic replace.

## Consequences
- + Undo, recovery, scripting (`ferrix-cli apply edits.json`) and testing share one mechanism.
- + Property tests: `apply ∘ inverse = id`, `save ∘ reopen` preserves model.
- − Every feature must implement a correct inverse — including page deletes (keep removed pages in
  the session until save) and redaction (undo allowed only before save).
- − Incremental saves grow files; we offer "Optimize" and suggest it when update sections pile up.
