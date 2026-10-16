# app-linux — native GTK4 shell (spike)

Linux-only shell for Writan, per
[`docs/adr/0004-linux-native-shell.md`](../docs/adr/0004-linux-native-shell.md).
No webview: GTK4 + libadwaita + GtkSourceView; all logic comes from
`writan-core` (UI-thin rule).

## Build & run (Linux)

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev
cargo run -p writan-linux                  # last vault from config.json, else a folder picker
WRITAN_VAULT=~/Notes cargo run -p writan-linux
WRITAN_MEASURE=1 cargo run --release -p writan-linux  # prints cold start + idle RSS, then quits
```

On non-Linux targets the crate compiles to a stub; the pure modules
(`spans`, `config`) are unit-tested everywhere.

## State

- config: `~/.config/com.writan.app/config.json` (same shape as the Tauri
  app: `vault_path` + opaque `settings`; atomic writes — ADR-0003)
- data: `~/.local/share/com.writan.app/` (`index.sqlite`, `trash/`)

## Spike status (2026-10-16)

Working: note list with live search, GtkSourceView editor with Markdown
syntax highlighting, live preview pane (pulldown-cmark → GTK text tags),
save (Ctrl+S), new note (Ctrl+N), focus search (Ctrl+F), folder picker,
last-vault persistence, `WRITAN_MEASURE` timings.

Not yet (M3.5 work): file watcher, autosave, tag/folder filter chips, trash
UI, attachments, settings panel, keyboard-shortcut service, inline live
preview. Measurements live in the ADR.
