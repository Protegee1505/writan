# Writan — progress log

Milestone-level history and current state. Plans and upcoming tasks:
`docs/PLAN.md`. Decisions: `docs/adr/README.md`. Agent rules: `AGENTS.md`.

Last updated: 2026-10-19 (M4, M4.5, M5 slices 1 + 2a–2c; nightly sync soak fixed).

## Milestone history

- **M0 — bootstrap** (commit `0324dc5`): Rust + MSVC toolchain, cargo workspace
  (`writan-core`/`-crypto`/`-sync`/`-server` + `app/`), Tauri 2 + SvelteKit
  shell, CI (fmt, clippy `-D warnings`, tests, cargo-deny). ADR-0001. 18 tests.
- **M1 — vault core** (commit `24c2654`): Markdown + YAML frontmatter as source
  of truth, SQLite/FTS5 **trigram** index (rebuildable from files, CJK +
  substring), notify-based watcher, trash, workspace orchestration. ADR-0002.
  54 tests.
- **M2 — desktop MVP, first pass** (commit `bf17157`, later commits): three-pane
  Tauri UI, CodeMirror 6 editor + inline live preview, tags & folders, search,
  command palette, attachments, themes. 57 tests. IME acceptance deferred to the
  GTK4 shell (ADR-0004).
- **M2.5 — preferences & polish** (2026-10-16): settings registry (~30 options,
  live-apply, app-data `config.json`, ADR-0003), filterable settings panel,
  keymap service with rebindable shortcuts, logo/branding, session restore.
  Release build verified: exe 6.53 MB, MSI 3.29 MB, NSIS 2.41 MB; cold start
  ≈396 ms; idle ≈120 MB private / ≈249 MB working set (WebView2 helpers). 59
  tests.
- **M3 — sync v1 + E2EE** (2026-10-17): ADR-0005 (blob format + key hierarchy)
  and ADR-0006 (S3 target). Sealed blob codec, bounded-diff3 merge engine (~60 s
  LWW window, explicit conflict copies), folder target + sync engine,
  keyring/recovery codes, desktop app wiring (settings, palette, keychain
  unlock), S3-compatible bucket target. Three-device simulator green in nightly
  CI against folder and bucket targets. 73 tests.
- **M3.5 — GTK4 spike** (2026-10-16, gate passed): `app-linux/` compiles and its
  unit tests run on the ubuntu CI job; release binary 3.09 MB; headless measure
  (xvfb, software rendering) cold start 166 ms, idle RSS ≈131 MB. The full shell
  proceeds as M3.5; ADR-0004 carries the outcome.
- **Search latency budget verified** (2026-10-17): release-only `search_perf`
  test at 10 000 notes — ≈5.6 ms avg / 6.2 ms max vs the 30 ms budget (≈5×
  headroom); runs as a step in `ci.yml`.
- **Visual debugging harness** (2026-10-17): `tools/visual/` — action-script
  driver (`drive.mjs`, x11 + CDP backends), mock IPC backend for the frontend
  (`?mock=1`, `app/src/lib/mock.ts`), Xvfb screenshot script for the GTK shell
  and one for the browser mock, fixture vault (pinned/tags/CJK/folders), and
  `visual.yml` CI jobs: Linux GTK shell + Windows app (WebView2 over CDP, with
  `windows-probe.ps1` for launch diagnosis) with screenshot artifacts. The
  Windows job builds with `--features tauri/custom-protocol` (without it
  `tauri/build.rs` points the webview at `devUrl`) and injects
  `additionalBrowserArgs` into `tauri.conf.json` for that build only (wry sets
  the WebView2 property, which makes the env var variant a no-op). Evidence:
  commit `6237d86`, run `39026501469` (both jobs green).
- **Linux bundles in CI** (2026-10-17): new `linux-bundles` job in
  `artifacts.yml` (ubuntu-22.04, so the binaries ask for glibc ≤2.34 rather
  than an ubuntu-latest build's 2.39). Artifact `writan-linux-bundles` holds
  `.deb` 5.3 MB, `.rpm` 5.3 MB and `.AppImage` 83.6 MB (unsigned); all three
  ship `usr/bin/writan-app` + desktop entry + icons. deb/rpm depend on
  `libwebkit2gtk-4.1-0` and `libgtk-3-0`; the AppImage carries its own GTK +
  WebKit (241 MB unpacked) and only needs glibc. Evidence: commit `0adbc67`,
  run `39027221307` (3/3 jobs green).
- **Package metadata** (2026-10-17): `bundle.category`/`shortDescription`/
  `longDescription` set, so the desktop entry declares a category instead of
  `Categories=`; all three Linux bundles now ship
  `app/src-tauri/bundle/Writan.desktop.metainfo.xml` as
  `/usr/share/metainfo/` (AppStream), validated with `appstreamcli`. Its one
  warning is the non-RDNS component id — the desktop entry is `Writan.desktop`
  because Tauri names it after the product, so the AppStream id must match.
  New `release` job in `artifacts.yml`: on a `v*` tag it attaches every
  artifact plus `SHA256SUMS.txt` to that tag's GitHub release; manual runs
  skip it. Evidence: commit `ddbb17a`, run `39028319039` (3/3 jobs green, release
  job skipped as designed) — the `.deb` now declares `Section: utils`,
  `Categories=Office;` and carries the metainfo, and `rpm -qpl` plus an
  AppImage extraction show the same file in both.
- **M3.5 slice 1 — watcher + autosave in the GTK shell** (2026-10-17, commit
  `74be408`): `app-linux` follows the vault like the Tauri shell —
  `VaultWatcher` drained on a 200 ms main-loop poll, `reconcile_paths`, list
  refresh, and an open-note reload only while the buffer is clean (never
  mid-typing). Autosave reads `settings.editor.autosaveMs` (700 ms default,
  clamped 100–5000) from the app-data `config.json`, plus save on note switch,
  Ctrl+S and close; the header shows unsaved/saved state. New
  `scripts/gtk-shell-test.sh` (Xvfb + xdotool + sqlite3) proves autosave,
  external-edit reload without data loss, watcher indexing and list selection —
  5/5 green locally; `docs/PARITY.md` ticks both parity items.
- **M3.5 slice 2 — sidebar filters + note actions in the GTK shell** (2026-10-17,
  commit `4d64dad`): the sidebar grew folder and tag chips with counts
  (`workspace.folder_counts()`/`tag_counts()`), an active-filter row with its ×
  and `Clear`, and `refresh()` now feeds tag/folder into `SearchQuery` — only
  startup and vault switches fall back to opening the first row, so filtering
  never changes the open note. The editor header gained the title entry (Enter
  or focus leaving renames the note and its file on disk), word count
  (`settings.view.showWordCount`), a pin toggle (pinned notes sort first, `●`
  in the list) and a two-step delete (`settings.general.confirmDelete`) that
  moves the note to the app-data trash and clears the editor.
  `scripts/gtk-shell-test.sh` grew from 5 to 13 checks (filter hides/restores a
  note, pin row order, rename on disk, armed delete, editor cleared) — 13/13
  green locally; `docs/PARITY.md` ticks six more rows.
- **M3.5 slice 3 — trash panel in the GTK shell** (2026-10-17): the header
  trash button opens a modal `adw::Window` listing what `Trash::list()` holds,
  each row showing title, deletion time (`settings.view.dateFormat`, relative
  by default, via new `app-linux/src/dates.rs`) and original path, with
  `Restore` (reindex + list refresh) and `Delete forever`; `Empty Trash` sits
  in the panel header and Escape or the close button dismisses it. The time
  crate gained its `parsing` feature; `scripts/gtk-shell-test.sh` grew from 13
  to 17 checks (panel opens on the deleted note, Restore returns the file,
  purge and empty clear the trash) — 17/17 green locally.
- **M3.5 slice 4 — command palette + rebindable shortcuts in the GTK shell**
  (2026-10-17): new `app-linux/src/keymap.rs` holds the same nine actions as
  the Tauri keymap (`app/src/lib/keymap.ts`) — Ctrl+P (alias Ctrl+K), Ctrl+N,
  Ctrl+F, Ctrl+S, Ctrl+,; trash/pin/theme/sync start unbound — and a
  drift-guard unit test parses the TypeScript file so the two tables cannot
  drift silently. The shell installs its GTK accelerators from that table (the
  three hardcoded ones are gone), reads `settings.shortcuts` overrides from
  app-data `config.json` at startup, and refuses overrides that collide with
  reserved text-editing keys (Ctrl+C and friends) with a stderr warning.
  Ctrl+P opens a modal palette listing New note, Save now, Open trash, Rebuild
  search index, Pin/Unpin, the visible notes and — without a vault — the
  folder picker: filtered as you type, Up/Down/Enter/Escape, every row's hint
  showing its current binding. `scripts/gtk-shell-test.sh` grew from 17 to 24
  checks (palette opens, filters and opens a note, closes on Escape; a
  `settings.shortcuts` rebind takes effect on restart while Ctrl+P goes dead
  and the Ctrl+K alias survives; untouched defaults stay intact) — 24/24 green
  locally against the release binary.
- **M3.5 slice 5 — settings panel with live apply in the GTK shell**
  (2026-10-17): Ctrl+, (the keymap's `open-settings`, plus a header gear button
  and an `Open settings…` palette item) opens a modal panel with a filter box,
  a `No matching settings` state and `adw::PreferencesPage` groups — Editor
  (autosave delay 200–2000 ms), View (show word count, date format), General
  (confirm before deleting), Shortcuts (one row per keymap action: Change
  records the next chord, Backspace unbinds, Esc cancels, reserved or
  already-taken combos are refused with the reason on the row, Reset drops an
  override) and Advanced (data folder with Open, Rebuild search index, and a
  two-click Reset settings that clears `settings` from `config.json` and
  restores defaults). Every change persists immediately and live-applies —
  shortcut changes re-install accelerators at once, closing slice 4's "live
  rebind later" caveat. `app-linux/src/config.rs` gained
  `set_setting`/`remove_setting`/`set_shortcut`/`remove_shortcut`/
  `clear_settings` over one atomic writer, and `app-linux/src/keymap.rs`
  gained `binding_from_parts`/`binding_conflict` so a recorded chord is stored
  in the same `Ctrl+Shift+P` spelling as the Tauri panel (27 unit tests green).
  `scripts/gtk-shell-test.sh` grew from 24 to 31 checks (panel opens at
  620x560, the filter narrows to the Command palette row, a rebind recorded in
  the panel reaches `config.json` and takes effect while the old chord dies,
  Reset brings the default Ctrl+P back, Show word count off reaches
  `config.json`) — 31/31 green locally against the release binary.
- **M3.5 slice 6 — attachments in the GTK shell** (2026-10-17): the naming,
  25 MB cap and write rules moved out of the Tauri command into
  `crates/writan-core/src/attachments.rs` (`attachment_name`,
  `write_attachment`; `attachments/<slug>-<hash8>.<ext>`, identical bytes
  reuse the file), and `app/src-tauri/src/commands.rs` now delegates to it. The
  GTK editor takes attachments three ways: Ctrl+V with a clipboard image
  (`pasted-<hash>.png` via `GdkTexture::save_to_png_bytes`) or with copied
  files, drag-drop onto the editor (a Linux extra — the Tauri shell has none),
  and the palette's `Attach file…` through the GTK file dialog. Inserted links
  match the note's cursor: `![name](attachments/…)` for images (the shell reuses
  one insert path for paste and attach — Tauri's palette item always inserts a
  plain link, even for images), `[name](attachments/…)` otherwise.
  `scripts/gtk-shell-test.sh` grew from 31 to 36 checks (clipboard image paste
  stores + links, a second paste reuses the file, palette attach imports, text
  paste still falls through) — 36/36 green locally against the release binary.
- **M3.5 slice 7 — parity sweep + GTK polish** (2026-10-18, commit `27ea230`):
  the last three
  open parity rows closed. Tags: core gained `Workspace::update_note`/
  `set_tags`/`sanitize_tags` (Tauri's save path delegates to it too) and the
  GTK editor header gained tag chips plus an `Add tag…` entry (comma splits,
  Enter or focus-leaving commits, the chip's × removes). Appearance: a `Theme`
  combo (follow system / light / dark) drives `adw::StyleManager`, an
  `Accent color` combo offers the same six presets as
  `app/src/lib/settings.svelte.ts` (a drift-guard unit test parses the
  TypeScript table), and a display-wide CSS provider recolours switches,
  selections and suggested actions with the accent — CSS `@define-color` does
  not reach libadwaita's accent variables, so it paints the widgets directly;
  the shipped `toggle-theme` action is registered for the first time. Session
  restore: every note open writes `settings.session.lastNoteId`, startup
  reopens it while `general.reopenLastNote` (default true, new switch in the
  General group) is on, else the first row. Polish: `GTK_THEME`-based dark
  detection as a fallback, `No notes yet`/`No matching notes` empty states, an
  editor placeholder while no note is open, and the preview pane renders local
  images inline (texture cache, 460 px cap; http(s)/data links stay text).
  Tauri gained `empty_trash` (command, store action, panel button). The
  `scripts/gtk-shell-test.sh` fixture grew a note with an image link;
  checks went 36 → 48 (tag chip add and remove, theme and accent persistence,
  a pixel check that the accent repaints a live switch, an inline-image pixel
  check, restart-reopens-last-note and the reopen-off fallback) — 48/48 green
  locally against the release binary with `cargo fmt`/`clippy` clean,
  `cargo test --workspace` green, `svelte-check` 0/0 and the frontend build
  OK. The real-hardware routine (IME + RAM) is written down in
  `docs/REAL-HARDWARE.md`.

- **M3.5 slice 8 — sync UI in the GTK shell** (2026-10-18, commit `7c0af7e`,
  CI run `39093415280` green): the last open
  parity row. The shell-agnostic glue moved out of the Tauri app into
  `writan-sync::session` + `writan-sync::keychain` (sync config object,
  keychain services, connect/join, unlock, pass, status), so both shells share
  one implementation and one `config.json` sync object; the Tauri commands
  became thin DTO wrappers. The Settings panel gained a `Sync` group with the
  Tauri wording and states (not set up: folder/S3 target, password, Enable;
  recovery code shown once with Copy; locked: password or recovery code
  unlock; connected: target, short vault/device ids, Sync now, Lock, two-click
  Turn off, last sync + report line `N published · N merged · N adopted · N
  conflict copies · N deleted · N unchanged`); the pass runs on a worker
  thread (its own `Vault::open`, results drained by the 200 ms poll, then
  `Workspace::reconcile()` + refresh) and `sync-now` joined the palette.
  `scripts/gtk-shell-test.sh` went 48 → 56 checks: keyring creation and first
  pass, config + `sync-state.json`, clipboard recovery code, palette Sync now,
  a locked device publishes nothing, password unlock publishes again,
  two-click Turn off, and a second sandbox "device" that joins the keyring
  with the password and adopts notes — 56/56 green locally against the release
  binary. `cargo fmt`/`clippy -D warnings` clean, `cargo test --workspace`
  green (150 tests, 3 ignored), `svelte-check` 0/0, frontend build OK.

- **M3.5 slice 9 — user-reported polish in the GTK shell** (2026-10-18, commit
  `5035d9e`): five
  fixes from the user's review, landed with Tauri parity where the shells share
  a feature. Minimal tag row (`tag-chip`/`tag-entry`): borderless chips plus a
  quiet Add-tag entry, same look for the sidebar facet chips. A note context
  menu in both shells (Duplicate · Rename… · Pin/Unpin · Copy note text · Copy
  file path · Move to trash) on new core `Vault::duplicate` /
  `Workspace::duplicate_note` (`<title> copy`, fresh id and timestamps, its own
  file; `duplicate-note` is a bindable action in both keymaps). Panes cannot
  collapse or overflow (`shrink-*-child = false`, floors 200/320/160) and the
  dragged widths persist as `view.sidebarWidth` / `view.editorWidth`
  (`config.rs` load/save, Settings rows, Reset). The palette searches every
  setting (`SETTINGS_INDEX` shared with the panel filter and drift-guarded
  against `app/src/lib/settings.svelte.ts`; Enter reveals the row) and is now an
  in-window `gtk::Overlay` layer rather than a window or popover — the reported
  "it drags the window" bug, plus a popovers-with-entries keyboard strand found
  headlessly. `scripts/gtk-shell-test.sh` went 56 → 62 checks (44–46: row menu
  duplicate, palette settings reveal, divider persistence + pane floor) and is
  62/62 green here (run 7, exit 0); two harness fixes: the Settings window
  occludes the main one until closed (focus it before Escape), and an `ERR`
  trap now names the line of any `set -e` abort. Follow-up the same day (commit
  `72bb95b`): the sidebar's facet chips took the app's own hover/checked styles
  (libadwaita painted them 40 px/31 px tall for a 13 px label) and one strip
  capped at 168 px replaced the per-category scrollers that floated the chips
  ~75 px below the search field — run 9 is 62/62 green again. `cargo
  fmt`/`clippy -D warnings` clean, `cargo test --workspace` green, `svelte-check`
  0/0, frontend build OK.

- **M4 chunk A — noteserver accounts, tokens, storage** (2026-10-18, commit
  `57a0c57`, CI run `39094790760` green): new `crates/writan-server`: email +
  password accounts (Argon2id via `writan-crypto`, unknown emails pay the same
  derivation, 5 failures → 60 s lockout doubling to 15 min), device tokens
  (32 random bytes shown once, SHA-256 at rest), SQLite store (WAL, schema
  version in `meta`, one connection behind a mutex), `serve` /
  `reset-password` / `--version` CLI; 18 unit + 10 API + 3 CLI tests.
- **M4 chunk B — vaults, blob API, change feed** (2026-10-18, commit `23c543e`,
  CI run `39095140181` green): idempotent `PUT /api/v1/vaults/{id}` (one vault
  per account; foreign or unknown → 404), create-only blob PUT with
  `SyncTarget::write` semantics (identical → 204, different → 409; path rules,
  size and quota limits → 400/413/507) and `GET …/changes?since=` with
  per-vault monotonic seqs; 5 API tests + db/util unit tests.
- **M4 chunk C — SSE change stream** (2026-10-18, commit `fc20fa2`, CI run
  `39095480794` green): `GET …/events` replays the backlog from
  `Last-Event-ID`/`since` and follows live writes over a broadcast channel; a
  lagged reader self-heals by re-reading the REST feed; a test against the real
  binary covers replay, live delivery, resume, 401 and a bad cursor.
- **M4 chunk E — server sync target + connect flow** (2026-10-18, commit
  `61173d3`, CI run `39096935170` green): `writan-sync::server` speaks ADR-0007
  as a `SyncTarget` over `ureq` (create-only PUT with 204/409 mapping, lazy
  vault discovery, cross-vault writes refused), `SyncKind::Server` +
  `connect_server` (register or join, keyring exchange, token in the OS keychain
  `com.writan.desktop.server`, vault id assigned by the account, the vault key
  never reaches the server), and `serve_blocking` so a test runs the real server
  in-process. The live round-trip test converges two devices through the server;
  the shells compile with `Server` in the kind list (the setup UI is chunk F).
- **M4 chunk D — pairing codes and the QR payload** (2026-10-18, commit
  `fc1629b`, CI run `39097641001` green): `POST /api/v1/pairings` mints a
  32-byte single-use code (SHA-256 at rest, 600 s TTL, expired rows pruned) and
  `POST /api/v1/pairings/redeem` registers the joining device and hands it its
  token. The code grants blob access only — the vault key never travels, so the
  joining device still needs the sync password or recovery code. The payload
  both shells share is compact JSON
  `{"format":"writan-pair/1","url":…,"code":…,"email":…}`
  (`pair_payload` / `parse_pair_payload`), and `join_with_code` joins a vault
  through it; a live test joins a second device by code and refuses a used one.
- **M4 chunk H — server setup and the pairing QR in both shells** (2026-10-18,
  commit `a491a55`, CI run `39099064379` green after the serve-test fix in
  `7a2cac3`): `writan-sync::qr` renders the payload as a module matrix or a
  standalone SVG (`qrcode` crate, EC level M). Tauri gains
  `sync_connect_server`, `sync_join_server` (paste the QR text) and
  `sync_pairing_code` (code + expiry + payload + SVG); the Svelte sync panel
  gains the writan-server kind, account sign-in with a create-account switch,
  the join-with-code path and a "Pair a device" card with the inline QR, the
  code and copy; the GTK shell adds the same rows and a pairing dialog that
  paints the matrix with cairo. The serve smoke test now polls `/health`
  instead of trusting the first TCP connect (a CI runner reset that window).
- **M4 chunk F — sync settings polish and palette entries** (2026-10-18, commit
  `7b42da1`, CI run `39113987376` green): the sync status DTO carries the
  writan-server url and email, so both shells prefill the server fields when the
  settings open; the Tauri palette's "Set up sync…" is no longer folder-only,
  and a connected server vault adds a "Pair a device…" entry (the GTK palette
  gains the same one, wired to the pairing dialog). `docs/PARITY.md` records the
  writan-server row for both shells, and the Svelte panel was eyeballed in the
  `?mock=1` browser loop.
- **M4 chunk G — Docker image and deploy doc** (2026-10-18, commit `39f6b5a`,
  CI run `39115161620` and Docker run `39115161634` green): a multi-stage
  `Dockerfile` (rust:1.99-slim-bookworm → debian:bookworm-slim, non-root
  `writan`, `/data` volume, plain HTTP behind a reverse proxy) with a cached
  dependency layer, `docker-compose.yml`, `.github/workflows/docker.yml`
  (builds every push; pushes `ghcr.io/<owner>/writan-server` on `v*` tags or a
  manual run) and the deploy doc `docs/SERVER.md` (env table, Caddy TLS,
  backups, upgrades, free-tier notes incl. Oracle's idle-reclaim caveat). The
  image was built and smoke-tested locally: health, account, vault, blob and
  change feed over the container, uid 10001, SQLite persisted in the volume.
  **M4 complete.**
- **Sync setup panel fix** (2026-10-19, commit `ccdd36d`): after Turn off the
  Tauri panel showed the Locked view (the target stays in the config to prefill
  the form) with no way back to the method picker — reported on Windows; setup
  now means `enabled`, as in the GTK shell
  (`app/src/lib/components/SyncSection.svelte:30`); the browser mock's
  `sync_disable` kept a stale `target: null`, which is why the mock loop never
  showed it (`app/src/lib/mock.ts:308`).
- **M4.5 — one-password sign-in + one-command server** (2026-10-19): ADR-0008
  removes account passwords: one password derives both the keyring KEK and an
  Argon2id verifier, and the server stores a salt plus SHA-256(verifier) and
  runs no KDF per request (schema v2; `db::open` migrates a v1 store, and a
  pre-v0.2 account is unlocked by one `reset-password` run, which revokes its
  devices). `writan-sync` gained bring-forward adoption of a keyring made with
  an older password, `change_password` (append the keyring generation first,
  then `POST /api/v1/account/verifier`; every other device must sign in again)
  and `new_recovery_code`. Both shells show one Password field, the
  bring-forward prompt, and Change password / New recovery code rows.
  Releases gain a `writan-server` binary (glibc 2.35 floor) and
  `scripts/server-install.sh` (SHA-256-verified download, systemd unit, Caddy
  `--domain`); a personal build can bake `WRITAN_DEFAULT_SERVER`. Version
  0.2.0, released as `v0.2.0` (PR #1). Verification: `cargo test --workspace`
  223 tests green (3 ignored), `svelte-check` 0/0, frontend build OK,
  `gtk-shell-test` 62/62 (its sync section drives the one-password flow end to
  end, including a second device joining the keyring).
- **M5 prep — Android project + arm64 debug APK in CI** (2026-10-19, commits
  `eb2c441`, `1e44acb`): `scripts/android-setup.sh` reproduces the toolchain
  (JDK 17, cmdline-tools, SDK 37, NDK 27.3.13750724) under `/opt` — Cloud
  Shell's home is 5 GB. `tauri android init` generated
  `app/src-tauri/gen/android` (AGP 9.3.1, Gradle 9.6.1, `com.writan.desktop`,
  minSdk 24, targetSdk 37), committed so CI skips init; the root `.gitignore`
  ignores only `gen/schemas`. `artifacts.yml` gains an `android` job (Temurin
  17, `setup-android` v4, NDK r27, aarch64) that uploads
  `writan-android-arm64` (51 MB artifact), and `release` now waits on it.
  Evidence: run
  [39218160803](https://github.com/Protegee1505/writan/actions/runs/39218160803);
  a local `tauri android build --debug --apk --target aarch64` produced an
  arm64-only `app-universal-debug.apk` (208 MB debug). Known gap: `keyring`
  resolves to its `mock` store on Android, so no key persistence until the
  M5 Keystore task.
- **M5 slice 1 — responsive phone shell + app-private vault** (2026-10-19,
  commit `e4b9668`): ADR-0009. One breakpoint (`(max-width: 700px)` →
  `:root[data-narrow]`, `app/src/lib/viewport.svelte.ts`): one pane at a time,
  the list returns as a drawer with a scrim, touch targets ≥44 px (rows ≥48),
  `100dvh` + `interactive-widget=resizes-content` for the keyboard, safe-area
  insets, and phone-sized settings/trash/palette surfaces; above 700 px the
  desktop three-pane layout is unchanged. `Status.platform` (from
  `std::env::consts::OS`, mocked by `?platform=android`) tells the shell it is
  on Android: the welcome card offers "Use this device's storage" and a new
  `open_default_vault` command opens `<app data dir>/vault`; folder-picking
  affordances are hidden there and sync setup says it waits for the Keystore
  slice. Harness: `drive.mjs` gained `eval`/`assert`, `mock-shot.sh --size`,
  and `tools/visual/actions/mobile.actions` walks the phone shell and the
  Android first run at 390×844 (8 artefacts, 0 failures; routine in
  `docs/ANDROID.md`). Local DoD green: fmt/clippy/223 tests (3 ignored),
  `svelte-check` 0/0, frontend build OK.
- **M5 slice 2a — Android share target** (2026-10-19, commit `218ef5a`, fix
  `44965db`): ADR-0010 (decisions 1–3). An in-repo Kotlin plugin
  (`app/src-tauri/gen/android/app/src/main/java/com/writan/desktop/WritanPlugin.kt`,
  package `com.writan.desktop`, registered by `register_android_plugin`) takes
  `ACTION_SEND` `text/plain` from `load()`/`onNewIntent()` and parks
  `{text, subject}` in SharedPreferences until Rust `take_shared` drains it —
  a share that arrives before a vault exists survives process death and
  becomes a note once one is open. `writan-core` derives title/body
  (`share::draft_from_share`: subject wins, else the first non-empty line with
  Markdown markers stripped, a lone URL becomes its host, 80-char cap;
  whitespace-only input rejected) and `Workspace::create_note_from_share`
  writes + indexes the file. The shell drains after `init`/vault adoption and
  on window focus; the mock backend takes `?share=…&subject=…` and
  `tools/visual/actions/mobile-share.actions` walks a first-run share (its own
  script: a third document load in one driver session races the mock dev
  server). Verification: full local DoD green (`cargo fmt`/`clippy` clean,
  237 tests incl. 11 share unit + 1 integration, svelte-check 0/0, frontend
  build OK) and both strict mock runs (`mobile.actions`, `mobile-share.actions`);
  the arm64 debug APK job compiles the plugin's Kotlin green
  ([39249980550](https://github.com/Protegee1505/writan/actions/runs/39249980550)).
- **Sync fixpoint fix — a merge must dominate both sides** (2026-10-19): the
  nightly soak's `m=1`-per-sweep loop was `LwwDiff3`'s fast-forward keeping one
  side's vector alone, so the version it published never covered the other
  device's and the pair re-merged the same note on every sweep
  (`crates/writan-sync/src/merge.rs`, `resolve`): all three fast-forward paths
  now claim the union of both vectors — the newer stamp standing for both when
  the content is equal. Regression: `crates/writan-sync/tests/fixpoint.rs`
  (fails without the fix) plus vector-dominance assertions in the merge tests;
  15/15 local release soak runs green where 5/15 failed before. The soak's
  failure dump also became self-explaining (per-file recorded vs on-disk
  digests, each target entry with its vector and digest; `note_digest` /
  `blob_digest` are public for it). Evidence: commit `bcf90ff`, nightly run
  [39240627614](https://github.com/Protegee1505/writan/actions/runs/39240627614)
  green on folder and bucket; ADR-0005 amended.
- **M5 slice 2b — Keystore-backed keychain** (2026-10-19, commit `c5f9112`):
  ADR-0010 decisions 4–5, plus the dated note in ADR-0009 §8. On Android
  `crates/writan-sync/src/keychain.rs` resolves to `keyring-core` with
  `android-native-keyring-store` (SharedPreferences + Keystore AES-GCM) behind
  `cfg(target_os = "android")`; desktop keeps `keyring` 3 and the service names
  (`com.writan.desktop.sync`/`.s3`/`.server`) are unchanged. Because
  tauri/tao/wry never initialize `ndk-context`, `Keyring.kt` (from
  `MainActivity.onCreate`) and `app/src-tauri/src/android.rs` hand it the
  `applicationContext` once — a later slice-2c call is a no-op. The manifest
  gains `android:allowBackup="false"` (a Keystore key cannot leave the device),
  Android sync setup offers S3/server only, and the "keystore bridge not built"
  hint is gone. Verification: `cargo check` + `clippy --target
  aarch64-linux-android` for `writan-sync`/`writan-app` clean; strict mock runs
  (`mobile.actions` incl. the new Android sync settings step, and
  `mobile-share.actions`) green; desktop DoD green (fmt/clippy, 237 tests,
  svelte-check 0/0, frontend build OK); `ci.yml`, the Docker image and the
  arm64 debug APK job all green
  ([39250831846](https://github.com/Protegee1505/writan/actions/runs/39250831846)).
- **M5 slice 2c — opt-in background sync** (2026-10-19, commit `<2c>`, ADR-0010
  decisions 7–8): `AppConfig.background_sync` (default off) and a `background`
  field on the sync status; the connected Android panel carries an On/Off row
  whose command (`sync_set_background`) stores the flag and asks the Kotlin
  plugin to schedule or cancel a unique WorkManager job (`writan-sync`,
  network-connected, 15-minute floor, `ExistingPeriodicWorkPolicy.UPDATE`,
  plus one immediate run when switching on). The worker hands the app-data
  directory to Rust (`SyncBridge.run` → `app/src-tauri/src/android.rs`), which
  reuses the live app's workspace and session when the process is up and
  otherwise opens the same state headlessly, unlocking from the Keystore; the
  JSON reply tells Kotlin success/retry/disabled, and a disabled reply cancels
  the job. Launch reconciles the schedule with `config.json` (a short retry
  while the plugin loads). Verification: desktop DoD green (fmt/clippy, 238
  tests, svelte-check 0/0, frontend build OK); `clippy --target
  aarch64-linux-android` for `writan-sync`/`writan-app` clean; strict
  `mobile-share.actions` run green incl. the background-sync toggle step; the
  arm64 debug APK job green.

## Current state

- Branch `main`; M0–M3 complete, M3.5 (native Linux shell) slices 1–9 done —
  `docs/PARITY.md` is green; only the real-hardware acceptance (IME + RAM)
  waits on the user's laptop (`docs/REAL-HARDWARE.md`). M4 noteserver complete
  (`crates/writan-server`, ADR-0007): accounts, blob API, change feed/SSE, the
  server sync target, pairing codes, the QR display and the settings wiring in
  both shells, and the Docker image + `docs/SERVER.md` (chunks A–H). M4.5
  complete (ADR-0008): one password for sign-in and keyring, keyring rotation,
  and a release server binary with a one-command installer. M5 slice 1 is done:
  the Svelte shell works one pane at a time at phone width and a fresh Android
  install opens an app-private vault (ADR-0009); the Android project is
  committed and `artifacts.yml` builds the arm64 debug APK. Slice 2a adds the
  share target: text/URL shares become notes, kept natively until a vault
  exists (ADR-0010). Slice 2b backs the sync secrets with the Android keystore,
  so a restart unlocks sync without the password; slice 2c adds the opt-in
  background sync — a 15-minute WorkManager job that runs the same pass on its
  own, off unless the panel says on.
  Verification: `cargo fmt`/`clippy` clean, `cargo test --workspace` 238 tests
  green, `svelte-check` 0/0, frontend build OK.
- Bundle id `com.writan.desktop` (was `com.writan.app`; app data lives under
  `%APPDATA%\com.writan.desktop`). Workspace MSRV: Rust 1.90; version 0.2.0
  since M4.5.
- CI is the compiler: this box re-images regularly and carries rustup minimal +
  rustfmt only — locally run `cargo fmt`, `cargo fetch`/`cargo metadata`
  (lockfile) and the npm frontend checks; CI compiles, lints and tests
  everything. The full local DoD works after one passwordless-sudo apt line
  (`libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev libgtk-3-dev
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`);
  run `sudo apt-get update` first — a stale index 404s the fetch.
- CI map: `ci.yml` (fmt/clippy/tests on windows-latest + ubuntu-latest, a
  release search-perf step, cargo-deny), `artifacts.yml` (Windows installers,
  `writan-linux-x64` binary + `measure.txt` + bundles, `writan-server-linux-x64`,
  `writan-android-arm64` debug APK; manual dispatch or a `v*` tag, kept 90
  days), `nightly.yml` (three-device sync soak, folder + bucket, cron
  `17 3 * * *`), `docker.yml` (the writan-server image; builds every push,
  pushes GHCR on `v*` tags or a manual run), `web.yml` (static SPA → GitHub
  Pages preview). The ubuntu job is the free Linux compiler for `app-linux/`;
  `artifacts.yml` and the visual workflow also cover the shell.
- Cloud-first dev: **Google Cloud Shell** is the agent host
  (`scripts/cloudshell-setup.sh`, `docs/CLOUD.md`); the free `opencode/*` models
  answer from its IP but are rate-limited from Codespaces' — the codespace
  (`.devcontainer/`) is the optional full-toolchain env (agent UI works, free
  models do not).
- Nightly bucket target uses `adobe/s3mock` with initial buckets: MinIO
  withdrew its public images and binaries (checked 2026-10-17).
- `deny.toml` carries documented ignores for unfixable unmaintained crates
  (Tauri's `proc-macro-error`, the `unic-*` family, the deprecated zbus 5.x
  family merged into zbus 6); RustSec advisories are resolved (time 0.3.55,
  plist 1.10.1, quick-xml 0.42.0) and the yanked `inout` was bumped to 0.2.4.

## Active blockers

- **GTK4 IME + Linux RAM on real hardware**: IME acceptance (ibus + fcitx5) and
  the idle-RAM re-measure against the < 100 MB budget (headless
  software-rendered measure is ≈131 MB) need the user's Linux laptop. The app
  has not run on Linux interactively yet — the spike runs headless in CI.
  The routine is in `docs/REAL-HARDWARE.md`; append the outcome to ADR-0004.
- **Android on-device acceptance**: the phone shell has only been driven in
  Chromium so far (`tools/visual/actions/mobile.actions`, `docs/ANDROID.md`);
  touch feel, the real on-screen keyboard and the app-private vault need the
  sideloaded `writan-android-arm64` APK on a phone — the routine is in
  `docs/ANDROID.md`.

## How to run & verify

```sh
# core crates (from repo root)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# search perf budget at 10k notes (release; ignored test)
cargo test -p writan-core --release --test search_perf -- --ignored --nocapture

# N-device sync simulator (release; ignored; bucket variant needs WRITAN_S3_*)
cargo test -p writan-sync --release --test simulator -- --ignored --nocapture three_devices_converge_through_a_shared_folder

# desktop app
npm --prefix app install
npm --prefix app run check     # svelte-check
npm --prefix app run build
npm --prefix app run tauri dev

# release installers (Windows) / Linux GTK shell (Linux only)
npm --prefix app run tauri build
cargo build --release -p writan-linux

# GTK shell headless checks (Xvfb; no real app data touched)
scripts/gui-shot.sh --bin target/release/writan        # walkthrough screenshots
scripts/gtk-shell-test.sh --bin target/release/writan  # 62-check shell acceptance run
```

## Environment & gotchas

- Reference Windows box: Win10 LTSC 10.0.19043, i3-9100F, 7.88 GB RAM; rustc
  1.99.1 (rustup minimal + rustfmt); Node 24.22 / npm 11.19; WebView2 runtime
  installed. In fresh PowerShell, `cargo` may need
  `%USERPROFILE%\.cargo\bin\cargo.exe`; a cosmetic `Exited with code 1` /
  NativeCommandError can appear when cargo writes to stderr — trust the
  `Finished` line / `$LASTEXITCODE`.
- Close every running `writan-app` before `tauri build`: the bundler patches
  `writan-app.exe` between installers, so a running instance fails the build
  with `os error 32` / `os error 5`.
- Tauri release builds hard-error on mismatched npm/Rust Tauri versions
  (`tauri dev` only warns): `@tauri-apps/api` 2.11.1, `plugin-dialog` 2.7.3,
  `plugin-opener` 2.5.5.
- After moving/renaming the checkout, stale absolute paths in `target/` break
  Tauri build-script reads (`os error 3`): delete `target/debug/build` dirs
  matching `^(tauri|wry|writan-app)` (or `cargo clean`), then rebuild.
- App state (`config.json`, `index.sqlite`, `trash/`, sync state, keys) lives in
  the OS app-data directory — never in the vault, never in the repo.
- Git LF→CRLF warnings on commit (`core.autocrlf`) are harmless. Tauri 2
  converts camelCase JS arguments to snake_case Rust parameters automatically.
- Android on a small box: the SDK/NDK belong outside `$HOME`
  (`scripts/android-setup.sh` puts them in `/opt/android-sdk`) and a debug
  build needs `GRADLE_USER_HOME` and `CARGO_TARGET_DIR` off the home partition
  too — Gradle's own cache overruns Cloud Shell's 5 GB home on its own.
- In CI, `android-actions/setup-android` must be `@v4`: v3 runs `sdkmanager
  tools`, and that package no longer exists, so the job dies before setting
  anything up (v4.0.2 fixed it).
