# ADR-0004: Native Linux shell (GTK4)

Status: accepted, spike-gated. Date: 2026-10-16.

## Context

- ADR-0001 chose Tauri 2 + the system webview for all platforms and kept a
  one-week Freya (native Skia) spike as the escape hatch if WebKitGTK editing
  problems turned out to be fatal on Linux (see `docs/PLAN.md`, known risks).
- Measured on the 2026-10-16 release build (Windows, WebView2): the app
  process itself is 4.4 MB private / 18.9 MB working set, but the webview
  helper processes bring idle totals to ≈120 MB private / ≈249 MB working
  set. The webview runtime is the entire weight of the app.
- The user wants the Linux app to be native — no webview — fast, good-looking
  and without bloat (2026-10-16).
- Only GTK4 offers the hard part off the shelf with first-class Rust
  bindings: GtkSourceView 5 is a battle-tested editor widget (Markdown
  highlighting, undo, soft wrap, drag & drop) and Linux IME runs through the
  system input methods (ibus/fcitx5) — the exact area where WebKitGTK was the
  risk. libadwaita supplies the modern native look, plus native dialogs,
  window/menu integration and accessibility.
- Rejected alternatives: Slint 1.18 (editable text has no per-span styling —
  a highlighted Markdown editor would be hand-built; ADR-0001 already noted
  the rich-text editing gap), Iced and egui (pure-Rust single binaries, but
  every widget — dialogs, palette, drag & drop, accessibility — would be
  custom, and "native look" becomes an invented look), Freya 0.5 (no text
  editing primitives at all), Dioxus-native/Blitz (renderer too young),
  Qt (not Rust-first).

## Decisions

1. Linux ships a **GTK4 shell** in a new `app-linux/` workspace member
   (gtk4-rs + libadwaita + GtkSourceView 5). No webview and no JS on Linux.
2. Tauri remains the shell for **Windows and Android**. `writan-core`,
   `writan-crypto` and `writan-sync` stay UI-agnostic; shells are views and
   hold no business logic (UI-thin rule, enforced at review time and by the
   parity checklist).
3. Editor on Linux: **GtkSourceView with Markdown highlighting plus a
   rendered preview pane** (toggle / side-by-side; comrak output mapped onto
   GTK text tags or widgets). CodeMirror-style inline live preview is not a
   v1 goal on Linux; it remains the Windows/Android affordance. Explicitly
   accepted by the user on 2026-10-16.
4. Settings and session state are the same opaque object in app-data
   `config.json` (ADR-0003); the shell follows the same schema and defaults.
   Nothing moves into the vault or into localStorage.
5. **Spike gate** (3–4 days): a minimal `app-linux/` that opens a real vault
   through `writan-core` without Tauri — libadwaita three-pane, GtkSourceView
   editing, preview pane — plus measured cold start, idle RAM and binary
   size. Build and smoke tests run in WSL2 + WSLg on the reference Windows
   box; final IME acceptance (ibus and fcitx5, CJK composition) happens on
   real Linux hardware (the user's laptop). The spike outcome is appended
   here; the full shell is milestone M3.5.
6. Linux distribution: system GTK4 via distro package or Flatpak (no bundled
   runtime). M6 adds `.deb` and Flatpak to the release pipelines.

## Consequences

- Two frontends to maintain. Mitigations: UI-thin rule (all logic stays in
  core), a parity checklist that lives in `docs/PARITY.md` (amended
  2026-10-17: moved out of `PROGRESS.md`, which stays a lean history), and
  shared command/DTO semantics with the Tauri shell.
- Linux idle RAM expected well below the webview figure (≈50–70 MB estimate,
  to be measured in the spike). Budget: < 100 MB idle.
- New system dependency on Linux: GTK4 / libadwaita / GtkSourceView 5
  (GNOME ships them; other distros need one package; Flatpak covers the
  rest). CI's Linux job builds `app-linux/` with cargo only — no webkit.
- The Linux WebKitGTK IME/contenteditable risk is retired. WebView2 on
  Windows and the Android webview are unaffected.
- Cost if the spike fails: ≈4 days of work; Linux stays on the Tauri shell.

## Spike outcome (2026-10-16)

- The `app-linux/` scaffold compiles and passes its unit tests on real Linux:
  the ubuntu CI job went green on run 38912122366 (`c8dddbe`) after five small
  fix rounds (crate alias names, `CssProvider` data API, `gtk::glib` re-export,
  text-tag setters, borrow fixes, one let-chain).
- Release binary: **3.09 MB** (`writan-linux-x64` artifact from run
  38912602349).
- Headless measurement (xvfb + `GSK_RENDERER=cairo`, software rendering):
  cold start → window **166 ms**; idle RSS **≈131 MB** (130.7 MB).
- Caveat: software rendering inflates RSS and the run has no compositor; the
  < 100 MB idle budget (Consequences) needs a real-hardware re-measure with a
  GPU before it can be judged. Interactive feel + ibus/fcitx5 IME acceptance
  remain on the user's laptop.
- Verdict: the gate is passed on both build and runtime viability — the full
  native shell proceeds as milestone M3.5 after M3 (sync v1 + E2EE).
