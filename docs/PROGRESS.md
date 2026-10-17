# Writan — progress log

Milestone-level history and current state. Plans and upcoming tasks:
`docs/PLAN.md`. Decisions: `docs/adr/README.md`. Agent rules: `AGENTS.md`.

Last updated: 2026-10-17 (M3 complete).

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
  skip it. Evidence: commit ``, run ``.

## Current state

- Branch `main`; M0–M3 complete, next milestone M3.5 (native Linux shell).
  Verification after M3: `cargo fmt`/`clippy` clean,
  `cargo test --workspace` 73 tests green, `svelte-check` 0/0, frontend build OK.
- Bundle id `com.writan.desktop` (was `com.writan.app`; app data lives under
  `%APPDATA%\com.writan.desktop`). Workspace MSRV: Rust 1.90.
- CI is the compiler: this box re-images regularly and carries rustup minimal +
  rustfmt only (no MSVC Build Tools since 2026-10-17) — locally run `cargo fmt`,
  `cargo fetch`/`cargo metadata` (lockfile) and the npm frontend checks; CI
  compiles, lints and tests everything.
- CI map: `ci.yml` (fmt/clippy/tests on windows-latest + ubuntu-latest, a
  release search-perf step, cargo-deny), `artifacts.yml` (Windows installers +
  `writan-linux-x64` binary and `measure.txt`; manual dispatch or a `v*` tag,
  kept 90 days), `nightly.yml` (three-device sync soak, folder + bucket, cron
  `17 3 * * *`), `web.yml` (static SPA → GitHub Pages preview). The ubuntu job
  is the free Linux compiler for `app-linux/`.
- Cloud-first dev: **Google Cloud Shell** is the agent host
  (`scripts/cloudshell-setup.sh`, `docs/CLOUD.md`); the free `opencode/*` models
  answer from its IP but are rate-limited from Codespaces' — the codespace
  (`.devcontainer/`) is the optional full-toolchain env (agent UI works, free
  models do not).
- Nightly bucket target uses `adobe/s3mock` with initial buckets: MinIO
  withdrew its public images and binaries (checked 2026-10-17).
- `deny.toml` carries documented ignores for unfixable unmaintained crates
  (Tauri's `proc-macro-error`, the `unic-*` family); RustSec advisories are
  resolved (time 0.3.55, plist 1.10.1, quick-xml 0.42.0).

## Active blockers

- **GTK4 IME + Linux RAM on real hardware**: IME acceptance (ibus + fcitx5) and
  the idle-RAM re-measure against the < 100 MB budget (headless
  software-rendered measure is ≈131 MB) need the user's Linux laptop. The app
  has not run on Linux interactively yet — the spike runs headless in CI.
  Append the outcome to ADR-0004.
- **Android untouched** (parked for M5): no `tauri android init` yet — needs
  JDK 17 + Android SDK/NDK.

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
