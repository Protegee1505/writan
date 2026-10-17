# Writan — progress log

Living document: update it whenever a milestone or notable task lands.
Plans and exit criteria live in `docs/PLAN.md`; agent instructions in `AGENTS.md`.

Last updated: 2026-10-17 — ADR-0005 (sync blob format + key hierarchy) accepted as M3 starts, and the sealed blob codec, bounded-diff3 merge engine, folder sync engine and keyring/recovery-code layer have landed green in CI; search latency budget measured in CI release (≈5.6 ms avg / 6.2 ms max at 10k notes, vs. the 30 ms budget) and the bundle id renamed to `com.writan.desktop`; YAML frontmatter parser swapped to `serde-saphyr` (drops the archived `unsafe-libyaml`; deny ignore removed; ADR-0002 amended); CI remains the Rust compiler for this box (rustup minimal + rustfmt only). Prior: CI fully green at `c8dddbe`; artifacts workflow green — Windows installers + Linux GTK shell binary (3.09 MB, cold start 166 ms, idle RSS ≈131 MB headless/software-rendered); M2.5 release verified; workspace MSRV is Rust 1.90; Linux goes native via ADR-0004.

## Current state

- Branch `main`. **M0 complete, M1 complete, M2 desktop MVP (first pass) complete,
  M2.5 preferences & polish complete, release build verified. M3 (sync v1 +
  E2EE) underway — ADR-0005 (sync blob format + key hierarchy) accepted, and the
  blob codec, merge engine (bounded diff3 + LWW window), folder sync engine and
  keyring/recovery-code layer landed and are green in CI (2026-10-17).**
  M2 remaining: IME acceptance now rides on the GTK4 shell (ADR-0004); the
  runtime perf budgets are measured (see the Pre-M3 entries). Then M3.5
  (native Linux shell, ADR-0004).
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
- This dev box is re-imaged regularly. On 2026-10-17 it carries Rust 1.99.1
  via rustup **minimal + rustfmt** only — deliberately *no* VS Build Tools, so
  local compiles are off the table; CI (Windows + Ubuntu) compiles, clippy-checks
  and tests everything, locally only `cargo fmt`, `cargo fetch` (lockfile),
  `cargo metadata` and the npm frontend checks run.
- Workspace MSRV is now Rust 1.90 (was 1.85) so `cargo update` can take patched
  dependencies: `time` 0.3.55 and (via `plist` 1.10.1) `quick-xml` 0.42.0 fixed
  the three RustSec DoS advisories. `cargo-deny` advisories is green with
  documented ignores in `deny.toml` for the unfixable unmaintained crates
  (Tauri's `proc-macro-error` + the `unic-*` family). The `unsafe-libyaml`
  ignore is gone: `serde_yaml_ng` was replaced by `serde-saphyr` (2026-10-17).
- CI: `.github/workflows/ci.yml` (fmt/clippy/tests on Windows + Ubuntu, a
  release search-perf budget step, deny)
  doubles as the free Linux compiler for the GTK shell;
  `.github/workflows/artifacts.yml` builds downloadable Windows installers and
  the Linux binary + headless spike measurement (`workflow_dispatch` or a `v*`
  tag). WSL2 was shelved — the cafe test box resets on reboot; Linux
  measurements run in CI, interactive/IME acceptance on the user's laptop.
- GTK4 spike (M3.5) status: `app-linux/` compiles and its unit tests run on the
  ubuntu CI job (all jobs green at `c8dddbe`, run 38912122366); the artifacts
  workflow also ran green (run 38912602349) — Linux release binary 3.09 MB,
  headless Xvfb measure: cold start 166 ms, idle RSS ≈131 MB (software
  rendering; above the <100 MB budget — re-measure on real hardware).
  `writan-windows-x64` and `writan-linux-x64` are downloadable from that run
  (kept 90 days).
- The app has not been tested on Linux or Android interactively yet — the GTK4
  spike compiles and runs headless in CI; interactive feel + ibus/fcitx5 IME
  acceptance happen on the user's laptop.

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
- Notes: the bundle id `com.writan.app` ended in `.app` (Tauri warning;
  renamed to `com.writan.desktop` on 2026-10-17 — it moves
  `%APPDATA%\com.writan.app`); one 563 kB JS chunk (CodeMirror) triggers
  vite's size hint (gzip 195 kB; candidate for code splitting later).
- Lessons: close every running `writan-app` before `tauri build` — the
  bundler patches `writan-app.exe` between installers, so a running instance
  fails the build with `os error 32` / `os error 5`.
- Evidence: `svelte-check` 0/0, vite build OK, `tauri build` exit 0.

### M3.5 — GTK4 spike scaffold + CI builds (2026-10-16)
- `app-linux/` scaffold committed (`bd556ae`): libadwaita three-pane shell,
  GtkSourceView Markdown editor, rendered preview pane, XDG config reuse,
  `WRITAN_MEASURE=1` harness (prints cold start + idle RSS, then quits);
  10 unit tests (`spans` parser + `config`).
- The first real Linux compile (ubuntu CI) caught the crate aliases: gtk4 and
  libadwaita export library names `gtk4` / `libadwaita`. The workspace now
  renames them to the in-code names (`gtk = { package = "gtk4", … }`,
  `adw = { package = "libadwaita", … }`), so `app-linux` code reads
  `gtk::` / `adw::` like the ecosystem examples.
- MSRV 1.90 + `cargo update -p time -p plist -p quick-xml`: three RustSec
  advisories resolved (time 0.3.55, plist 1.10.1, quick-xml 0.42.0);
  `deny.toml` carries documented ignores for the unfixable unmaintained
  crates (`proc-macro-error`, `unic-*`; the `unsafe-libyaml` entry was removed
  when the YAML parser swap landed on 2026-10-17).
- Two `collapsible_if` clippy nits in `writan-core` (`index.rs`, `vault.rs`)
  collapsed into let-chains (edition 2024).
- `.github/workflows/artifacts.yml`: `windows` job (portable exe + MSI + NSIS
  setup) and `linux` job (release binary + `measure.txt` from an xvfb run);
  Android parked for M5 (needs `tauri android init` + SDK/NDK first).
- Evidence: 69 tests green on Windows; the ubuntu job iterated to green after
  five small fix rounds (GTK API nits, borrow fixes, one let-chain; final
  commit `c8dddbe`, run 38912122366 — windows + ubuntu + deny all pass).
- Spike measurements (run 38912602349, ubuntu-latest, headless Xvfb +
  `GSK_RENDERER=cairo` software rendering): release binary **3.09 MB**;
  cold start → window **166 ms**; idle RSS **≈131 MB** (130.7 MB). Caveat:
  software rendering inflates RSS and there is no compositor — the <100 MB
  idle budget needs a real-hardware re-measure with a GPU.
- Artifacts from the same run: `writan-windows-x64` (portable exe + MSI + NSIS
  setup) and `writan-linux-x64` (binary + `measure.txt`).

### Pre-M3 — frontmatter parser swap (2026-10-17)
- `serde_yaml_ng` 0.10 → `serde-saphyr` 2.0.0 (saphyr lineage, pure Rust,
  `default-features = false`, `features = ["serialize", "deserialize"]`);
  the archived `unsafe-libyaml` leaves the tree and `deny.toml`'s
  RUSTSEC-2024-0449 ignore is removed; ADR-0002 amended.
- Frontmatter now round-trips through a `serde_json` `Map`/`Value` with
  `preserve_order` enabled, so unknown keys *and their order* survive
  rewrites; empty `---\n---` frontmatter means "no metadata" (explicit guard
  instead of relying on the parser's empty-input behavior).
- Emitter quoting: strings a YAML 1.1 reader would take for timestamps or
  numbers are quoted on write, so `created`/`updated` and titles like `123`
  stay strings; UUID note ids stay unquoted.
- 4 new tests lock the contract (workspace count 69 → 73): empty frontmatter
  block, key-order preservation, block-scalar round-trip, numeric-looking
  string round-trip.
- Evidence: commit `749ab2d`, CI run 39002472683 — `deny` green (1m2s), tests
  green on ubuntu-latest (2m35s) and windows-latest (3m12s): 73 passed,
  0 failed.

### Pre-M3 — search latency budget verified (2026-10-17)
- New `crates/writan-core/tests/search_perf.rs` (ignored by default; CI runs
  it release-only) builds a 10 000-note in-memory index (~12–15 s) and times
  five warm query shapes over 21 runs each after a warm-up. Expectations sit
  below the 50-hit default result cap in `Index::search`.
- First release measurements of the <30 ms budget at 10k notes (averages:
  ubuntu-latest / windows-latest): common word 5.57 / 5.80 ms, tag + word
  5.17 / 5.51 ms, two-word phrase 0.29 / 0.33 ms, rare word 0.30 / 0.34 ms,
  no match 0.05 / 0.05 ms; worst max 6.15 ms — ≈5× headroom.
- CI: new release-only step after the debug suite
  (`cargo test -p writan-core --release --test search_perf -- --ignored
  --nocapture`); evidence commits `d1e9f47`, `2a877d7` (runner wiring
  `d7176f9`), CI run 39004075344 — all green.
- Same stretch: bundle id `com.writan.app` → `com.writan.desktop` lands
  (Tauri `.app`-suffix warning; app data moves to
  `%APPDATA%\com.writan.desktop`), commit `d33ea18`.

### M3 — sync format ADR (2026-10-17)
- ADR-0005 fixes the blob layout
  (`items/<note-uuid>/<device-uuid>/<counter>.enc`, `tombstones/…`, append-only
  keyring generations `keys/<n>.json`), the sealed payloads (writer's full
  version vector + note fields inside the blob), the AAD scheme (kind byte ‖
  note id), and the key hierarchy (random vault key; Argon2id KEKs for password
  *and* recovery code; recovery export mandatory before the first keyring).
- Create-only immutable blobs + per-device counters + a
  seal-once/retry-same-bytes rule make uploads crash-safe on dumb targets (no
  rename needed).
- Local-only sync state (device id, vectors, merge base, seal cache) stays in
  app-data, never in the vault; a target is refused if it sits inside the vault
  or carries a different `vault_id`.
- Next chunks build on it: blob codec → merge engine → folder target + engine →
  keys/recovery → app integration (+ N-device simulator).

### M3 — blob codec, merge engine, folder sync (2026-10-17)
- Sealed blob codec in `writan-sync` (`blob.rs`): note-version and tombstone
  payloads, AAD binding (kind byte ‖ note id), strict blob-path parsing —
  commit `945adfb`, CI run 39005193370.
- Merge engine behind `MergeStrategy` (`merge.rs`): contents first, then a ~60 s
  LWW window, then bounded diff3 (≤5 000 lines / 1 s, via `diffy`), otherwise
  an explicit conflict copy; resolved plans claim the union of both vectors —
  commits `6461c2e`, `978f636`, run 39006129804.
- Folder `SyncTarget` + reconciliation engine (`target.rs`, `engine.rs`):
  create-only writes, per-device counters, sync state in app-data (vectors,
  bases, digests); the integration test drives two vaults end-to-end through
  create/edit, delete/resurrect, disjoint-edit merge, and a same-line conflict
  copy — commits `5d72b5a`, `d76f1df`, `fcb5740`, `e656c6f`, run 39008265493.
- Review + CI catches: vector dominance no longer discards an unpublished local
  edit (the strategy defers to contents; dominance stays the engine's storage
  decision), and a fresh conflict copy is marked as locally present so the same
  pass cannot tombstone it (`e656c6f`, pinpointed by a temporary state dump in
  CI).
- Next: keys & recovery (keyring generations, recovery-code export, keychain
  unlock), then app integration + the N-device simulator.

### M3 — keys and recovery codes (2026-10-17)
- `writan-crypto` gained `random_key()`; a new `writan-sync::keys` module holds
  append-only keyring generations (`keys/<n>.json`), per-purpose wraps bound by
  `keyring_aad`, and an unlock that walks generations newest-first, falling back
  when the newest is torn; a foreign `vault_id` is refused — commits `6bbe0b4`,
  `cae81ab`, run 39009026034.
- Recovery codes: 32 random bytes shown once as 52 Crockford base32 symbols in
  groups of four; decoding ignores case and separators, maps O→0 and I/L→1, and
  rejects non-zero padding. `bootstrap_keyring` refuses to write anything until
  the code round-trips, keeping the export mandatory before the first keyring
  (ADR-0005 §8). 10 new tests cover generation round-trips, torn-generation
  fallback, foreign-`vault_id` refusal and the on-disk file shape.
- The first CI run flagged one clippy lint (`collapsible_if` in the
  foreign-vault check) — fixed in `cae81ab`; the rerun is fully green.
- Next: the app wires it together — settings section, enable/unlock dialogs,
  keychain (Windows Credential Manager) unlock on start — then the N-device
  simulator in nightly CI.

## Next actions

Next (order is a suggestion, not a constraint):
1. GTK4 spike (ADR-0004): scaffold committed and green everywhere; ubuntu CI
   is now the Linux compiler (five fix rounds, green at `c8dddbe`) and the
   artifacts workflow produced the binary + measurements (see the M3.5
   section). Remaining: interactive smoke + IME acceptance (ibus + fcitx5) and
   a real-hardware idle-RAM re-measure on the user's Linux laptop; then append
   the final spike outcome to ADR-0004 and start the full shell (M3.5) after
   M3.
   Android (M5): run `npm --prefix app run tauri -- android init` with JDK 17
   + Android SDK/NDK, commit `app/src-tauri/gen/android`, then add an
   `android` job to `artifacts.yml` (debug APK upload).
2. Perf budgets (Windows desktop): cold start < 400 ms (≈396 ms measured),
   search < 30 ms at 10k notes (≈5.6 ms avg / 6.2 ms max measured in CI
   release — see the Pre-M3 entry), RAM < 150 MB idle (≈120 MB private
   measured; the working set is higher with WebView2). The Linux GTK
   idle-RAM re-measure rides on item 1.
3. M3 (sync v1 + E2EE) is underway: the blob codec, merge engine, folder
   sync engine and keyring/recovery-code layer have landed and are green in CI
   (see the M3 entries). Next: the app wires sync together (settings, dialogs,
   keychain unlock) + the N-device simulator in nightly CI; an S3 target
   follows. The native Linux shell (M3.5, ADR-0004) follows M3.

## How to run & verify

```sh
# core crates (from repo root)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# search perf budget at 10k notes (release; ignored test)
cargo test -p writan-core --release --test search_perf -- --ignored --nocapture

# desktop app
npm --prefix app install
npm --prefix app run check     # svelte-check
npm --prefix app run build
npm --prefix app run tauri dev # opens the app window (first compile is slow)

# release installers (Windows) / Linux GTK shell (Linux only)
npm --prefix app run tauri build
cargo build --release -p writan-linux
```

## CI & builds

- `.github/workflows/ci.yml` (every push): `fmt` / `clippy -D warnings` /
  `test` on windows-latest + ubuntu-latest, a release-only `Search perf
  budget` step (10k-note latency test), plus `cargo-deny` (licenses, bans,
  advisories). The ubuntu job is the free Linux compiler for the GTK shell.
- `.github/workflows/artifacts.yml` (Actions tab → "Run workflow", or push a
  `v*` tag): publishes downloadable artifacts — `writan-windows-x64`
  (portable exe, MSI, NSIS setup) and `writan-linux-x64` (release binary +
  `measure.txt` with cold-start and idle-RSS numbers, captured headless under
  xvfb). Download with
  `gh run download -R Protegee1505/writan -n writan-windows-x64` (artifact
  names as above); artifacts are kept for 90 days.
- Fresh machine (the cafe test box resets on reboot): the durable workspace is
  the repo + CI. Push edits and let CI build/verify; this box (2026-10-17)
  carries rustup minimal + rustfmt (for `cargo fmt` and lockfile updates via
  `cargo fetch`) and Node — nothing else. Install MSVC Build Tools only if
  interactive `tauri dev` is wanted locally.
- Android is parked for M5 — exact init steps under Next actions.

## Environment notes (reference dev machine)

- Windows 10 LTSC 10.0.19043, i3-9100F, 7.88 GB RAM; rustc 1.99.1 (rustup
  minimal + rustfmt; no MSVC Build Tools as of 2026-10-17 — compiles happen in
  CI); Node 24.22 / npm 11.19; WebView2 runtime installed.
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
