# Real hardware check — GTK shell (M3.5)

What only a real desktop can answer, before the native shell counts as
accepted: interactive feel + IME (the item M2 deferred) and the idle-RAM
number against ADR-0004's < 100 MB budget (the headless 131 MB run used
software rendering). Do it on the Fedora laptop, then record the outcome as a
dated amendment to `docs/adr/0004-linux-native-shell.md`.

## Build or fetch the binary

- Build: `sudo dnf install gtk4-devel libadwaita-devel gtksourceview5-devel`,
  then `cargo build --release -p writan-linux`.
- Or download the `writan-linux-x64` artifact from the latest `artifacts.yml`
  run (the GTK shell binary plus measurements).

Use the fixture vault at `tools/visual/fixtures/vault/` for the walkthrough —
it has a pinned note, tags, folders, CJK text, an image link and a long body.
Copy it, or point Writan at your real notes folder if you prefer.

## 1. Smoke (first run)

- Fresh `config.json` (move `~/.config/com.writan.desktop/` aside), launch,
  choose the vault: welcome page → notes list.
- Type in a note: status shows "Unsaved changes" then "Saved"; quit and
  reopen: the same note is back (session restore).
- Pin, rename, the two-click delete, then restore, purge and Empty Trash.
- Search; tag and folder chips; add and remove a tag from the editor header.
- Palette (Ctrl+P), settings (Ctrl+,, theme/accent/reopen switch).
- Attachments: drag a file in, paste an image from the desktop, Ctrl+V plain
  text must still paste; the preview pane renders the note, images included.
- Edit the open note from a terminal: the shell reloads it without losing the
  cursor.

## 2. IME (ibus and fcitx5)

For each engine, with a CJK input method (e.g. pinyin) active:

- `ibus`: log in with ibus, `GTK_IM_MODULE=ibus` set.
- `fcitx5`: the laptop's daily setup, `GTK_IM_MODULE=fcitx` set.

In the editor (and in the title, search and tag entries): the preedit shows at
the cursor, candidates can be picked, the commit lands, backspace behaves, no
flicker. Record engines and versions tried — this closes M2's deferred item.

## 3. RAM re-measure (ADR-0004 budget)

- Configure a vault, then run `WRITAN_MEASURE=1 ./writan`: it prints
  `writan-measure cold-start-ms: N` when the window maps and
  `writan-measure idle-rss-kb: N` after three idle seconds, then quits.
- Also watch a normal session: `watch -n 5 'ps -o rss= -p $(pgrep -x writan)'`
  while typing and scrolling — RSS should settle, not creep (leak watch).
- With a GPU/compositor the number should come down from 131 MB; the budget
  is < 100 MB idle.

## 4. Record the outcome

Append to ADR-0004: machine, distro, GPU, session type (Wayland/X11), IME
engines and versions, both measure numbers, and anything that broke. Then
update `docs/PROGRESS.md` (tick the blocker) and `docs/PLAN.md`. Commit and
push — a failing IME or RAM result becomes a blocker entry, not a footnote.
