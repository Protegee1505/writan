# Writan — progress log

Living document: update it whenever a milestone or notable task lands.
Plans and exit criteria live in `docs/PLAN.md`; agent instructions in `AGENTS.md`.

Last updated: 2026-10-16 — M2.5 landed and release build verified (exe 6.53 MB, MSI 3.29 MB, setup 2.41 MB, ≈120 MB private idle RAM on Windows); Linux goes native via ADR-0004.

## Current state

- Branch `main`. **M0 complete, M1 complete, M2 desktop MVP (first pass) complete,
  M2.5 preferences & polish complete, release build verified.**
  M2 remaining: IME acceptance now rides on the GTK4 shell (ADR-0004), plus the
  runtime perf budgets; then M3 (sync v1 + E2EE), then M3.5 (native Linux shell,
  ADR-0004).
- Verification after M2.5: fmt/clippy clean, `cargo test --workspace` 59 tests
  green, `svelte-check` 0 errors / 0 warnings, frontend build OK, app launched
  via `tauri dev`.
- Release build (2026-10-16, Windows): `tauri build` exit 0 — exe 6.53 MB
  (codegen-units 1, LTO, strip), MSI 3.29 MB, NSIS setup 2.41 MB; cold start to
  window ≈396 ms; idle ≈120 MB private / ≈249 MB working set (the app process
  itself is 4.4 MB private — the rest is WebView2). npm Tauri packages pinned
  to 2.11.1 / 2.7.3 / 2.5.5 so the release build's version gate passes (the
  dev build only warns).
- Real-session verification: the user ran the app, chose
  `C:\Users\Administrator\Downloads` as the vault, switched the theme to dark
  and reopened the last note after relaunch — settings/session round-trip
  through app-data `config.json` confirmed.
- This dev box had been re-imaged without a Rust toolchain; on 2026-10-16 Rust
  1.99.1 (rustup) and VS 2022 Build Tools (MSVC 14.44.35207, Windows SDK
  10.0.26100) were installed here.
- The app has not been tested on Linux or Android yet (the GTK4 spike is the
  first Linux work).

## Milestone log

### M0 — bootstrap (commit 0324dc5)
- Toolchain on Windows: Rust 1.99.1 + MSVC (VS Build Tools, Win11 SDK 22621);
  Node 24 / npm 11.
- Cargo workspace: `crates/writan-core`, `-crypto`, `-sync`, `-server`,
  `app/src-tauri`; Tauri 2 + SvelteKit app shell; CI (fmt, clippy `-D warnings`,
  tests on Windows + Ubuntu; cargo-deny job); `docs/PLAN.md`; ADR-0001.
- Evidence: 18 tests, fmt/clippy clean.

### M1 — vault core (commit 24c2654; docs commit 7868951)
`crates/writan-core`:
- `vault.rs` — Markdown + YAML frontmatter as source of truth; unknown
  frontmatter keys preserved; adoption of plain `.md` files; atomic saves
  (temp + fsync + rename); `<slug>-<id8>.md` naming.
- `index.rs` — SQLite (rusqlite bundled) + FTS5 **trigram** index: CJK and
  substring queries work; LIKE fallback below 3 characters; tag/folder/pin
  filters; snippets. Always rebuildable from files.
- `watch.rs` — notify-based watcher with debounce; `VaultEvent::{Upserted,
  Removed}`.
- `trash.rs` — soft delete to `<state-dir>/trash/` with JSON sidecar;
  restore/purge.
- `workspace.rs` — orchestration: full `reconcile()`, incremental
  `reconcile_paths()`, self-write suppression via (mtime_ns, size) compare;
  save/create/rename/delete/restore/pin/search APIs.
- Format decisions: `docs/adr/0002-vault-format.md`.
- Evidence: 54 tests green (incl. 9 integration tests in
  `crates/writan-core/tests/workspace.rs`); clippy/fmt clean.

### M2 — desktop MVP, first pass (commit bf17157)
- `app/src-tauri/src/state.rs` — app state; vault path persisted to
  `<app-data>/config.json`; reopens the last vault at startup; watcher-thread
  generation guard.
- `app/src-tauri/src/commands.rs` — 14 commands: status, open vault, list,
  search, get/create/save/rename/delete, trash list/restore/purge, pin,
  rebuild index. camelCase DTOs; errors mapped to `String` at the boundary.
- `app/src-tauri/src/lib.rs` — plugins (opener, dialog); setup opens the last
  vault; watcher emits a `vault-changed` event to the UI.
- UI (`app/src`): three-pane layout, CodeMirror 6 Markdown editor with autosave
  (700 ms) and `Ctrl+S`, debounced search with snippets, trash panel, pinning,
  light/dark theme, welcome flow when no vault; shortcuts `Ctrl+N` / `Ctrl+F` /
  `Ctrl+S`; typed IPC wrappers in `lib/api.ts`; runes store in
  `lib/store.svelte.ts`.
- Evidence: frontend build + svelte-check clean; workspace clippy/tests clean.

### M2 — live preview (commit 9f834ec)
- `app/src/lib/editor/markdown-live.ts` — CodeMirror 6 view plugin styling
  headings, quotes, code blocks and links via decorations; syntax marks are
  dimmed — and hidden away from the cursor — while the source stays Markdown.
- Evidence: svelte-check clean; frontend build OK.

### M2 — tags & folders (2026-10-16)
- Core: `Index::tag_counts()` (JSON1 aggregation over indexed tags),
  `Workspace::tag_counts()` / `folder_counts()`; new commands `list_tags` /
  `list_folders`; `search_notes` now accepts tags/folder/pinned filters;
  `save_note` persists tags.
- UI: sidebar filter chips (folders + tags with counts; Clear button); tag
  editor row in the note header (Enter/comma adds a tag, × removes it).
- Evidence: 56 tests green (34 core + 10 integration + 7 crypto + 1 server +
  4 sync); svelte-check clean; frontend build OK.

### M2 — command palette (2026-10-16)
- `Ctrl+P` / `Ctrl+K` opens a command palette: new note, save, theme switch,
  trash, rebuild index, pin/unpin, and a note switcher; arrow-key navigation,
  Enter runs, Esc closes. Rebuilding the index reports stats in a toast.
- Evidence: svelte-check clean (0 warnings); frontend build OK.

### M2 — attachments (2026-10-16)
- Paste an image/file into the editor, or use the palette's "Attach file…":
  bytes are stored in `<vault>/attachments/<slug>-<hash8>.<ext>` (deduped by
  content hash, 25 MB cap) and a relative Markdown link is inserted at the
  cursor. Commands: `save_attachment` (bytes), `import_attachment` (path).
- Evidence: 57 tests green (incl. an attachment-naming test in the app crate);
  svelte-check clean; frontend build OK. OS drag & drop is not wired yet
  (paste + file dialog only).

### M2.5 — preferences & polish (2026-10-16)
- Preferences: opaque `settings` object in `<app-data>/config.json` with new
  `get_settings` / `save_settings` commands (object-checked, 256 KiB cap,
  atomic temp+rename writes; ADR-0003). TS registry
  `app/src/lib/settings.svelte.ts` owns schema/defaults/deep-merge; ~30 options
  across appearance (theme light/dark/system, 6 accent presets, UI scale),
  editor (font, size, line height, wrap, live preview, autosave delay), view
  (list density, word count, date format), search (search-while-typing,
  debounce), general (confirm delete, reopen last note) and advanced (data
  folder, rebuild index, reset all).
- `SettingsPanel.svelte`: filterable panel reachable from `Ctrl+,` and the
  command palette ("Open settings…"; the palette also fuzzy-matches individual
  options and deep-links into the panel). Live-apply, per-option reset,
  keyboard-shortcut recorder that rejects conflicts and reserved keys
  (`app/src/lib/keymap.ts`). All window shortcuts now dispatch through the
  keymap service, and the editor's save key follows the configured binding.
- Live search polish: debounce from settings (default 150 ms), stale-response
  guard, clear button, "Searching…" indicator, `<mark>` highlighting of titles
  and snippets, Enter commits, Esc clears.
- Logo: custom W mark (rounded square + stroke W) — `branding/writan-icon.svg`
  plus a 1024 px PNG master; in-app `Logo.svelte` renders it with the live
  theme accent; the top bar shows the logo only (wordmark stays in the OS
  window title). All bundle icons regenerated with `tauri icon`
  (Windows/macOS/iOS/Android).
- Session state: `settings.session.lastNoteId` reopens the last note; one-time
  migration off the legacy `writan-theme` localStorage key.
- Evidence: 59 tests green (new: `settings_roundtrip_and_defaults`,
  `config_without_settings_key_still_loads` in `app/src-tauri/src/state.rs`);
  fmt/clippy clean; svelte-check clean; frontend build OK.

### M2.5 — release build & measurement (2026-10-16)
- `npm --prefix app run tauri build` green after pinning `@tauri-apps/api`
  2.11.1, `@tauri-apps/plugin-dialog` 2.7.3 and `@tauri-apps/plugin-opener`
  2.5.5 (the release build treats mismatched npm/Rust Tauri versions as a
  hard error; `tauri dev` only warns).
- Artifacts: `writan-app.exe` 6.53 MB, `Writan_0.1.0_x64_en-US.msi` 3.29 MB,
  `Writan_0.1.0_x64-setup.exe` 2.41 MB.
- Measured: cold start → window ≈396 ms; idle ≈120 MB private / ≈249 MB
  working set across 7 processes (app 4.4 MB private) — the webview helpers
  are the whole weight, which is the motivation note behind ADR-0004.
- Notes: the bundle id `com.writan.app` ends in `.app` (Tauri warning;
  harmless for our targets, decide before M3 whether to rename — it moves
  `%APPDATA%\com.writan.app`); one 563 kB JS chunk (CodeMirror) triggers
  vite's size hint (gzip 195 kB; candidate for code splitting later).
- Lessons: close every running `writan-app` before `tauri build` — the
  bundler patches `writan-app.exe` between installers, so a running instance
  fails the build with `os error 32` / `os error 5`.
- Evidence: `svelte-check` 0/0, vite build OK, `tauri build` exit 0.

## Next actions

Next (order is a suggestion, not a constraint):
1. GTK4 spike (ADR-0004): minimal `app-linux/` opens a real vault through
   `writan-core` — libadwaita three-pane, GtkSourceView editing, preview
   pane; build/smoke in WSL2 + WSLg here; measure cold start, idle RAM,
   binary size; final ibus/fcitx5 IME acceptance on the user's Linux laptop;
   append the outcome to ADR-0004. Scaffold is written and green on Windows
   (`app-linux` stub + `spans`/`config` unit tests; 10 tests) — first Linux
   compile happens in WSL2 next (the WSL feature needs one reboot).
2. Perf budgets still open: cold start < 400 ms (≈396 ms measured), search
   < 30 ms at 10k notes (not measured yet), RAM < 150 MB idle (≈120 MB
   private measured; the working set is higher with WebView2).
3. Then M3 (sync v1 + E2EE): version-vector blobs to folder + S3 targets,
   bounded diff3 with visible conflict copies, recovery-key export, keychain
   unlock, N-device sync simulator in nightly CI. The primitives already exist
   in `writan-sync` (version vectors) and `writan-crypto` (Argon2id +
   XChaCha20-Poly1305); the native Linux shell (M3.5, ADR-0004) follows.

## How to run & verify

```sh
# core crates (from repo root)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# desktop app
npm --prefix app install
npm --prefix app run check     # svelte-check
npm --prefix app run build
npm --prefix app run tauri dev # opens the app window (first compile is slow)
```

## Environment notes (reference dev machine)

- Windows 10 LTSC 10.0.19043, i3-9100F, 7.88 GB RAM; rustc 1.99.1;
  Node 24.22 / npm 11.19; VS Build Tools (MSVC + Win11 SDK 22621); WebView2
  runtime installed.
- In fresh PowerShell sessions `cargo` may not be on PATH: use
  `%USERPROFILE%\.cargo\bin\cargo.exe`.
- PowerShell sometimes shows a cosmetic `Exited with code 1` /
  NativeCommandError when cargo writes progress to stderr — trust the
  `Finished` line and `$LASTEXITCODE`, not the noise.

## Gotchas learned

- After moving/renaming the checkout, stale absolute paths in `target/` can
  break Tauri build-script reads (`os error 3`). Fix: delete `target/debug/build`
  dirs matching `^(tauri|wry|writan-app)` (or full `cargo clean`), rebuild.
- Git prints LF→CRLF warnings on commit (`core.autocrlf`); harmless.
- Tauri 2 converts camelCase JS arguments to snake_case Rust parameters
  automatically.
- App state (`config.json`, `index.sqlite`, `trash/`) lives in the OS app-data
  directory — never in the vault, never in the repo.
