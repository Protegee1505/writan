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
| Folder filter | [x] | [ ] | |
| Tag filter | [x] | [ ] | |
| Clear filters | [x] | [ ] | |
| Pinned notes first + pin/unpin | [x] | [ ] | |
| New note (Ctrl+N) | [x] | [x] | |
| Rename note | [x] | [ ] | |
| Delete note → trash + toast | [x] | [ ] | |
| Trash panel: list/restore/purge/empty | [x] | [ ] | |
| Editor: Markdown editing | [x] | [x] | |
| Editor: inline live preview | [x] | ~ | Linux: highlight + preview pane (ADR-0004 §3) |
| Preview pane | ~ | [x] | Windows/Android: inline live preview instead |
| Autosave + dirty state | [x] | [x] | Linux reads `settings.editor.autosaveMs` (700 ms default) |
| Ctrl+S explicit save | [x] | [x] | |
| File watcher: external changes | [x] | [x] | Linux reloads the open note only while clean |
| Attachments: paste / drag-drop / import | [x] | [ ] | |
| Command palette | [x] | [ ] | |
| Settings panel | [x] | [ ] | same app-data `config.json` (ADR-0003) |
| Rebindable shortcuts | [x] | [ ] | defaults from `app/src/lib/keymap.ts` |
| Theme: light/dark/follow-OS | [x] | [x] | |
| Accent color choice | [x] | [ ] | |
| Session restore (last note) | [x] | [ ] | `settings.session` |
| Sync (folder/S3, unlock, sync now) | [x] | [ ] | after M3.5 core slices |

## M3.5 slices

1. Watcher + autosave — done (2026-10-17).
2. Sidebar: folder/tag filters, pinned first, pin/unpin, rename.
3. Trash UI.
4. Command palette + shortcut service.
5. Settings panel.
6. Attachments.
7. Parity sweep + docs + measurement prep.
