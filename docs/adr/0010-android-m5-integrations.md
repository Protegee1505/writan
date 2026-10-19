# ADR-0010: Android M5 integrations — share target, Keystore key storage, background sync

Status: accepted. Date: 2026-10-19.

## Context

- M5 slice 2 needs three platform integrations on Android, each with a
  non-obvious option: sharing text into a note, storing sync keys in the
  Android Keystore, and running sync in the background.
- **Share target.** The app should appear in the system share sheet and receive
  `ACTION_SEND` text. Tauri 2 has no first-party share plugin, and
  `tauri-plugin-sharetarget` 0.1.6 is GPL-2.0-or-later — unusable in this
  MIT/Apache-2.0 project. The Android side is small enough to own: read the
  intent, park the payload, hand it to Rust.
- **Sync keys.** `keyring` 3 (desktop) has no Android store and falls back to
  `keyring::mock`, so sync credentials would not survive a reboot (a recorded
  gap in `docs/PROGRESS.md`). `keyring` 4 fixes nothing: its `v1` API rejects
  Android outright. The alternative is the same crate split the app already
  uses (a store behind a trait), with a separate Android store.
- **Background sync.** Periodic, battery-aware scheduling on Android means
  WorkManager. The Tauri plugin ecosystem only offers long-lived foreground
  services (`tauri-plugin-background-service`), which is a different behaviour
  and a notification requirement.
- Constraints that bound all three: vault files stay the source of truth
  (ADR-0002), no app state inside the vault, E2EE parameters unchanged
  (ADR-0005), no telemetry, settings-first, and one small dependency over a
  framework where a platform primitive suffices.

## Decisions

1. **The share target is an in-repo Kotlin Tauri plugin**, `WritanPlugin`
   (package `com.writan.desktop`), registered with
   `register_android_plugin("com.writan.desktop", "WritanPlugin")` and reached
   from Rust through `PluginHandle::run_mobile_plugin_async`. The manifest adds
   an `ACTION_SEND` + `text/plain` filter next to MAIN/LAUNCHER (the activity
   already has `launchMode="singleTask"`), and `load()`/`onNewIntent()` park the
   payload in SharedPreferences — so a share that arrives before the vault
   exists survives process death, and `take` clears it exactly once.
2. **The pull happens after a vault is open.** Rust's `take_shared` returns
   `null` without draining the pending share when no vault is open; the shell
   calls it after `init`/vault adoption and again on window focus. A share is
   never answered with "open a vault first" — it becomes a note once one
   exists.
3. **Shared text is shaped by `writan-core`**, not the UI: `draft_from_share`
   (subject wins as title, else the first non-empty line with Markdown markers
   stripped; a single URL becomes its host; title capped at 80 chars;
   whitespace-only input rejected) and `Workspace::create_note_from_share`.
   Only text/URL in v1; images and files need attachment import and are out of
   scope.
4. **Android sync keys live in the Android Keystore** via
   `android-native-keyring-store` behind `keyring-core`, selected under
   `cfg(target_os = "android")` in `crates/writan-sync/src/keychain.rs`;
   desktop keeps `keyring` 3. The store's service/account names stay
   unchanged, so the visible behaviour matches desktop.
5. **ndk-context initialization is ours**: Tauri/tao don't initialize it, so a
   small JNI shim (Kotlin `Keyring.initializeNdkContext`, Rust
   `Java_com_writan_desktop_Keyring_initializeNdkContext`) initializes it from
   `MainActivity.onCreate` and from the worker before the first keychain use.
6. **Android auto-backup stays off** (add `android:allowBackup="false"`): the
   vault is app-private and Keystore-wrapped preferences cannot be restored on
   another device, so backup would only move an undecryptable copy of app
   state.
7. **Background sync is opt-in, Android-only, 15 minutes**: `background_sync:
   bool` in `AppConfig` (default `false`), a toggle in the sync settings, and a
   unique periodic WorkManager job (`writan-sync`, network-connected, policy
   `UPDATE`) plus a one-time run when it is enabled. The worker calls a JNI
   entry that runs the same sync pass as `sync_now`, serialized with it; when
   the app process is alive it uses the live workspace, otherwise it opens the
   vault headlessly. No notification permission; Doze may delay runs.
8. **The schedule is reconciled on launch**, not only when the toggle is
   flipped: app setup reads `background_sync` and schedules or cancels the job,
   so a reinstall or a cleared WorkManager database cannot leave the two out
   of step.
   *(2026-10-19, implementation: the launch reconcile retries briefly on a
   side thread because the Kotlin plugin may still be loading when setup runs;
   the worker cancels its own job when `config.json` says the opt-in is off,
   which also covers a schedule that outlived an uninstall of the toggle.)*

## Consequences

- One inbound path (share) and one background caller (worker) funnel through
  `writan-core`; the vault invariant, sync blob format and E2EE parameters are
  untouched.
- The Keystore keeps the sync key usable across reboots, like the desktop
  keyring; losing the phone loses the key, so keeping a recovery-key export
  available remains the safety net.
- R8/ProGuard must keep the plugin, worker and JNI classes (`@Keep`), since
  they are reached by name; the release build checklist in `docs/ANDROID.md`
  gains this line.
- The share flow is verifiable in Chromium through the mock backend
  (`?share=…`); real share sheets, Keystore and WorkManager still need a
  sideloaded APK on hardware.
- Hardware back remains unwired and image/file shares remain out of scope;
  both are follow-ups, not silent gaps (`docs/ANDROID.md`).
