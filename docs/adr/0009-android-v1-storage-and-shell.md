# ADR-0009: Android v1 — app-private vault and a one-pane shell

Status: accepted. Date: 2026-10-19.

## Context

- M5 needs a sideloadable APK that is *usable*, not just installable. Two
  questions decide that, and both are storage/UX decisions rather than
  implementation detail.
- **Where do notes live on a phone?** The desktop first run asks the user to
  pick a notes folder. That cannot exist on Android: `tauri-plugin-dialog`
  2.7.3 implements only `pick_file`/`pick_files`/`save_file` there (no folder
  API), and reaching an arbitrary folder means SAF — a whole platform bridge
  (persisted URI permissions, DocumentFile I/O) that Tauri does not expose. A
  fresh install would otherwise stop at the welcome card forever.
- **How does a three-pane shell work at phone width?** The desktop layout is
  sidebar + editor side by side, with a configurable sidebar width and a
  `min-width: 200px` floor; at 390 px that leaves ~190 px of editor. Nothing in
  `app/src` knew about a breakpoint before this work (the only `matchMedia` use
  was the dark-mode preference).
- The vault invariant (ADR-0002) is "Markdown files on disk are the source of
  truth", and app state (config, index, trash, keys) must never live inside the
  vault — both must hold on Android unchanged.

## Decisions

1. **The Android v1 vault is app-private**: `<app data dir>/vault`, created on
   first run by a new Tauri command `open_default_vault` (idempotent: it opens
   what exists). The welcome card replaces "Choose notes folder" with "Use this
   device's storage"; folder picking affordances (the vault button, the
   "Change notes folder" menu item, the data-folder "Open" action) are hidden
   when the shell runs on Android.
2. **No SAF in v1.** Notes stay plain Markdown + frontmatter, but only the app
   can read them. Moving a vault between the phone and a computer is sync's or
   export's job, and export/backup lands in M6; `docs/ANDROID.md` says so
   plainly, because uninstalling the app deletes the vault.
3. **The shell learns its platform from the backend**: `Status` gains
   `platform: string` (`std::env::consts::OS`), and the shell keys the
   Android-only differences off `store.android`. The dialog plugin is not
   probed for capabilities — a string from the backend is honest, testable
   through the mock backend (`?platform=android`) and stable across plugin
   upgrades.
4. **One breakpoint, `(max-width: 700px)`, owned by one store**
   (`app/src/lib/viewport.svelte.ts`), which mirrors it onto
   `:root[data-narrow]`. CSS and the harness both read that attribute; no
   component writes its own media query.
5. **One pane at a time at narrow**: the list fills the window until a note is
   open, then the editor does; the list returns as a drawer over the editor
   with a scrim, closed by tapping a row, the scrim or the ☰ button. The
   current note survives a resize in either direction, and the desktop
   three-pane layout is untouched (no change above 700 px).
6. **Phone affordances follow the phone idioms**: ≥44 px tap targets (≥48 px
   note rows), the top bar keeps search, new note and an overflow menu that
   carries the desktop bar's actions, `100dvh` instead of `100vh`,
   `viewport-fit=cover` + safe-area insets, and
   `interactive-widget=resizes-content` so the on-screen keyboard resizes the
   layout instead of covering the editor.
7. **Android hardware back is not wired in v1**: Tauri 2.11 exposes no
   back-button hook; the drawer closes by tap. Recorded as a slice-2 follow-up
   rather than a private Kotlin channel.
8. **Sync setup is visible but marked not-yet-supported on Android** until the
   Keystore-backed unlock (M5 slice 2) replaces the keyring's mock store there.
   *(2026-10-19: M5 slice 2b does exactly that — the keychain is backed by
   `android-native-keyring-store` (`ADR-0010` §4–5), the settings hint is gone
   and the folder target is hidden on Android because there is no folder
   picker there.)*

## Consequences

- A sideloaded APK is usable end to end (list, editor, tags, trash, settings,
  theme) with no dead end at first run, and the vault still satisfies ADR-0002.
- The vault lives in app-private storage, so `noteCount`/data folder paths in
  the UI show a path no file manager can open; that is why the folder-opening
  affordances are hidden instead of shown-and-broken.
- The phone layout is verifiable without a device: the mock backend takes
  `?platform=android&welcome=1`, and `tools/visual/actions/mobile.actions`
  asserts the one-pane/drawer geometry at 390×844 in Chromium. Touch feel, the
  real keyboard and Keystore behaviour still need hardware.
- Migration to SAF (if it ever happens) means a new location for an existing
  vault; the ADR that adds it must describe that move. Nothing here blocks it:
  the vault is a plain directory and sync treats it as one.
