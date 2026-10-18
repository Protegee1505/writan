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
| Tag filter | [x] | [x] | sidebar chips with counts; single active filter |
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
| Command palette | [x] | [x] | Linux: modal window; extra `Choose notes folder…` when no vault is open; key hints show the live bindings |
| Settings panel | [x] | [x] | same app-data `config.json` (ADR-0003); Linux live-applies each change (autosave delay, word count, date format, confirm delete, theme, accent, reopen-last-note) and persists it immediately |
| Rebindable shortcuts | [x] | [x] | defaults from `app/src/lib/keymap.ts` (a unit test keeps the GTK table in step); Linux reads `settings.shortcuts` from `config.json` at startup and rebinds in the settings panel (Change, then press the keys; Backspace unbinds, Esc cancels; reserved and clashing combos are refused) |
| Theme: light/dark/follow-OS | [x] | [x] | Linux: `adw::StyleManager` color scheme, `system` follows the desktop (`toggle-theme` action live since slice 7) |
| Accent color choice | [x] | [x] | same six presets in both shells (the Linux list is drift-guarded against `settings.svelte.ts`); painted by a display-wide CSS provider that recolours switches, selections and suggested actions |
| Session restore (last note) | [x] | [x] | `settings.session.lastNoteId` written on every note open; `general.reopenLastNote` (default true) is a switch in the Linux General group |
| Sync (folder/S3, unlock, sync now) | [x] | [x] | one shared `writan-sync::session` layer (same config object, keychain entries and wording); GTK verified by `gtk-shell-test` checks 36–43 — recovery code shown once, password unlock, lock, two-click Turn off, and a second sandbox device joining the folder keyring and adopting notes |

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
