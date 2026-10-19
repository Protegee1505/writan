# Android (sideload builds)

The phone build is a Tauri 2 app whose frontend is the same Svelte shell as the
desktop one; the differences are storage (no folder picker — ADR-0009), the
responsive layout, and features still missing below. The project lives in
`app/src-tauri/gen/android` (committed).

## Get the APK

- CI: the **artifacts** workflow → `writan-android-arm64` (arm64 debug APK).
  On a `v*` tag it is also attached to the release. Debug builds only —
  release signing is still open.
- Locally, after `scripts/android-setup.sh` prints the exports:

  ```sh
  GRADLE_USER_HOME=/opt/gradle-home CARGO_TARGET_DIR=/opt/writan-target-android \
    npm --prefix app run tauri -- android build --debug --apk --target aarch64
  ```

  The APK lands under
  `app/src-tauri/gen/android/app/build/outputs/apk/**/`.

## Install

```sh
adb install -r app/src-tauri/gen/android/app/build/outputs/apk/**/app-*.apk
```

or copy the APK to the phone and tap it (allow installing from this source).

## First run

The welcome card offers **Use this device's storage**: the vault is created at
`<app data dir>/vault` and opened. There is no folder picker on Android —
`tauri-plugin-dialog` 2.7.3 has no folder API there — so the app owns the
folder. Notes are still plain Markdown + frontmatter on disk, but no other app
can read them yet (no SAF; export/backup is M6). Uninstalling the app deletes
the vault: keep a copy through sync or the desktop until export ships.

## What works in M5 slice 1

- One pane at a time: the list fills the screen until a note is open, then the
  editor does; the list comes back as a drawer over the editor.
- Search, tags and folders, pinning, trash, settings, theme, autosave.
- The on-screen keyboard resizes the layout instead of covering the editor
  (`interactive-widget=resizes-content` plus `100dvh`).

## Share target (M5 slice 2)

Writan appears in the system share sheet for text and links. A shared text (or
URL) becomes a note and opens: the share subject becomes the title, otherwise
the first non-empty line (Markdown markers stripped); a lone URL becomes its
host name. A share that arrives before a vault exists — first run, or the app
was killed — is parked natively and becomes a note as soon as the vault is
open, so nothing is dropped in favour of "set up the app first".

Images and other file shares are not handled in v1.

Without a phone, the flow runs in Chromium against the mock backend; either
walk `tools/visual/actions/mobile-share.actions` or open

```sh
?mock=1&platform=android&share=Hello&subject=From%20the%20share%20sheet
```

## Sync (M5 slice 2b)

Sync works on Android with the S3 and Writan-server targets. The vault key,
the S3 secret and the server token live in the app's Keystore-backed store
(`android-native-keyring-store`, ADR-0010 §4–5), whose entries survive a
restart, so sync unlocks without the password. The app context the store needs
is handed to Rust from `MainActivity.onCreate` (`Keyring.kt` → `android.rs`).
Android backup stays off (`android:allowBackup="false"`): a Keystore key cannot
leave the device, and the vault is app-private. The shared-folder target is
desktop-only — Android has no folder picker (ADR-0009 §2).

## Not yet

- **Background sync** — the opt-in 15-minute WorkManager run lands later in
  slice 2, on top of this Keystore unlock.
- **Hardware back button** — Tauri 2.11 exposes no hook; the drawer closes by
  tapping a row or the scrim. Follow-up in slice 2.
- Keyboard shortcuts, image/file shares, export/backup, release signing.

## Verifying without a device

The same shell runs in Chromium against the mock backend, with the fake phone
platform and a fresh-install state:

```sh
scripts/mock-shot.sh --serve --size 390x844 \
  --actions tools/visual/actions/mobile.actions --strict
```

That script asserts the layout (one pane, drawer, no desktop bar) and walks the
Android first-run flow. `scripts/mock-shot.sh --serve --size 390x844
--actions tools/visual/actions/mobile-share.actions --strict` walks the share
target (a parked share becomes the open note once the vault exists). What
these cannot check is touch feel, the real keyboard and Keystore behaviour —
that needs the sideloaded APK on a phone.
