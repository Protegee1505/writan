# Writan — implementation plan

Status: M0–M3 complete (2026-10-17) · M3.5 in progress — slices 1–3 done,
slice 4 (command palette + shortcut service) next.
History and evidence: `docs/PROGRESS.md`. Decisions: `docs/adr/README.md`. Stack
rationale: `docs/adr/0001-stack.md`. Workspace MSRV: Rust 1.90.

## Next tasks

Order is a suggestion, not a constraint.

1. **GTK4 acceptance on real Linux hardware** (the user's laptop): interactive
   smoke, IME (ibus + fcitx5, CJK composition), and the idle-RAM re-measure
   against the < 100 MB budget — then append the outcome to ADR-0004. This also
   closes M2's deferred IME item. Start from the fixture vault and screenshot
   loop in `tools/visual/README.md`.
2. **M3.5 full native shell** (`app-linux/`, ADR-0004): slices 1–3 done
   (watcher + autosave, sidebar filters + note actions, trash panel). Left:
   command palette + shortcut service, settings panel, attachments, then the
   parity sweep.
3. **Parity checklist** (`docs/PARITY.md`) vs the Tauri shell stays green as
   each M3.5 slice lands.
4. **Android init** (M5 prep, when the toolchain is available):
   `npm --prefix app run tauri -- android init` with JDK 17 + Android SDK/NDK,
   commit `app/src-tauri/gen/android`, then add an `android` job to
   `artifacts.yml` (debug APK upload).

## Upcoming milestones

| # | Scope | Exit criteria |
|---|-------|---------------|
| M3.5 | Native Linux shell (1.5–3 wk) | `app-linux/` GTK4 + libadwaita + GtkSourceView (ADR-0004): three-pane UI, Markdown highlighting + rendered preview pane, command palette, settings (same app-data `config.json`), trash, attachments; parity checklist vs the Tauri shell green; Linux idle RAM < 100 MB measured; IME verified with ibus + fcitx5 on real hardware. |
| M4 | noteserver (1–2 wk) | axum + SQLite server: email+password and QR pairing, blob API, SSE change events; Docker image; deploy doc for free tiers (Oracle Always Free caveat: idle reclaim — run companions or switch to PAYG). |
| M5 | Android (2–3 wk) | Tauri Android build (NDK r27+), responsive layout, share-target capture, Keystore-backed unlock, opt-in 15-min WorkManager sync; APK attached to releases (F-Droid metadata later). |
| M6 | Hardening & release (1–2 wk) | Perf budgets verified; MSI/.deb/Flatpak/AppImage/APK pipelines; desktop updater (not on Android); export/backup; LICENSE files; docs. |

Remaining estimate: ≈5.5–10 weeks part-time.

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
