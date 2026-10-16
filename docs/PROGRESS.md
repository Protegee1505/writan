# Writan — progress log

Living document: update it whenever a milestone or notable task lands.
Plans and exit criteria live in `docs/PLAN.md`; agent instructions in `AGENTS.md`.

Last updated: 2026-10-16 — M2: live preview, tags & folders, command palette landed; 56 tests green.

## Current state

- Branch `main`. **M0 complete, M1 complete, M2 desktop MVP (first pass) complete.**
  M2 remaining: command palette, attachments, IME/Freya check (see Next
  actions), then M3 (sync v1 + E2EE).
- Verification at the last commit: `cargo fmt --check` clean, `cargo clippy
  --workspace --all-targets -- -D warnings` clean, `cargo test --workspace`
  56 tests green, `svelte-check` 0 errors, frontend `npm run build` OK.
- The app has not yet had a full manual smoke run in the Tauri window, and has
  not been tested on Linux or Android at all.

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

## Next actions

M2 remaining (order is a suggestion, not a constraint):
1. Attachments (paste/drag into `attachments/`, relative links) — planned M2
   scope.
2. IME smoke test on Windows and Linux; decide on the 1-week Freya spike
   (M2 exit item).
3. Then M3 (sync v1 + E2EE): version-vector blobs to folder + S3 targets,
   bounded diff3 with visible conflict copies, recovery-key export, keychain
   unlock, N-device sync simulator in nightly CI. The primitives already exist
   in `writan-sync` (version vectors) and `writan-crypto` (Argon2id +
   XChaCha20-Poly1305).

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
