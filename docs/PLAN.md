# Writan — implementation plan

Status: M3 in progress — ADR-0005 fixed the sync blob format + key hierarchy; the sealed blob codec, bounded-diff3 merge engine, folder sync engine and keyring/recovery-code layer (recovery-code export included) are landed and green in CI, the desktop app wires sync (settings, palette, keychain unlock), and the N-device simulator runs green in nightly CI (2026-10-17). M2.5 complete (2026-10-16); release build verified (exe 6.53 MB, ≈120 MB private idle RAM); search budget at 10k notes measured (≈5.6 ms avg vs 30 ms; release step in CI) and the bundle id is `com.writan.desktop`. Linux goes native: ADR-0004 (GTK4, no webview) — spike passed, full shell is M3.5. M2 exit items still open: IME acceptance on the GTK4 shell (the Linux idle-RAM re-measure rides on it). M1 complete (commit 24c2654). M0 complete (commit 0324dc5). Stack rationale: `docs/adr/0001-stack.md`. CI (`artifacts.yml`) publishes downloadable Windows installers + the Linux binary; workspace MSRV is Rust 1.90.
Living progress log: `docs/PROGRESS.md` · Agent guide: `AGENTS.md`.

## Milestones and exit criteria

| # | Scope | Exit criteria |
|---|-------|---------------|
| M0 | Bootstrap (0.5–1 wk) | Rust + MSVC toolchain on Windows; git repo; workspace (`writan-core/-crypto/-sync/-server` + `app/`); CI (fmt, clippy `-D warnings`, tests, cargo-deny) on Windows and Ubuntu; ADR-0001. |
| M1 | Vault core (2–3 wk) | Vault open/create; note CRUD on Markdown files with frontmatter (`id`/`title`/`tags`); SQLite index + FTS5 (trigram for CJK) rebuildable from files; file watcher reconciles external edits; trash + pinning. |
| M2 | Desktop MVP (3–4 wk) | Tauri app: three-pane UI (folders/tags, note list, editor); CodeMirror 6 live preview; search; themes; attachments; shortcuts + command palette; IME test on Windows passes. Linux editor/IME acceptance moved to the GTK4 shell (ADR-0004, spike gate); the Freya spike is dropped in its favour. |
| M2.5 | Preferences & polish (0.5–1 wk) | Settings panel (appearance / editor / view / shortcuts / search / general / advanced) reachable from the command palette and `Ctrl+,`; every option live-applies and persists in app-data `config.json` (ADR-0003); theme modes incl. follow-OS + accent + scale; editor font/size/line-height; rebindable global shortcuts with conflict detection; top-bar logo only; search filters while typing. |
| M3 | Sync v1 + E2EE (2–3 wk) | Version vectors; encrypted blob upload/download to folder + S3 targets; bounded diff3 merge with visible conflict copies; recovery-key export; keychain unlock; N-device sync simulator green in nightly CI. |
| M3.5 | Native Linux shell (1.5–3 wk) | `app-linux/` GTK4 + libadwaita + GtkSourceView (ADR-0004): three-pane UI, Markdown highlighting + rendered preview pane, command palette, settings (same app-data `config.json`), trash, attachments; parity checklist vs the Tauri shell green; Linux idle RAM < 100 MB measured; IME verified with ibus + fcitx5 on real hardware. |
| M4 | noteserver (1–2 wk) | axum + SQLite server: email+password and QR pairing, blob API, SSE change events; Docker image; deploy doc for free tiers (Oracle Always Free caveat: idle reclaim — run companions or switch to PAYG). |
| M5 | Android (2–3 wk) | Tauri Android build (NDK r27+), responsive layout, share-target capture, Keystore-backed unlock, opt-in 15-min WorkManager sync; APK attached to releases (F-Droid metadata later). |
| M6 | Hardening & release (1–2 wk) | Perf budgets verified; MSI/.deb/Flatpak/AppImage/APK pipelines; desktop updater (not on Android); export/backup; LICENSE files; docs. |

Total estimate: ≈15–22 weeks part-time (includes M2.5 and the native Linux shell).

## Performance budgets

- Cold start < 400 ms (desktop).
- Search < 30 ms at 10 000 notes.
- Desktop RAM < 150 MB idle (Windows/webview); the native Linux shell < 100 MB idle.
- No telemetry, ever.

## Sync model (summary)

- Per-note encrypted blobs: `items/<uuid>/<device>/<counter>.enc` + `tombstones/`.
- Dumb targets only: folder (Syncthing), S3-compatible (Backblaze B2 free 10 GB —
  reference backend; Cloudflare R2 alternative), WebDAV, writan-server.
- Merge: bounded three-way diff (max edit 5 000 lines / 1 s), ~60 s
  later-write-wins window for near-simultaneous edits, otherwise an explicit
  "conflict copy". Never silent overwrites. CRDT (Loro) is deliberately out of
  v1; merge logic stays behind a trait.
- E2EE: Argon2id (19 MiB, t=2, p=1) → KEK wrapping the vault key;
  XChaCha20-Poly1305 blobs with note id as AAD; mandatory recovery-key export.

## v1 scope / non-goals

In: Markdown editing (inline live preview on Windows/Android; Linux:
syntax highlight + rendered preview pane, ADR-0004), folders + tags, FTS
incl. CJK, themes
(incl. follow-OS + accent), preferences panel (fonts, text size, rebindable
shortcuts), attachments, command palette, pinning, trash, export/import,
E2EE sync, Android share-target.

Out (v1): realtime collaboration, web app, plugin API, graph view, iOS,
AI/OCR features, FCM push.

## Known risks

- Native Linux shell (ADR-0004): dual-frontend parity drift — mitigate with
  UI-thin shells (no logic outside core) + the parity checklist; GTK4 /
  libadwaita / GtkSourceView as system dependencies (Flatpak covers other
  distros); final IME acceptance needs real Linux hardware (user's laptop).
  The old WebKitGTK risk is retired for Linux.
- Android background limits: 15-min WorkManager floor is the realistic
  cadence; instant delivery would need FCM (out of scope).
- Windows 10 LTSC: WebView2 must be present (already installed on the
  reference machine); installers bundle the bootstrapper.
- Scope creep: any v1 wish beyond the list above goes to a backlog note,
  not into a milestone.
