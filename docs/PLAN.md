# Writan — implementation plan

Status: M0–M3 complete (2026-10-17) · M3.5 slices 1–9 done (2026-10-18): the
parity checklist is green; only the real-hardware acceptance remains (slice 9
was the user-reported polish pass: minimal tag chips, note context menus,
pane floors + remembered widths, the palette as an in-window layer with
settings search). M4
noteserver complete (2026-10-18): accounts, blob API, change feed + SSE, the
server sync target, pairing codes, the QR display and the settings wiring in
both shells, and the Docker image + `docs/SERVER.md` deploy doc (chunks A–H).
M4.5 one-password sign-in and server setup complete (2026-10-19): ADR-0008
verifier auth (schema v2, legacy migration), client keyring rotation
(change password, new recovery code, bring-forward on an older keyring), one
Password field in both shells, and the release binary + `server-install.sh`.
M5 Android started (2026-10-19): `app/src-tauri/gen/android` is committed and
CI builds an arm64 debug APK (`writan-android-arm64`); slice 1 landed — the
Svelte shell works one pane at a time at phone width, and a fresh install uses
an app-private vault instead of a folder picker (ADR-0009).
History and evidence: `docs/PROGRESS.md`. Decisions: `docs/adr/README.md`. Stack
rationale: `docs/adr/0001-stack.md`. Workspace MSRV: Rust 1.90.

## Next tasks

Order is a suggestion, not a constraint.

1. **GTK4 acceptance on real Linux hardware** (the user's laptop): interactive
   smoke, IME (ibus + fcitx5, CJK composition), and the idle-RAM re-measure
   against the < 100 MB budget — then append the outcome to ADR-0004. This also
   closes M2's deferred IME item. The routine is `docs/REAL-HARDWARE.md`; the
   fixture vault is in `tools/visual/README.md`.
2. **M3.5 full native shell** (`app-linux/`, ADR-0004): slices 1–9 done
   (watcher + autosave, sidebar filters + note actions, trash panel, command
   palette + rebindable shortcuts, settings panel with live apply, attachments,
   the parity sweep: tags, appearance, session restore, look polish, the
   sync UI: folder/S3/server targets, recovery code, unlock, sync now, lock,
   turn off, pairing QR, and the polish pass: quiet tag row, note context
   menus, pane floors + remembered divider widths, the palette layer with
   settings search). Left: nothing beyond the real-hardware acceptance
   (task 1).
3. **Parity checklist** (`docs/PARITY.md`) is green; keep it so in the same
   commit as every shell change.
4. **M5 slice 2 — share-target capture**: an Android share intent (text or
   URL) creates a note in the vault, then Keystore-backed unlock and the
   opt-in 15-minute WorkManager sync (M5 exit criteria below). Sideload the
   `writan-android-arm64` APK to check slice 1 on a phone first — the routine
   is `docs/ANDROID.md`.

## Upcoming milestones

| # | Scope | Exit criteria |
|---|-------|---------------|
| M3.5 | Native Linux shell (1.5–3 wk) | `app-linux/` GTK4 + libadwaita + GtkSourceView (ADR-0004): three-pane UI, Markdown highlighting + rendered preview pane, command palette, settings (same app-data `config.json`), trash, attachments; parity checklist vs the Tauri shell green; Linux idle RAM < 100 MB measured; IME verified with ibus + fcitx5 on real hardware. |
| M4 | noteserver (1–2 wk) — **done 2026-10-18** | axum + SQLite server: email+password and QR pairing, blob API, SSE change events; Docker image; deploy doc for free tiers (Oracle Always Free caveat: idle reclaim — run companions or switch to PAYG). |
| M4.5 | one-password sign-in + one-command server (3 days) — **done 2026-10-19** | ADR-0008: one password derives both the keyring KEK and the sign-in verifier; the server stores a salt + verifier hash and runs no KDF per request (schema v2 migrates v1 stores; a pre-v0.2 account is unlocked by one `reset-password` run). Keyring rotation in `writan-sync` (`change_password`, `new_recovery_code`, bring-forward adoption of an older keyring). One Password field in both shells. Release `writan-server` binary + `scripts/server-install.sh` (SHA-256 verified, systemd, Caddy `--domain`). |
| M5 | Android (2–3 wk) | Tauri Android build (NDK r27+), responsive layout, share-target capture, Keystore-backed unlock, opt-in 15-min WorkManager sync; APK attached to releases (F-Droid metadata later). |
| M6 | Hardening & release (1–2 wk) | Perf budgets verified; MSI/.deb/Flatpak/AppImage/APK pipelines; desktop updater (not on Android); export/backup; LICENSE files; docs. |

Remaining estimate: ≈4.5–8 weeks part-time.

## Performance budgets

- Cold start < 400 ms (desktop).
- Search < 30 ms at 10 000 notes.
- Desktop RAM < 150 MB idle (Windows/webview); the native Linux shell < 100 MB idle.
- No telemetry, ever.

## Scope (v1)

In: Markdown editing (inline live preview on Windows/Android; Linux: syntax
highlight + rendered preview pane, ADR-0004), folders + tags, FTS incl. CJK,
themes (incl. follow-OS + accent), preferences panel (fonts, text size,
rebindable shortcuts), attachments, command palette, pinning, trash,
export/import, E2EE sync, Android share-target.

Out: realtime collaboration, web app, plugin API, graph view, iOS, AI/OCR
features, FCM push.

## Known risks

- Native Linux shell (ADR-0004): dual-frontend parity drift — mitigate with
  UI-thin shells (no logic outside core) + the parity checklist; GTK4 /
  libadwaita / GtkSourceView as system dependencies (Flatpak covers other
  distros); final IME acceptance needs real Linux hardware (the user's laptop).
- Android background limits: 15-min WorkManager floor is the realistic cadence;
  instant delivery would need FCM (out of scope).
- Windows 10 LTSC: WebView2 must be present (already installed on the reference
  machine); installers bundle the bootstrapper.
- Scope creep: any v1 wish beyond the list above goes to a backlog note, not
  into a milestone.
