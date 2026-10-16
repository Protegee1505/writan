# ADR-0003: App preferences (settings)

Status: accepted. Date: 2026-10-16.

## Context

M2.5 adds a preferences surface — theme (incl. follow-OS), accent, scale,
editor fonts, keybindings, search behavior — reachable from the command
palette, and meant to grow with every future feature. App state must live in
the OS app-data directory (ADR-0002), never in the vault ("Settings-first"
convention in `AGENTS.md`). Preferences must survive updates and stay
forward-compatible as options grow.

## Decisions

1. **Preferences live in `<app-data>/config.json`**, next to `vault_path`,
   under a single `settings` object. Rust treats it as opaque JSON (validated
   as an object, size-capped at 256 KiB) and exposes `get_settings` /
   `save_settings`. The schema is not duplicated in Rust: the TypeScript
   registry (`app/src/lib/settings.svelte.ts`) owns option metadata, defaults
   and the deep-merge on load, so adding an option never touches Rust.
2. **Session state rides in the same file**, under `settings.session`
   (e.g. `lastNoteId` for reopen-last-note). The registry excludes it from the
   UI and from "reset all", so resetting preferences never loses it.
3. **One-time migration:** if `appearance.theme` is absent and the legacy
   `writan-theme` localStorage key exists, seed the theme from it.
4. **Keybindings:** `settings.shortcuts` maps action id → canonical binding
   string (`Ctrl+Shift+K`). Defaults, matching, formatting and the dispatch
   live in `app/src/lib/keymap.ts`; the settings UI records bindings and
   rejects conflicts with other actions or reserved text-editing keys.
5. **Writes are atomic** (temp file + rename), and an unreadable or malformed
   `config.json` falls back to defaults instead of failing startup.

## Consequences

- Preferences are device-local, portable with the app-data folder, never
  synced, never inside the vault.
- Rust does not validate individual options; the frontend deep-merges stored
  values over defaults, so bad values self-heal and unknown keys from the
  future are ignored.
- Android reuses the same config shape; only the app-data directory differs.
