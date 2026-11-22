# ADR-0005: Storage for preferences and per-document state

- Status: **Accepted** (implemented 2026-11-22)
- Date: 2026-11-15

## Context
Ferrix keeps two kinds of local data with very different shapes:

| | User preferences | Per-document & history state |
|---|---|---|
| Examples | theme, default zoom/layout, key bindings, tool defaults, annotation author name | last page/zoom/layout per file, recent files, search history, document fingerprints, autosave journal index |
| Size | small, fixed set of keys | grows with every file opened (thousands of rows) |
| Writes | rare, on explicit change | frequent (every page turn), from several windows and later several processes |
| Access | load all at startup | look up by path or content hash; prune old entries |
| Human-edited? | yes: users edit, back up and sync dotfiles | no |

## Decision
Use the right tool for each:

1. **Preferences → one plain JSON file** (`settings.json` in the platform config dir via the
   `directories` crate), (de)serialized with `serde` into a versioned `Settings` struct with
   `#[serde(default)]` on every field so missing or unknown keys never break startup.
   Written atomically (temp file + rename). Reloaded when changed on disk.
2. **Per-document state and history → SQLite** (`state.db` in the platform data dir) via
   `rusqlite` with the `bundled` feature (no system dependency; public domain SQLite, MIT crate).
   WAL mode for safe concurrent access from several windows/processes; schema migrations
   numbered in code and recorded with `PRAGMA user_version`.
3. Both live behind a `ferrix-store` crate with a small API (`Settings::load/save`,
   `DocState::get(fingerprint)/put`, `Recent::list/push`) so the core never sees SQL or file
   formats, and tests use a temporary directory.

## Alternatives considered
- **Everything in SQLite:** atomic and queryable, but users can no longer read, edit, diff or
  sync their preferences, and every settings change needs UI.
- **Everything in JSON:** fine at first; degrades as per-document state grows (whole file
  rewritten on every page turn, lost updates when two windows write, no indexing or pruning).
- **TOML for preferences:** allows comments, which is nice for hand-editing; JSON chosen for
  zero-ambiguity round-tripping and wider tooling. Revisit if users ask for comments.

## Consequences
- + Preferences stay transparent and portable; history scales and survives concurrent writers.
- + Corruption is contained: a broken `state.db` can be moved aside and recreated without
  losing preferences, and vice versa.
- − Two storage mechanisms to maintain and test.
- − `rusqlite` with `bundled` adds a C compile step (cc) and ~1 MB to the binary.
- Privacy: `state.db` contains file paths and search terms; it gets a "Clear history" action
  and is never included in exported diagnostics.

## Implementation notes (2026-11-22)
- `crates/ferrix-store`: `Settings` (JSON) and `Store` (SQLite via `rusqlite` 0.43, `bundled`;
  new dependencies are MIT/Apache, SQLite itself public domain). The app's side is
  `ferrix-app/src/prefs.rs`: settings are read at startup and saved in the background whenever an
  update changes one (`App::save_settings` compares after every message); history writes run on a
  background thread, and the one read needed before the first paint (`Store::view`) is a single
  indexed row on the UI thread.
- **Locations:** `settings.json` in `ferrix_io::app_config_dir()` (macOS Application Support,
  Windows `%APPDATA%`, XDG config on Linux) and `state.db` in `app_data_dir()`. The `directories`
  crate was not needed. `FERRIX_DATA_DIR` overrides both, so tests and screenshots never touch
  the user's files.
- **Settings:** appearance, author name, sidebar shown, Dark Pages, last layout and zoom, and
  `restore_position`. Each field parses on its own, so one bad value costs only that field; keys
  from a newer version are kept on save and the version never goes down. Reloading on external
  change is not done yet.
- **History:** one `documents` table keyed by path (not content fingerprint yet: hashing large
  files on every open costs more than it saves; a renamed file starts fresh). It stores the last
  open time, page, offset within it, zoom and layout. It keeps 1000 rows, and Open Recent shows
  the 10 newest that still exist. A file that isn't a database is moved to `state.db.broken` and
  replaced. Clear Recent deletes every row. Schema migrations are numbered by `PRAGMA user_version`.
- **When positions are saved:** when a document closes (opening another, or recovery) and,
  synchronously, on quit. A crash loses the position, which is acceptable.

