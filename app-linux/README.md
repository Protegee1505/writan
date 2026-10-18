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

- config: `~/.config/com.writan.desktop/config.json` (same shape as the Tauri
  app: `vault_path` + opaque `settings` + the `sync` object; atomic writes —
  ADR-0003)
- data: `~/.local/share/com.writan.desktop/` (`index.sqlite`, `trash/`,
  `sync-state.json`)

## Status (2026-10-18)

M3.5 slices 1–9 are done and `docs/PARITY.md` is green: watcher + autosave,
sidebar filters + note actions, trash panel, command palette + rebindable
shortcuts, settings panel (live apply), attachments (paste, drop, palette
import), tags, appearance (theme + accent), session restore, inline preview
images, the sync UI (folder/S3 target, recovery code, unlock, sync now,
lock, turn off — on the shared `writan-sync::session` layer), and the polish
pass: quiet tag row, note context menus, pane floors + remembered divider
widths, the palette as an in-window layer with settings search, and a minimal
sidebar facet strip (compact chips in hover and checked states too, one 168 px
strip right under the search field).

Checks: `scripts/gtk-shell-test.sh` (62 checks, Xvfb — no real app data
touched); `WRITAN_MEASURE=1` prints the cold-start and idle-RSS numbers.
The real-hardware IME + idle-RAM acceptance is still on the list
(`docs/REAL-HARDWARE.md`).
