# Parity checklist — Tauri shell vs native Linux shell

ADR-0004 requires the two shells to stay in step. This is the list that
guards it: update it in the same commit as each Linux slice, and treat a
checked item as a promise verified by `tools/visual` (mock loop for the Tauri
shell, `scripts/gui-shot.sh` for the GTK shell) or a unit test.

Legend: `[x]` done · `[ ]` missing · `~` intentionally different (the note
says why).

| Feature | Tauri (Win/Android) | Linux GTK4 | Notes |
|---|---|---|---|
| Welcome / choose notes folder | [x] | [x] | |
| Three-pane layout | [x] | [x] | |
| Note list with live search (FTS, CJK) | [x] | [x] | |
| Folder filter | [x] | [x] | sidebar chips with counts; single active filter |
| Tag filter | [x] | [x] | sidebar chips with counts; single active filter; compact chips in one strip capped at 168 px (Tauri's `.filters` block is the model), so many tags scroll instead of pushing the note list down |
| Clear filters | [x] | [x] | active filter chips + `Clear` |
| Pinned notes first + pin/unpin | [x] | [x] | Linux: `●` marker in the list, pin button in the editor header |
| New note (Ctrl+N) | [x] | [x] | |
| Rename note | [x] | [x] | title entry commits on Enter or focus leaving |
| Delete note → trash + toast | [x] | [x] | Linux: two-step (arm, then delete) + toast |
| Trash panel: list/restore/purge/empty | [x] | [x] | Linux: modal window; both shells have `Empty Trash` (Tauri gained it in the sweep) |
| Editor: Markdown editing | [x] | [x] | |
| Editor header: title entry, word count, pin, delete | [x] | [x] | same `settings.view.showWordCount` default (Linux reads it in `config.rs`) |
| Edit note tags (add/remove) | [x] | [x] | chips + an Add-tag entry in the editor header; both shells save through core `Workspace::set_tags` (trim, `#`, dedupe) |
| Editor: inline live preview | [x] | ~ | Linux: highlight + preview pane (ADR-0004 §3) |
| Preview pane | ~ | [x] | Windows/Android: inline live preview instead; Linux renders local images inline, the Tauri preview shows the link |
| Autosave + dirty state | [x] | [x] | Linux reads `settings.editor.autosaveMs` (700 ms default) |
| Ctrl+S explicit save | [x] | [x] | |
| File watcher: external changes | [x] | [x] | Linux reloads the open note only while clean |
| Attachments: paste / drag-drop / import | [x] | [x] | rules shared via `writan-core::attachments` (25 MB cap, `attachments/<slug>-<hash8>.<ext>`); Linux: Ctrl+V (image or copied files), drop onto the editor (Tauri has no drag-drop), palette `Attach file…`; Linux images insert `![name](rel)` for paste and palette alike (Tauri's palette item always inserts a link) |
| Command palette | [x] | [x] | Linux: an in-window layer, not a window of its own (Escape, a click outside or losing focus dismisses it; nothing moves or resizes); extra `Choose notes folder…` when no vault is open; key hints show the live bindings; both palettes search the settings rows (`Settings: <label>` opens the panel filtered to that row) |
| Settings panel | [x] | [x] | same app-data `config.json` (ADR-0003); Linux live-applies each change (autosave delay, word count, date format, confirm delete, theme, accent, panel widths, reopen-last-note) and persists it immediately |
| Note context menu (right-click) | [x] | [x] | Duplicate · Rename… · Pin/Unpin · Copy note text · Copy file path · Move to trash; the row is selected first, so the menu acts on the note it names (the GTK menu's first entry takes focus, so Return duplicates) |
| Duplicate note | [x] | [x] | core `Workspace::duplicate_note`: same body, tags and extra frontmatter under a fresh id, `<title> copy`, its own file; the palette's `Duplicate this note` and the context menu both run it |
| Panel widths remembered | [x] | [x] | `settings.view.sidebarWidth` / `view.editorWidth` (Linux: drag a divider and it is written 400 ms later; both shells have the rows, `editorWidth` is GTK-only because Windows/Android have no second divider); GTK panes have hard floors (`shrink-*-child = false`, 200/320/280 px) so a drag can never collapse one away |
| Rebindable shortcuts | [x] | [x] | defaults from `app/src/lib/keymap.ts` (a unit test keeps the GTK table in step); Linux reads `settings.shortcuts` from `config.json` at startup and rebinds in the settings panel (Change, then press the keys; Backspace unbinds, Esc cancels; reserved and clashing combos are refused) |
| Theme: light/dark/follow-OS | [x] | [x] | Linux: `adw::StyleManager` color scheme, `system` follows the desktop (`toggle-theme` action live since slice 7) |
| Accent color choice | [x] | [x] | same six presets in both shells (the Linux list is drift-guarded against `settings.svelte.ts`); painted by a display-wide CSS provider that recolours switches, selections and suggested actions |
| Session restore (last note) | [x] | [x] | `settings.session.lastNoteId` written on every note open; `general.reopenLastNote` (default true) is a switch in the Linux General group |
| Sync (folder/S3, unlock, sync now) | [x] | [x] | one shared `writan-sync::session` layer (same config object, keychain entries and wording); GTK verified by `gtk-shell-test` checks 36–43 — recovery code shown once, password unlock, lock, two-click Turn off, and a second sandbox device joining the folder keyring and adopting notes |
| Sync to a writan-server (ADR-0007) | [x] | [x] | account sign-in or create-account, "Join with a pairing code", and a "Pair a device" QR card in both shells, backed by the same `writan-sync::server` calls (palette gains `Pair a device…`); wire-level tests are Rust-side (`server_sync.rs`, `writan-server/tests/*`), both panels compile-checked |

## M3.5 slices

1. Watcher + autosave — done (2026-10-17).
2. Sidebar filters, pinned first, pin/unpin, rename, delete — done (2026-10-17).
3. Trash panel: list/restore/purge/empty — done (2026-10-17); Linux also has `Empty Trash` (Tauri extra to add in the sweep).
4. Command palette + shortcut service — done (2026-10-17).
5. Settings panel — done (2026-10-17).
6. Attachments — done (2026-10-17).
7. Parity sweep + docs + measurement prep — done (2026-10-18): tags, appearance
   (theme + accent), session restore, the Tauri `Empty Trash`, GTK polish
   (empty states, `GTK_THEME` dark fallback, inline preview images).
8. Sync UI in the GTK shell — done (2026-10-18): folder and S3 targets,
   recovery code (shown once, Copy), password/recovery unlock, Sync now, lock,
   two-click Turn off; verified by `gtk-shell-test` checks 36–43.
9. Polish slice (user-reported) — done (2026-10-18): minimal tag chips and a
   quiet Add-tag entry, note context menus in both shells (with core
   `duplicate_note`), pane floors + remembered divider widths, the palette as
   an in-window layer that dismisses on an outside click (no popover, no
   window), settings search in both palettes, and a follow-up on the sidebar's
   facet strip: chips compact in hover and checked states too, all three rows
   in one 168 px strip that hugs the search field (`gtk-shell-test` checks
   44–46, 62/62 green; labels drift-guarded by a Rust test against
   `settings.svelte.ts`).
