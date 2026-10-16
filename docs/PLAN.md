# Writan — implementation plan

Status: M0 in progress (2026-10-16). Stack rationale: `docs/adr/0001-stack.md`.

## Milestones and exit criteria

| # | Scope | Exit criteria |
|---|-------|---------------|
| M0 | Bootstrap (0.5–1 wk) | Rust + MSVC toolchain on Windows; git repo; workspace (`writan-core/-crypto/-sync/-server` + `app/`); CI (fmt, clippy `-D warnings`, tests, cargo-deny) on Windows and Ubuntu; ADR-0001. |
| M1 | Vault core (2–3 wk) | Vault open/create; note CRUD on Markdown files with frontmatter (`id`/`title`/`tags`); SQLite index + FTS5 (trigram for CJK) rebuildable from files; file watcher reconciles external edits; trash + pinning. |
| M2 | Desktop MVP (3–4 wk) | Tauri app: three-pane UI (folders/tags, note list, editor); CodeMirror 6 live preview; search; themes; attachments; shortcuts + command palette; IME test on Windows/Linux passes. One-week Freya spike at the end decides if native-Skia UI is warranted. |
| M3 | Sync v1 + E2EE (2–3 wk) | Version vectors; encrypted blob upload/download to folder + S3 targets; bounded diff3 merge with visible conflict copies; recovery-key export; keychain unlock; N-device sync simulator green in nightly CI. |
| M4 | noteserver (1–2 wk) | axum + SQLite server: email+password and QR pairing, blob API, SSE change events; Docker image; deploy doc for free tiers (Oracle Always Free caveat: idle reclaim — run companions or switch to PAYG). |
| M5 | Android (2–3 wk) | Tauri Android build (NDK r27+), responsive layout, share-target capture, Keystore-backed unlock, opt-in 15-min WorkManager sync; APK attached to releases (F-Droid metadata later). |
| M6 | Hardening & release (1–2 wk) | Perf budgets verified; MSI/.deb/AppImage/APK pipelines; desktop updater (not on Android); export/backup; LICENSE files; docs. |

Total estimate: ≈12–18 weeks part-time.

## Performance budgets

- Cold start < 400 ms (desktop).
- Search < 30 ms at 10 000 notes.
- Desktop RAM < 150 MB idle.
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

In: live-preview Markdown editing, folders + tags, FTS incl. CJK, themes,
attachments, shortcuts + command palette, pinning, trash, export/import,
E2EE sync, Android share-target.

Out (v1): realtime collaboration, web app, plugin API, graph view, iOS,
AI/OCR features, FCM push.

## Known risks

- Linux WebKitGTK: contenteditable/IME bugs — validate in week 1 of M2;
  fallbacks: pin webkit version, split edit/preview mode, Freya spike.
- Android background limits: 15-min WorkManager floor is the realistic
  cadence; instant delivery would need FCM (out of scope).
- Windows 10 LTSC: WebView2 must be present (already installed on the
  reference machine); installers bundle the bootstrapper.
- Scope creep: any v1 wish beyond the list above goes to a backlog note,
  not into a milestone.
