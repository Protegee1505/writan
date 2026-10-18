#!/usr/bin/env bash
# Headless functional smoke test for the GTK shell (app-linux, M3.5).
#
#   scripts/gtk-shell-test.sh --bin /tmp/opencode/target/release/writan
#
# Runs the real app on an Xvfb display against a throwaway vault and checks
# the M3.5 behaviors end to end:
#   1. startup opens the remembered vault and shows its first note
#   2. typing lands in the Markdown file on disk (debounced autosave)
#   3. an external edit while the buffer is clean reloads the editor
#   4. a new file in the vault reaches the index and the list
#   5. the list opens the new note
#   6. the pin button pins the note and pinned notes sort first
#   7. a tag chip filters the list (and clearing it restores the rows)
#   8. renaming in the title entry renames the file on disk
#   9. the trash button arms once, then moves the note to the app-data trash
#  10. the header trash button opens the trash panel with the deleted note
#  11. Restore in the panel brings the file back into the vault
#  12. Delete forever purges the trashed copy for good
#  13. Empty Trash clears the remaining entries
#  14. Ctrl+P opens the command palette
#  15. typing 'delta' and Enter in it opens the note
#  16. Escape and a click outside dismiss it, with the window unmoved
#  17. a settings.shortcuts override leaves Ctrl+P dead
#  18. the rebound Ctrl+Shift+P opens the palette
#  19. the Ctrl+K alias still opens it
#  20. Ctrl+N (an untouched default) still creates a note
#  21. Ctrl+, opens the Settings panel
#  22. its filter narrows the list and a rebind writes settings.shortcuts
#  23. Escape closes it
#  24. the old shortcut is dead and the new one opens the palette
#  25. Reset in the panel drops the override
#  26. the default Ctrl+P opens the palette again
#  27. Show word count off reaches config.json
#  28. Ctrl+V with an image on the clipboard stores it under attachments/
#  29. pasting the same image again reuses the file
#  30. the palette's 'Attach file…' imports a picked file
#  31. pasted text still lands in the editor
#  32. the tag row adds a tag to the file and its chip removes it again
#  33. theme and accent choices land in config.json; the accent repaints a switch
#  34. a local image link renders inline in the preview pane
#  35. the app reopens the last note, and stops doing so when switched off
#  36. enabling sync creates the folder keyring and publishes the vault
#  37. the enabled sync lands in config.json and sync-state.json
#  38. Copy puts the recovery code on the clipboard
#  39. the palette's Sync now publishes a freshly created note
#  40. a locked device leaves the target alone
#  41. unlocking with the password publishes again
#  42. the two-click Turn off clears enabled in config.json
#  43. a second device joins the folder keyring and adopts notes
#  44. a note row's right-click menu duplicates the note
#  45. the palette finds a setting and opens the Settings panel on it
#  46. a dragged divider is remembered, and the pane floor holds
# Real app data is untouched: XDG dirs point into a temp directory, which is
# kept only when a check fails (or with --keep). Screenshots land in --out.
#
# Deps (one line on Ubuntu/Debian):
#   sudo apt-get install -y xvfb xdotool imagemagick sqlite3 xclip \
#     libgtk-4-1 libadwaita-1-0 libgtksourceview-5-0
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${WRITAN_BIN:-$repo/target/release/writan}"
out="${WRITAN_TEST_OUT:-/tmp/writan-gtk-test}"
display=":${WRITAN_TEST_DISPLAY:-94}"
keep=""

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) bin="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    -h|--help) sed -n '2,44p' "$0"; exit 0 ;;
    *) echo "gtk-shell-test: unknown option $1" >&2; exit 2 ;;
  esac
done

for tool in Xvfb xdotool import sqlite3 xclip; do
  command -v "$tool" >/dev/null || {
    echo "gtk-shell-test: $tool is missing; see the dep line in this script's header" >&2
    exit 1
  }
done
[ -x "$bin" ] || { echo "gtk-shell-test: not executable: $bin" >&2; exit 1; }

work="$(mktemp -d /tmp/writan-gtk-shell-test-XXXXXX)"
vault="$work/vault"
config_home="$work/config"
data_home="$work/data"
mkdir -p "$vault" "$config_home/com.writan.desktop" "$data_home" "$out"

pass=0
fail=0
note() { printf '%s\n' "$*"; }
ok() { pass=$((pass + 1)); note "ok   - $*"; }
bad() { fail=$((fail + 1)); note "FAIL - $*"; }

cleanup() {
  [ -n "${app_pid:-}" ] && kill "$app_pid" 2>/dev/null || true
  [ -n "${xvfb_pid:-}" ] && kill "$xvfb_pid" 2>/dev/null || true
  if [ "$fail" -gt 0 ] || [ -n "$keep" ]; then
    note "gtk-shell-test: kept $work (app log: $work/app.log)"
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT
# An unguarded command that fails aborts the script under `set -e`; without
# this trap the log would simply stop mid-check with no line to look at.
trap 'printf "gtk-shell-test: aborted at line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&2' ERR

# One pinned note, so startup always lands on it (list order: pinned, updated).
cat >"$vault/alpha.md" <<'EOF'
---
id: aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa
title: Alpha
tags: [smoke]
pinned: true
created: 2026-10-01T09:00:00Z
updated: 2026-10-01T09:00:00Z
---

alpha body line.

Second paragraph.
EOF
# A note whose body starts with a local image link, for the inline preview.
mkdir -p "$vault/attachments"
convert -size 120x60 xc:tomato "$vault/attachments/shot.png"
cat >"$vault/shot.md" <<'EOF'
---
id: 55555555-5555-4555-8555-555555555555
title: Shot
created: 2026-10-01T09:30:00Z
updated: 2026-10-01T09:30:00Z
---

![shot](attachments/shot.png)

An inline image.
EOF
printf '{"vault_path": "%s"}' "$vault" >"$config_home/com.writan.desktop/config.json"

Xvfb "$display" -screen 0 1280x800x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb_pid=$!
export DISPLAY="$display"
# A cold xkbcomp cache can keep Xvfb busy for a few seconds: wait for the
# display rather than for a fixed time, and say so when it never answers.
display_ready=""
for _ in $(seq 1 100); do
  if xdotool getdisplaygeometry >/dev/null 2>&1; then
    display_ready=1
    break
  fi
  sleep 0.2
done
if [ -z "$display_ready" ]; then
  echo "gtk-shell-test: Xvfb $display did not come up (see $work/xvfb.log)" >&2
  exit 1
fi

env \
  WRITAN_VAULT="$vault" \
  XDG_CONFIG_HOME="$config_home" \
  XDG_DATA_HOME="$data_home" \
  GTK_THEME="Adwaita:dark" \
  GSK_RENDERER=cairo \
  "$bin" >"$work/app.log" 2>&1 &
app_pid=$!

# The biggest visible window named "Writan" (GTK also opens tiny helper ones).
find_window() {
  local id best="" best_area=0 w h area geom
  while read -r id; do
    [ -n "$id" ] || continue
    geom="$(xdotool getwindowgeometry --shell "$id" 2>/dev/null || true)"
    w="$(sed -n 's/^WIDTH=//p' <<<"$geom")"
    h="$(sed -n 's/^HEIGHT=//p' <<<"$geom")"
    area=$(( ${w:-0} * ${h:-0} ))
    if [ "$area" -gt 1000 ] && [ "$area" -gt "$best_area" ]; then
      best="$id"
      best_area="$area"
    fi
  done < <(xdotool search --onlyvisible --name Writan 2>/dev/null || true)
  printf '%s' "$best"
}

window=""
for _ in $(seq 1 100); do
  window="$(find_window)"
  [ -n "$window" ] && break
  sleep 0.2
done
[ -n "$window" ] || { bad "app window never appeared (see $work/app.log)"; exit 1; }
# Park the window at a known size and place so clicks use stable coordinates.
xdotool windowmove "$window" 0 0
xdotool windowsize "$window" 1000 700
sleep 0.6
xdotool windowfocus --sync "$window"
xdotool windowraise "$window"

shot() { import -window "$window" "$out/$1.png" 2>/dev/null || true; }
shot_of() { import -window "$2" "$out/$1.png" 2>/dev/null || true; }
trash_window() { xdotool search --onlyvisible --name '^Trash$' 2>/dev/null | head -1; }
settings_window() { xdotool search --onlyvisible --name '^Settings$' 2>/dev/null | head -1; }

close_settings() { # close the Settings panel if one is up: it holds the clicks
  local settings
  for _ in $(seq 1 3); do
    settings="$(settings_window || true)"
    [ -n "$settings" ] || return 0
    xdotool windowfocus --sync "$settings" 2>/dev/null || true
    xdotool key --clearmodifiers Escape
    sleep 0.5
  done
  [ -z "$(settings_window || true)" ]
}

# The command palette is a layer inside the main window, not a window of its
# own: nothing to search for and nothing a window manager can move or resize.
# What it does is observable — while it is up it takes the keyboard, and Escape
# or a click outside dismisses it. These helpers type a throwaway marker and
# watch where it lands: the palette's entry leaves no trace, the editor writes
# it into the note on disk.
open_note_file() { # the file behind the note title in the window title
  local title
  title="$(xdotool getwindowname "$window" | sed 's/ — Writan$//')"
  grep -l "^title: ${title}$" "$vault"/*.md 2>/dev/null | head -1
}

frontmatter() { # the YAML block of a note: where its tags live
  sed -n '2,/^---$/p' "$1" 2>/dev/null
}

editor_gets_typing() { # marker -> 0 once the note on disk carries it
  local marker="$1" file=""
  xdotool windowfocus --sync "$window"
  xdotool type --delay 25 -- "$marker"
  for _ in $(seq 1 12); do
    file="$(open_note_file)"
    if [ -n "$file" ] && grep -q -- "$marker" "$file"; then
      xdotool key --clearmodifiers ctrl+z # leave the note as it was
      sleep 1.0
      return 0
    fi
    sleep 0.25
  done
  return 1
}

palette_open() { # 0 when Ctrl+P opens the palette; leaves it closed again
  local marker="palette-probe-$$-$RANDOM" file=""
  xdotool windowfocus --sync "$window"
  xdotool key --clearmodifiers ctrl+p
  sleep 0.8
  xdotool type --delay 25 -- "$marker"
  sleep 0.4
  xdotool key --clearmodifiers Escape
  sleep 0.4
  for _ in $(seq 1 8); do
    file="$(open_note_file)"
    if [ -n "$file" ] && grep -q -- "$marker" "$file"; then
      xdotool key --clearmodifiers ctrl+z # the editor got it: no palette
      sleep 1.0
      return 1
    fi
    sleep 0.25
  done
  return 0
}

wait_title() { # substring, seconds
  local want="$1" deadline=$(( $(date +%s) + $2 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    case "$(xdotool getwindowname "$window")" in *"$want"*) return 0 ;; esac
    sleep 0.2
  done
  return 1
}

# One palette round trip: open it with `key`, type `text`, press Enter and see
# which note the window ends up on. Without the palette the text would land in
# the editor and Enter would only add a line, so the title proves both.
open_note_with() { # key binding, search text, expected title substring
  xdotool windowfocus --sync "$window"
  xdotool key --clearmodifiers "$1"
  sleep 0.8
  xdotool type --delay 30 -- "$2"
  sleep 0.6
  xdotool key --clearmodifiers Return
  sleep 0.8
  case "$(xdotool getwindowname "$window")" in *"$3"*) return 0 ;; esac
  xdotool key --clearmodifiers Escape
  return 1
}

open_note_via_palette() { # search text, expected title substring
  open_note_with ctrl+p "$1" "$2"
}

run_palette_command() { # search text: open the palette, filter, press Enter
  xdotool windowfocus --sync "$window"
  xdotool key --clearmodifiers ctrl+p
  sleep 0.8
  xdotool type --delay 30 -- "$1"
  sleep 0.6
  xdotool key --clearmodifiers Return
  sleep 0.8
}

if wait_title "Alpha — Writan" 15; then
  ok "startup opened the remembered vault and showed its first note"
else
  bad "window title is '$(xdotool getwindowname "$window")', wanted 'Alpha — Writan'"
fi
shot 00-startup

# 2. Typing autosaves to the file on disk (700 ms debounce + save).
xdotool mousemove --sync 520 400
xdotool click 1
sleep 0.3
xdotool key --clearmodifiers ctrl+End
xdotool type --delay 25 -- " autosave-marker-a1"
sleep 2.5
if grep -q "autosave-marker-a1" "$vault/alpha.md"; then
  ok "typing reached alpha.md on disk without an explicit save"
else
  bad "autosave marker missing from alpha.md"
fi
shot 10-autosaved

# 3. An external edit while the buffer is clean reloads the editor: the typed
# text must append to the reloaded content, keeping the external marker.
printf '\nexternal-marker-b2\n' >>"$vault/alpha.md"
sleep 2.5
xdotool key --clearmodifiers ctrl+End
xdotool type --delay 25 -- " typed-after-reload-c3"
sleep 2.5
external_line="$(grep -n "external-marker-b2" "$vault/alpha.md" | head -1 | cut -d: -f1 || true)"
typed_line="$(grep -n "typed-after-reload-c3" "$vault/alpha.md" | head -1 | cut -d: -f1 || true)"
if [ -n "$external_line" ] && [ -n "$typed_line" ] && [ "$external_line" -lt "$typed_line" ]; then
  ok "external edit reloaded the editor and later typing kept both changes"
else
  bad "external edit handling lost data (external-marker at ${external_line:-none}, typed at ${typed_line:-none})"
fi

# 4. A new file in the vault reaches the index and the note list.
cat >"$vault/beta.md" <<'EOF'
---
id: bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb
title: Beta
tags: [smoke, other]
created: 2026-10-02T09:00:00Z
updated: 2026-10-02T09:00:00Z
---
 
beta body line.
EOF
sleep 2.5
count="$(sqlite3 "$data_home/com.writan.desktop/index.sqlite" \
  "select count(*) from notes where title='Beta'" 2>/dev/null || echo 0)"
if [ "$count" = "1" ]; then
  ok "the watcher indexed the new file"
else
  bad "index has $count notes titled Beta"
fi

# Click list rows until a row click *changes* the open note to the wanted one;
# the working y is printed. Comparing before/after titles keeps a no-op click
# on empty list space from looking like a hit when the note is already open.
open_note() { # title, y...
  local title="$1"
  shift
  local y before after
  for y in "$@"; do
    before="$(xdotool getwindowname "$window")"
    xdotool mousemove --sync 250 "$y"
    xdotool click 1
    sleep 0.6
    after="$(xdotool getwindowname "$window")"
    if [ "$after" != "$before" ]; then
      case "$after" in
        *"$title — Writan"*) printf '%s' "$y"; return 0 ;;
      esac
    fi
  done
  return 1
}

# The first list row that opens the note, scanning down from the list top.
first_row_for() { # title
  local title="$1" y before after
  for y in 170 190 210 230 250 270 290 310 330 350 370; do
    before="$(xdotool getwindowname "$window")"
    xdotool mousemove --sync 250 "$y"
    xdotool click 1
    sleep 0.4
    after="$(xdotool getwindowname "$window")"
    if [ "$after" != "$before" ]; then
      case "$after" in
        *"$title — Writan"*) printf '%s' "$y"; return 0 ;;
      esac
    fi
  done
  return 1
}

# 5. The list opens the new note (Beta is second: Alpha is pinned).
if beta_y="$(open_note Beta 233 185 281 329)"; then
  ok "the refreshed list opens the new note (Beta at y=$beta_y)"
else
  bad "clicking the list did not open Beta"
fi
shot 20-beta-selected

# 6. Pin: the editor toolbar pin writes through to the index, and pinned
# notes sort first.
xdotool mousemove --sync 723 77
xdotool click 1
sleep 1.2
pinned="$(sqlite3 "$data_home/com.writan.desktop/index.sqlite" \
  "select pinned from notes where title='Beta'" 2>/dev/null || echo "")"
if [ "$pinned" = "1" ]; then
  ok "the pin button pinned the note (index pinned=1)"
else
  bad "index pinned='$pinned' for Beta after clicking pin"
fi
# Scan order matters: a row click only shows up as a title change when that
# note is not the open one, so open Alpha first, then look for Beta.
alpha_first="$(first_row_for Alpha || true)"
beta_first="$(first_row_for Beta || true)"
if [ -n "$beta_first" ] && [ -n "$alpha_first" ] && [ "$beta_first" -lt "$alpha_first" ]; then
  ok "the pinned note sorts above the others (Beta at y=$beta_first, Alpha at y=$alpha_first)"
else
  bad "pinned sorting wrong (Beta at '${beta_first:-none}', Alpha at '${alpha_first:-none}')"
fi
shot 30-pinned

# 7. A tag chip filters the list: #other is Beta's alone, so Alpha must
# disappear; the active filter's × brings it back.
xdotool mousemove --sync 160 173
xdotool click 1
sleep 1.2
shot 40-tag-filter
alpha_filtered="$(first_row_for Alpha)" || alpha_filtered=""
if [ -n "$alpha_filtered" ]; then
  bad "the #other chip did not hide the other note (Alpha still clickable at y=$alpha_filtered)"
else
  ok "the #other chip filtered the list (Alpha is not in it)"
fi
xdotool mousemove --sync 52 116
xdotool click 1
sleep 1.2
shot 45-filter-cleared
alpha_y="$(first_row_for Alpha)" || alpha_y=""
if [ -n "$alpha_y" ]; then
  ok "clearing the filter brought the note back (Alpha at y=$alpha_y)"
else
  bad "clearing the tag filter did not restore the list"
fi

# 8. Rename from the title entry: Enter renames the note and its file.
open_note Beta 170 190 210 230 250 270 290 >/dev/null || true
xdotool mousemove --sync 420 77
xdotool click 1
sleep 0.3
xdotool key --clearmodifiers ctrl+a
xdotool type --delay 25 -- "Gamma"
xdotool key --clearmodifiers Return
sleep 1.5
renamed="$(find "$vault" -maxdepth 1 -name 'gamma-*.md' | head -1)"
if [ -n "$renamed" ] && [ ! -f "$vault/beta.md" ]; then
  ok "renaming in the title entry renamed the file on disk ($(basename "$renamed"))"
else
  bad "rename did not produce a gamma file (beta.md gone: "$([ -f "$vault/beta.md" ] && echo no || echo yes)")"
fi
shot 50-renamed

# 9. Delete: the trash button arms first (settings.general.confirmDelete
# defaults to true), the second click moves the note to the app-data trash.
file_before="$(find "$vault" -maxdepth 1 -name 'gamma-*.md' | head -1)"
xdotool mousemove --sync 768 77
xdotool click 1
sleep 0.6
if [ -n "$file_before" ] && [ -e "$file_before" ]; then
  xdotool click 1
  sleep 1.5
  if [ ! -e "$file_before" ]; then
    ok "the armed trash click moved the note out of the vault"
  else
    bad "the second trash click did not delete the note"
  fi
else
  bad "the first trash click deleted without the confirm step"
fi
trashed="$(find "$data_home/com.writan.desktop/trash" -name '*.md' 2>/dev/null | wc -l)"
if [ "${trashed:-0}" -ge 1 ]; then
  ok "the note landed in the app-data trash"
else
  bad "nothing landed in the app-data trash"
fi
case "$(xdotool getwindowname "$window")" in
  "Writan") ok "the editor cleared after the delete" ;;
  *) bad "window title after delete: '$(xdotool getwindowname "$window")'" ;;
esac
shot 60-after-delete

# 10. The header trash button opens the trash panel on the deleted note.
# Coordinates are calibrated for the 1000x700 test window (see the shots).
xdotool mousemove --sync 804 27
xdotool click 1
sleep 1.5
tw="$(trash_window || true)"
if [ -n "$tw" ]; then
  note "trash panel: $(xdotool getwindowgeometry --shell "$tw" | tr '\n' ' ')"
  ok "the header trash button opened the trash panel"
else
  bad "no window titled Trash appeared"
fi
shot 70-trash-panel
[ -n "$tw" ] && shot_of 71-trash-panel-window "$tw"

# 11. Restore brings the file back into the vault (and off the panel).
if [ -n "$tw" ]; then
  # The panel's first row (1000-wide screen: panel at 0,0): Restore at x=449.
  xdotool mousemove --sync 449 80
  xdotool click 1
  sleep 1.5
  restored="$(find "$vault" -maxdepth 1 -name 'gamma-*.md' | head -1)"
  if [ -n "$restored" ]; then
    ok "Restore brought the file back into the vault ($(basename "$restored"))"
  else
    bad "Restore did not bring the file back into the vault"
  fi
  shot_of 75-restored "$tw"
  xdotool mousemove --sync 613 26
  xdotool click 1
  sleep 1.0
fi

# 12. Trash it again, then Delete forever drops the copy from the trash.
if [ -n "$restored" ]; then
  open_note Gamma 170 190 210 230 250 270 290 >/dev/null || true
  xdotool mousemove --sync 768 77
  xdotool click 1
  sleep 0.6
  xdotool click 1
  sleep 1.5
  xdotool mousemove --sync 804 27
  xdotool click 1
  sleep 1.5
  tw="$(trash_window || true)"
  if [ -n "$tw" ]; then
    xdotool mousemove --sync 559 80
    xdotool click 1
    sleep 1.5
    shot_of 80-purged "$tw"
    purged="$(find "$data_home/com.writan.desktop/trash" -name '*.md' 2>/dev/null | wc -l)"
    if [ "${purged:-1}" = "0" ]; then
      ok "Delete forever dropped the trashed copy"
    else
      bad "trash still holds $purged note(s) after Delete forever"
    fi
    xdotool mousemove --sync 613 26
    xdotool click 1
    sleep 1.0
  else
    bad "the trash panel did not reopen for the purge check"
  fi
fi

# 13. Empty Trash: one more entry, then the header button clears it.
open_note Alpha 170 190 210 230 250 270 290 >/dev/null || true
xdotool mousemove --sync 768 77
xdotool click 1
sleep 0.6
xdotool click 1
sleep 1.5
xdotool mousemove --sync 804 27
xdotool click 1
sleep 1.5
tw="$(trash_window || true)"
if [ -n "$tw" ]; then
  xdotool mousemove --sync 533 27
  xdotool click 1
  sleep 1.5
  shot_of 85-emptied "$tw"
  left="$(find "$data_home/com.writan.desktop/trash" -name '*.md' 2>/dev/null | wc -l)"
  if [ "${left:-1}" = "0" ]; then
    ok "Empty Trash cleared the remaining entry"
  else
    bad "trash still holds $left note(s) after Empty Trash"
  fi
  xdotool mousemove --sync 613 26
  xdotool click 1
else
  bad "the trash panel did not reopen for the empty check"
fi

# 14-16. Command palette (a layer inside the main window, not a window of its
# own): Ctrl+P opens it, typing
# 'delta' filters and Enter opens that note, Escape and a click outside both
# dismiss it with the keyboard back on the editor, and neither moves or
# resizes the window.
cat >"$vault/delta.md" <<'EOF'
---
id: dddddddd-dddd-4ddd-8ddd-dddddddddddd
title: Delta
tags: [smoke]
created: 2026-10-03T09:00:00Z
updated: 2026-10-03T09:00:00Z
---

delta body line.
EOF
sleep 2.5
if palette_open; then
  ok "Ctrl+P opened the command palette"
else
  bad "Ctrl+P did not open the command palette"
fi
shot 90-palette

if open_note_via_palette delta "Delta — Writan"; then
  ok "typing 'delta' and Enter opened the note from the palette"
else
  bad "the palette did not open Delta (title: '$(xdotool getwindowname "$window")')"
fi
shot 93-delta-open

xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
sleep 0.8
xdotool key --clearmodifiers Escape
if editor_gets_typing "escape-dismiss-marker"; then
  ok "Escape dismissed the palette and the editor had the keyboard again"
else
  bad "the palette kept the keyboard after Escape"
fi

geometry_before="$(xdotool getwindowgeometry --shell "$window" | tr '\n' ' ')"
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
sleep 0.8
# Well below the palette (which sits at the top, at most ~430 px tall): the
# click lands in the editor pane, so the palette must autohide and the click
# must both dismiss it and put the keyboard back in the editor.
xdotool mousemove --sync 520 620
xdotool click 1
sleep 0.3
geometry_after="$(xdotool getwindowgeometry --shell "$window" | tr '\n' ' ')"
if editor_gets_typing "click-dismiss-marker" && [ "$geometry_before" = "$geometry_after" ]; then
  ok "clicking outside dismissed the palette and left the window where it was"
else
  bad "a click outside the palette dismissed nothing or moved the window"
fi
shot 94-palette-dismissed

# 17-20. settings.shortcuts: restart with command-palette rebound to
# Ctrl+Shift+P. Ctrl+P must be dead, the new binding and the Ctrl+K alias
# must open the palette, and untouched defaults (Ctrl+N) must survive.
kill "$app_pid" 2>/dev/null || true
wait "$app_pid" 2>/dev/null || true
app_pid=""
printf '{"vault_path": "%s", "settings": {"shortcuts": {"command-palette": "Ctrl+Shift+P"}}}' "$vault" \
  >"$config_home/com.writan.desktop/config.json"
env \
  WRITAN_VAULT="$vault" \
  XDG_CONFIG_HOME="$config_home" \
  XDG_DATA_HOME="$data_home" \
  GTK_THEME="Adwaita:dark" \
  GSK_RENDERER=cairo \
  "$bin" >>"$work/app.log" 2>&1 &
app_pid=$!
window=""
for _ in $(seq 1 100); do
  window="$(find_window)"
  [ -n "$window" ] && break
  sleep 0.2
done
if [ -n "$window" ]; then
  xdotool windowmove "$window" 0 0
  xdotool windowsize "$window" 1000 700
  sleep 0.6
  xdotool windowfocus --sync "$window"
  xdotool windowraise "$window"
else
  bad "the app window never reappeared after the restart"
fi

# Ctrl+P is dead: with the palette up, Enter would run its first item ("New
# note") and change the window title; with it dead, Enter only adds a line to
# the editor.
before_title="$(xdotool getwindowname "$window")"
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
sleep 1.0
xdotool key --clearmodifiers Return
sleep 1.2
if [ "$(xdotool getwindowname "$window")" = "$before_title" ]; then
  ok "the override left Ctrl+P dead"
else
  bad "Ctrl+P still opened the palette after the rebind"
fi
shot 94-rebound

if open_note_with ctrl+shift+p shot "Shot — Writan"; then
  ok "Ctrl+Shift+P opened the palette (the rebind is live)"
else
  bad "the rebound Ctrl+Shift+P did not open the palette"
fi

if open_note_with ctrl+k delta "Delta — Writan"; then
  ok "the Ctrl+K alias still opens the palette"
else
  bad "the Ctrl+K alias did not open the palette after the rebind"
fi

xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+n
sleep 1.2
case "$(xdotool getwindowname "$window")" in
  *"Untitled — Writan"*) ok "Ctrl+N still creates a note after the override load" ;;
  *) bad "Ctrl+N did not create a note (title: '$(xdotool getwindowname "$window")')" ;;
esac
shot 95-new-note

# 21-27. Settings panel: Ctrl+, opens it, the filter narrows it, a rebind
# writes settings.shortcuts, Escape closes it, the new combo opens the
# palette while the old one is dead, Reset drops the override, and the Show
# word count switch reaches config.json. The panel is parked at 0,0 (620x560)
# and rows are reached by typing a filter (triple-click clears the entry),
# so a row added to the panel only moves the filtered coordinates.
config_file="$config_home/com.writan.desktop/config.json"
config_has() { grep -q "$1" "$config_file" 2>/dev/null; }
wait_config_has() { # substring
  for _ in $(seq 1 25); do
    config_has "$1" && return 0
    sleep 0.2
  done
  return 1
}
open_settings() {
  xdotool windowfocus --sync "$window"
  xdotool key --clearmodifiers ctrl+comma
  settings=""
  for _ in $(seq 1 25); do
    settings="$(settings_window || true)"
    [ -n "$settings" ] && break
    sleep 0.2
  done
  [ -n "$settings" ] || return 1
  xdotool windowmove "$settings" 0 0
  sleep 0.3
  xdotool windowfocus --sync "$settings"
  xdotool windowraise "$settings"
}

if open_settings; then
  geom="$(xdotool getwindowgeometry --shell "$settings" 2>/dev/null || true)"
  sw="$(sed -n 's/^WIDTH=//p' <<<"$geom")"
  sh="$(sed -n 's/^HEIGHT=//p' <<<"$geom")"
  if [ "$sw" = "620" ] && [ "$sh" = "560" ]; then
    ok "Ctrl+, opened the Settings panel (${sw}x${sh})"
  else
    bad "the Settings panel is ${sw:-?}x${sh:-?}, wanted 620x560"
  fi
  shot_of 96-settings "$settings"
else
  bad "Ctrl+, did not open a window titled 'Settings'"
fi

# 22. Filter down to the Command palette row and rebind it from the panel.
if [ -n "$settings" ]; then
  xdotool mousemove --sync 300 76
  xdotool click 1
  xdotool type --delay 25 -- "command"
  sleep 0.8
  shot_of 97-settings-filtered "$settings"
  xdotool mousemove --sync 538 166
  xdotool click 1
  sleep 0.4
  xdotool key --clearmodifiers ctrl+alt+p
  rebound=""
  for _ in $(seq 1 25); do
    if config_has '"command-palette": "Ctrl+Alt+P"'; then rebound=1; break; fi
    sleep 0.2
  done
  if [ -n "$rebound" ]; then
    ok "the filter narrowed to the Command palette row and rebinding it wrote settings.shortcuts"
  else
    bad "rebinding the filtered row did not write settings.shortcuts"
  fi
  shot_of 98-rebound "$settings"
fi

# 23. Escape closes the panel.
if [ -n "$settings" ]; then
  xdotool key --clearmodifiers Escape
  sleep 0.8
  if [ -z "$(settings_window || true)" ]; then
    ok "Escape closed the Settings panel"
  else
    bad "Escape did not close the Settings panel"
  fi
fi

# 24. The replaced Ctrl+Shift+P must be dead; the new combo opens the palette.
before_title="$(xdotool getwindowname "$window")"
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+shift+p
sleep 1.0
xdotool key --clearmodifiers Return
sleep 1.2
if [ "$(xdotool getwindowname "$window")" = "$before_title" ]; then
  ok "the replaced Ctrl+Shift+P stayed dead"
else
  bad "the replaced Ctrl+Shift+P still opened the palette"
fi
if open_note_with ctrl+alt+p shot "Shot — Writan"; then
  ok "the binding recorded in the panel opens the palette (the old one is dead)"
else
  bad "the new binding from the Settings panel did not open the palette"
fi

# 25-26. Reset drops the override and the default Ctrl+P comes back.
if open_settings; then
  xdotool mousemove --sync 300 76
  xdotool click --repeat 3 --delay 60 1
  sleep 0.2
  xdotool type --delay 25 -- "command"
  sleep 0.8
  xdotool mousemove --sync 462 168
  xdotool click 1
  reset=""
  for _ in $(seq 1 25); do
    if ! config_has 'Ctrl+Alt+P'; then reset=1; break; fi
    sleep 0.2
  done
  if [ -n "$reset" ]; then
    ok "Reset in the panel dropped the command-palette override"
  else
    bad "Reset left the override in config.json"
  fi
  shot_of 99-reset-default "$settings"
  xdotool key --clearmodifiers Escape
  sleep 0.8
  if open_note_with ctrl+p shot "Shot — Writan"; then
    ok "the default Ctrl+P opens the palette again after Reset"
  else
    bad "Ctrl+P did not open the palette after Reset"
  fi
fi

# 27. Show word count off reaches config.json.
if open_settings; then
  xdotool mousemove --sync 300 76
  xdotool click --repeat 3 --delay 60 1
  sleep 0.2
  xdotool type --delay 25 -- "word count"
  sleep 0.8
  xdotool mousemove --sync 544 137
  xdotool click 1
  words_off=""
  for _ in $(seq 1 25); do
    if config_has '"showWordCount": false'; then words_off=1; break; fi
    sleep 0.2
  done
  if [ -n "$words_off" ]; then
    ok "Show word count off wrote showWordCount: false to config.json"
  else
    bad "the Show word count switch did not reach config.json"
  fi
  shot_of 100-word-count-off "$settings"
  xdotool click 1
  sleep 0.5
  xdotool key --clearmodifiers Escape
  sleep 0.5
fi

# 28-29. Ctrl+V with an image on the clipboard stores it as an attachment,
# and pasting it again reuses the same file.
convert -size 120x60 xc:tomato "$out/red.png"
xclip -selection clipboard -t image/png -i "$out/red.png"
xdotool windowfocus --sync "$window"
xdotool mousemove --sync 650 300
xdotool click 1
sleep 0.3
xdotool key --clearmodifiers ctrl+v
pasted=""
for _ in $(seq 1 30); do
  pasted="$(ls "$vault/attachments" 2>/dev/null | grep '^pasted-' | head -1 || true)"
  [ -n "$pasted" ] && break
  sleep 0.2
done
if [ -n "$pasted" ]; then
  ok "Ctrl+V stored the clipboard image as attachments/$pasted"
  linked=""
  for _ in $(seq 1 25); do
    if grep -rq "attachments/$pasted" "$vault" 2>/dev/null; then linked=1; break; fi
    sleep 0.2
  done
  if [ -n "$linked" ]; then
    ok "the open note links the pasted image"
  else
    bad "the open note has no link to attachments/$pasted"
  fi
else
  bad "Ctrl+V with an image on the clipboard stored no attachment"
fi
shot_of 101-image-paste "$window"

count_before="$(ls "$vault/attachments" 2>/dev/null | wc -l)"
xdotool key --clearmodifiers ctrl+v
sleep 1.2
count_after="$(ls "$vault/attachments" 2>/dev/null | wc -l)"
if [ -n "$pasted" ] && [ "$count_after" = "$count_before" ]; then
  ok "pasting the same image again reused the file"
else
  bad "a second paste of the same image created another file"
fi

# 30. The palette's "Attach file…" imports a picked file through the dialog.
convert -size 120x60 xc:seagreen "$out/green.png"
if palette_open; then
  # The probe closed the palette again: reopen it for the attach flow.
  xdotool windowfocus --sync "$window"
  xdotool key --clearmodifiers ctrl+p
  sleep 0.8
  xdotool type --delay 30 -- "attach"
  sleep 0.5
  xdotool key --clearmodifiers Return
  dialog=""
  for _ in $(seq 1 25); do
    dialog="$(xdotool search --onlyvisible --name '^Attach file$' 2>/dev/null | head -1 || true)"
    [ -n "$dialog" ] && break
    sleep 0.2
  done
  if [ -n "$dialog" ]; then
    xdotool windowfocus --sync "$dialog"
    xdotool key --clearmodifiers ctrl+l
    sleep 0.4
    # Paste the path: typing it trips the location entry's completion.
    printf '%s' "$out/green.png" | xclip -selection clipboard
    xdotool key --clearmodifiers ctrl+v
    sleep 0.8
    xdotool key --clearmodifiers Delete
    sleep 0.2
    xdotool key --clearmodifiers Return
    attached=""
    for _ in $(seq 1 30); do
      attached="$(ls "$vault/attachments" 2>/dev/null | grep '^green-' | head -1 || true)"
      [ -n "$attached" ] && break
      sleep 0.2
    done
    if [ -n "$attached" ]; then
      ok "the palette's 'Attach file…' imported the picked file as attachments/$attached"
    else
      bad "the palette's 'Attach file…' imported nothing"
    fi
    shot_of 102-attach-window "$window"
  else
    bad "the palette's 'Attach file…' did not open a dialog"
  fi
else
  bad "Ctrl+P did not open the palette for Attach file…"
fi

# 31. Pasting text still reaches the editor (the handler must fall through).
printf 'text paste still works' | xclip -selection clipboard
xdotool windowfocus --sync "$window"
xdotool mousemove --sync 650 300
xdotool click 1
sleep 0.3
xdotool key --clearmodifiers ctrl+v
text_pasted=""
for _ in $(seq 1 30); do
  if grep -rq "text paste still works" "$vault" 2>/dev/null; then text_pasted=1; break; fi
  sleep 0.2
done
if [ -n "$text_pasted" ]; then
  ok "pasted text still reaches the editor"
else
  bad "pasted text did not reach the open note"
fi
shot_of 103-text-paste "$window"

# 32-35 need a couple of small helpers (the palette ones live at the top).
restart_app() {
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  app_pid=""
  env \
    WRITAN_VAULT="$vault" \
    XDG_CONFIG_HOME="$config_home" \
    XDG_DATA_HOME="$data_home" \
    GTK_THEME="Adwaita:dark" \
    GSK_RENDERER=cairo \
    "$bin" >>"$work/app.log" 2>&1 &
  app_pid=$!
  window=""
  for _ in $(seq 1 100); do
    window="$(find_window)"
    [ -n "$window" ] && break
    sleep 0.2
  done
  if [ -n "$window" ]; then
    xdotool windowmove "$window" 0 0
    xdotool windowsize "$window" 1000 700
    sleep 0.6
    xdotool windowfocus --sync "$window"
    xdotool windowraise "$window"
  else
    bad "the app window never reappeared after the restart"
  fi
}
filter_settings() { # filter text
  xdotool mousemove --sync 300 76
  xdotool click --repeat 3 --delay 60 1
  sleep 0.2
  xdotool type --delay 25 -- "$1"
  sleep 0.9
}

# 32. The tag row: a chip removes that tag, a typed tag reaches the file.
if open_note_via_palette delta "Delta — Writan"; then
  # Delta starts with the fixture's 'smoke' tag: its chip removes that one.
  # The chip is small (that is the point of the minimal tag row), so a few
  # pixels either side of its centre are tried before the check fails.
  smoke_gone=""
  for x in 505 520 492; do
    xdotool mousemove --sync "$x" 111
    xdotool click 1
    for _ in $(seq 1 8); do
      if ! frontmatter "$vault/delta.md" | grep -q "smoke"; then smoke_gone=1; break; fi
      sleep 0.2
    done
    [ -n "$smoke_gone" ] && break
  done
  if [ -n "$smoke_gone" ]; then
    ok "clicking a tag chip removed that tag from the note file"
  else
    bad "the tag chip did not remove the tag"
  fi
  # The entry's centre line is y=110; one pixel lower is the editor body, so
  # the typed tag must land in the frontmatter (a tag), not merely in the file.
  xdotool mousemove --sync 387 110
  xdotool click 1
  sleep 0.5
  xdotool type --delay 30 -- "review-check"
  xdotool key --clearmodifiers Return
  tagged=""
  for _ in $(seq 1 25); do
    if frontmatter "$vault/delta.md" | grep -q "review-check"; then tagged=1; break; fi
    sleep 0.2
  done
  if [ -n "$tagged" ]; then
    ok "a tag typed in the editor header reached the note file"
  else
    bad "the typed tag did not reach the note file"
  fi
  shot_of 104-tag-added "$window"
  sleep 0.6
  # The typed tag is the only chip left, so it sits where the first one did.
  chip_gone=""
  for x in 505 520 492; do
    xdotool mousemove --sync "$x" 111
    xdotool click 1
    for _ in $(seq 1 8); do
      if ! frontmatter "$vault/delta.md" | grep -q "review-check"; then chip_gone=1; break; fi
      sleep 0.2
    done
    [ -n "$chip_gone" ] && break
  done
  if [ -n "$chip_gone" ]; then
    ok "clicking the chip of the typed tag removed it from the file"
  else
    bad "the typed tag's chip did not remove it"
  fi
else
  bad "the palette did not open Delta for the tag check"
fi

# 33. Appearance: the theme and accent choices persist, and the accent
# repaints libadwaita's switches live (teal: 95,215,198).
pick_appearance() { # filter text, popover item y
  filter_settings "$1"
  xdotool mousemove --sync 100 135
  xdotool click 1
  sleep 0.8
  xdotool mousemove --sync 570 "$2"
  xdotool click 1
  sleep 0.8
}
if open_settings; then
  pick_appearance "follow system" 214   # Dark
  if wait_config_has '"theme": "dark"'; then
    ok "picking 'Dark' in Appearance wrote theme: dark to config.json"
  else
    bad "the theme choice did not reach config.json"
  fi
  pick_appearance "follow system" 192   # Light
  if wait_config_has '"theme": "light"'; then
    ok "picking 'Light' wrote theme: light"
  else
    bad "the second theme choice did not reach config.json"
  fi
  pick_appearance "follow system" 170   # Follow system
  if wait_config_has '"theme": "system"'; then
    ok "picking 'Follow system' restored the default theme"
  else
    bad "'Follow system' did not reach config.json"
  fi
  pick_appearance "preview links" 214   # Teal
  if wait_config_has '"accent": "teal"'; then
    ok "picking 'Teal' wrote accent: teal to config.json"
  else
    bad "the accent choice did not reach config.json"
  fi
  filter_settings "word count"
  shot_of 106-accent-teal "$settings"
  px="$(convert "$out/106-accent-teal.png" -crop 10x10+539+132 +repage -resize 1x1 \
    -format '%[fx:int(255*r)],%[fx:int(255*g)],%[fx:int(255*b)]' info: 2>/dev/null || echo 0,0,0)"
  pxr="${px%%,*}"; pxgb="${px#*,}"
  pxg="${pxgb%%,*}"; pxb="${pxgb#*,}"
  if [ "${pxg:-0}" -ge 150 ] && [ "${pxb:-0}" -ge 140 ] && [ "${pxr:-255}" -le 130 ]; then
    ok "the teal accent repainted the Show word count switch ($px)"
  else
    bad "the switch did not take the accent colour ($px)"
  fi
  xdotool key --clearmodifiers Escape
  sleep 0.8
else
  bad "Ctrl+, did not open Settings for the appearance check"
fi

# 34. A local image link renders inline in the preview pane.
if open_note_via_palette shot "Shot — Writan"; then
  sleep 0.8
  shot_of 107-preview-image "$window"
  share="$(convert "$out/107-preview-image.png" -crop 100x40+840+80 +repage -fuzz 30% \
    -fill white -opaque '#FF6347' -fill black +opaque white \
    -format '%[fx:mean*100]' info: 2>/dev/null || echo 0)"
  if [ "${share%%.*}" -ge 20 ]; then
    ok "the preview pane painted the note's local image inline (${share}% tomato)"
  else
    bad "no inline image in the preview pane (${share}% tomato)"
  fi
else
  bad "the palette did not open the Shot note"
fi

# 35. Session restore: a restart reopens the last note, and stops doing so
# when 'Reopen last note' is switched off (it falls back to the first row).
if open_note_via_palette delta "Delta — Writan"; then
  sleep 0.5
  restart_app
  if wait_title "Delta — Writan" 12; then
    ok "a restart reopened the note that was open (session restore)"
  else
    bad "session restore did not reopen Delta (title: '$(xdotool getwindowname "$window")')"
  fi
else
  bad "the palette did not open Delta for the session check"
fi
if open_settings; then
  filter_settings "reopen"
  xdotool mousemove --sync 544 137
  xdotool click 1
  if wait_config_has '"reopenLastNote": false'; then
    ok "'Reopen last note' off wrote reopenLastNote: false to config.json"
  else
    bad "the reopen switch did not reach config.json"
  fi
  xdotool key --clearmodifiers Escape
  sleep 0.8
else
  bad "Ctrl+, did not open Settings for the reopen switch"
fi
# Pin Delta so it is the first row, then make Untitled the last note: the
# restart must ignore the last note and open the first row instead.
if open_note_via_palette delta "Delta — Writan"; then
  sleep 0.6
  xdotool mousemove --sync 723 77
  xdotool click 1
  sleep 1.0
  if open_note_via_palette untitled "Untitled — Writan"; then
    sleep 0.5
    restart_app
    if wait_title "Delta — Writan" 12; then
      ok "with 'Reopen last note' off the app opened the first row instead"
    else
      bad "the switch off did not fall back to the first row (title: '$(xdotool getwindowname "$window")')"
    fi
  else
    bad "the palette did not open Untitled for the fallback check"
  fi
fi

# --- Sync (slice 8): the folder target, the keyring, the recovery code, lock
# and unlock, Turn off, and a second device joining the same folder. ---
sync_dir="$work/sync"
sync_password="smoke-sync-pass"
launch_app() { # uses the current $vault / $config_home / $data_home
  env \
    WRITAN_VAULT="$vault" \
    XDG_CONFIG_HOME="$config_home" \
    XDG_DATA_HOME="$data_home" \
    GTK_THEME="Adwaita:dark" \
    GSK_RENDERER=cairo \
    "$bin" >>"$work/app.log" 2>&1 &
  app_pid=$!
  window=""
  for _ in $(seq 1 100); do
    window="$(find_window)"
    [ -n "$window" ] && break
    sleep 0.2
  done
  if [ -n "$window" ]; then
    xdotool windowmove "$window" 0 0
    xdotool windowsize "$window" 1000 700
    sleep 0.6
    xdotool windowfocus --sync "$window"
    xdotool windowraise "$window"
  else
    bad "the app window never appeared for the sync checks"
  fi
}
count_items() {
  find "$sync_dir/items" -name '*.enc' 2>/dev/null | wc -l | tr -d ' ' || true
}
wait_items() { # wanted count, seconds
  local want="$1" deadline=$(( $(date +%s) + $2 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if [ "$(count_items)" -ge "$want" ]; then return 0; fi
    sleep 0.3
  done
  return 1
}

# 36. Enabling sync creates the folder keyring and runs the first pass. The
# target is seeded in config.json so the check only types the password.
kill "$app_pid" 2>/dev/null || true
wait "$app_pid" 2>/dev/null || true
app_pid=""
mkdir -p "$sync_dir"
printf '{"vault_path": "%s", "sync": {"kind": "folder", "target": "%s", "enabled": false}}' \
  "$vault" "$sync_dir" >"$config_home/com.writan.desktop/config.json"
launch_app
if open_settings; then
  filter_settings "sync"
  xdotool mousemove --sync 480 284
  xdotool click 1
  xdotool type --delay 25 -- "$sync_password"
  shot_of 96-sync-setup "$(settings_window)"
  xdotool mousemove --sync 523 333
  xdotool click 1
  if wait_items 1 15; then
    if [ -f "$sync_dir/keys/1.json" ]; then
      ok "enabling sync created the folder keyring and published the vault"
    else
      bad "the first pass wrote items but no keys/1.json"
    fi
  else
    bad "enabling sync did not publish anything (items: $(count_items))"
  fi
else
  bad "Ctrl+, did not open Settings for the sync check"
fi
shot 97-sync-enabled

# 37. The enabled state and the vault binding reach the app-data dir.
if wait_config_has '"enabled": true' 5 &&
  [ -f "$data_home/com.writan.desktop/sync-state.json" ]; then
  ok "config.json carries enabled: true and sync-state.json exists"
else
  bad "config.json or sync-state.json did not record the enabled sync"
fi
shot 98-sync-connected

# 38. Copy puts the recovery code on the clipboard (it is shown once).
xdotool mousemove --sync 548 218
xdotool click 1
sleep 0.6
recovery="$(xclip -selection clipboard -o 2>/dev/null | tr -d '\n' || true)"
case "$recovery" in
  ????-????-????-????-????-????-????-????-????-????-????-????-????)
    ok "Copy put the recovery code on the clipboard" ;;
  *)
    bad "no recovery code on the clipboard (got '$recovery')" ;;
esac

# 39. A freshly created note is published by the palette's Sync now.
xdotool key --clearmodifiers Escape
sleep 0.8
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+n
sleep 1.0
xdotool type --delay 20 -- "Sync publish test"
sleep 2.0
items_before="$(count_items)"
if run_palette_command "sync now"; then
  if wait_items $((items_before + 1)) 15; then
    ok "the palette's Sync now published the new note"
  else
    bad "Sync now did not publish the new note (items: $(count_items), wanted $((items_before + 1)))"
  fi
else
  bad "the palette did not open for the Sync now check"
fi
shot 99-sync-now

# 40-41. Lock keeps the device from publishing; the password unlocks it again.
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+n
sleep 1.0
xdotool type --delay 20 -- "While locked"
sleep 2.0
if open_settings; then
  filter_settings "sync"
  xdotool mousemove --sync 548 381
  xdotool click 1
  sleep 0.8
  shot_of 100-sync-locked "$(settings_window)"
  xdotool key --clearmodifiers Escape
  sleep 0.8
else
  bad "Settings did not reopen for the lock check"
fi
items_before="$(count_items)"
run_palette_command "sync now" || true
sleep 2.0
if [ "$(count_items)" -eq "$items_before" ]; then
  ok "a locked device changed nothing on Sync now"
else
  bad "a locked device published without the key (items: $items_before -> $(count_items))"
fi
if open_settings; then
  filter_settings "sync"
  xdotool mousemove --sync 413 208
  xdotool click 1
  xdotool type --delay 25 -- "$sync_password"
  xdotool mousemove --sync 539 208
  xdotool click 1
  sleep 1.2
  xdotool key --clearmodifiers Escape
  sleep 0.8
else
  bad "Settings did not reopen for the unlock check"
fi
if run_palette_command "sync now" && wait_items $((items_before + 1)) 15; then
  ok "unlocking with the password published again"
else
  bad "Sync now after the unlock did not publish (items: $(count_items))"
fi
shot 101-sync-unlocked

# 42. Turn off arms once and clears enabled in config.json.
if open_settings; then
  filter_settings "sync"
  xdotool mousemove --sync 500 413
  xdotool click 1
  sleep 0.5
  xdotool mousemove --sync 511 427
  xdotool click 1
  if wait_config_has '"enabled": false' 6; then
    ok "the two-click Turn off cleared enabled in config.json"
  else
    bad "Turn off did not reach config.json"
  fi
  shot_of 102-sync-turnoff "$(settings_window)"
else
  bad "Settings did not reopen for the Turn off check"
fi

# The panel is a window of its own: left up it takes the clicks meant for the
# main window (the sync checks end with it in front, so close it here).
close_settings || true

# 44. Right-clicking a note row opens its menu. The rows sit lower when the
# sidebar shows facet chips, so the row is found by watching for the click that
# opens the menu (the menu is a popover window of its own — the palette, by
# contrast, stays inside the main window). The menu's first entry takes focus
# while it is up, so Enter duplicates that row's note and a copy lands in the
# vault next to the original.
xdotool windowfocus --sync "$window"
xdotool windowraise "$window"
copies_before="$(ls "$vault"/*-copy-*.md 2>/dev/null | wc -l | tr -d ' ' || true)"
windows_before="$(xdotool search --onlyvisible --name '.*' 2>/dev/null | sort | tr '\n' ' ' || true)"
menu_row=""
for y in 170 190 210 230 250 270; do
  xdotool mousemove --sync 200 "$y"
  xdotool click 3
  sleep 0.8
  if [ "$(xdotool search --onlyvisible --name '.*' 2>/dev/null | sort | tr '\n' ' ')" != "$windows_before" ]; then
    menu_row="$y"
    break
  fi
done
if [ -n "$menu_row" ]; then
  import -window root "$out/103-row-menu.png" 2>/dev/null || true
  xdotool key --clearmodifiers Return
  sleep 1.5
  copies_after="$(ls "$vault"/*-copy-*.md 2>/dev/null | wc -l | tr -d ' ' || true)"
  if [ "$copies_after" -gt "$copies_before" ]; then
    ok "the row menu's Duplicate wrote a copy to the vault"
  else
    bad "the row menu's Duplicate created no copy ($copies_before -> $copies_after)"
  fi
else
  bad "right-clicking the note list opened no menu (row scan y=170..270 at x=200)"
fi
shot 104-note-copied

# 45. The palette searches the Settings panel too: typing a setting's name and
# pressing Enter opens the panel filtered to that row (the reveal the Tauri
# palette has had all along).
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
sleep 0.8
xdotool type --delay 25 -- "word count"
sleep 0.8
shot 105-palette-setting
xdotool key --clearmodifiers Return
revealed=""
for _ in $(seq 1 25); do
  revealed="$(settings_window || true)"
  [ -n "$revealed" ] && break
  sleep 0.2
done
if [ -n "$revealed" ]; then
  ok "the palette found a settings row and opened the Settings panel on it"
  shot_of 106-settings-revealed "$revealed"
  xdotool key --clearmodifiers Escape
  sleep 0.6
else
  bad "a settings search in the palette did not open the Settings panel"
fi

# The reveal raised the panel over the window; close it so the list divider
# below is grabbable again.
close_settings || true

# 46. The list divider is remembered in config.json, and its floor keeps the
# pane on screen: dragged hard left it stops at 200 px, never at zero.
config_file="$config_home/com.writan.desktop/config.json"
xdotool windowfocus --sync "$window"
xdotool windowraise "$window"
xdotool mousemove --sync 281 350
xdotool mousedown 1
xdotool mousemove --sync 460 350
sleep 0.4
xdotool mouseup 1
sleep 1.4
if wait_config_has '"sidebarWidth"'; then
  ok "dragging the list divider was remembered in config.json"
else
  bad "the dragged divider never reached config.json"
fi
stored_width="$(grep -o '"sidebarWidth": *[0-9]*' "$config_file" | grep -o '[0-9]*' | tail -1 || true)"
if [ -n "$stored_width" ]; then
  xdotool mousemove --sync $((stored_width + 2)) 350
  xdotool mousedown 1
  xdotool mousemove --sync 0 350
  sleep 0.4
  xdotool mouseup 1
  sleep 1.4
  stored_width="$(grep -o '"sidebarWidth": *[0-9]*' "$config_file" | grep -o '[0-9]*' | tail -1 || true)"
  if [ "$stored_width" = "200" ]; then
    ok "the list pane cannot be dragged below its 200 px floor"
  else
    bad "the list divider left its floor (sidebarWidth: $stored_width)"
  fi
  shot 107-pane-floor
else
  bad "no sidebarWidth in config.json to drag from"
fi

# 43. A second device (its own vault and XDG dirs) joins the same folder with
# the password: no second keyring appears and the vault adopts device one's
# notes while publishing its own.
kill "$app_pid" 2>/dev/null || true
wait "$app_pid" 2>/dev/null || true
app_pid=""
device2="$work/device2"
vault2="$device2/vault"
mkdir -p "$device2/config/com.writan.desktop" "$device2/data" "$vault2"
printf '# Beta\n\ndevice two body.\n' >"$vault2/beta.md"
printf '{"vault_path": "%s", "sync": {"kind": "folder", "target": "%s", "enabled": false}}' \
  "$vault2" "$sync_dir" >"$device2/config/com.writan.desktop/config.json"
vault="$vault2" config_home="$device2/config" data_home="$device2/data"
launch_app
items_before="$(count_items)"
if open_settings; then
  filter_settings "sync"
  xdotool mousemove --sync 480 284
  xdotool click 1
  xdotool type --delay 25 -- "$sync_password"
  xdotool mousemove --sync 523 333
  xdotool click 1
  if wait_items $((items_before + 1)) 20; then
    if [ "$(ls "$sync_dir/keys" | wc -l | tr -d ' ')" -eq 1 ] &&
      ls "$vault2"/delta-*.md >/dev/null 2>&1; then
      ok "a second device joined the keyring and adopted device one's notes"
    else
      bad "the second device published but did not adopt (keys: $(ls "$sync_dir/keys" | tr '\n' ' '))"
    fi
  else
    bad "the second device did not sync (items: $(count_items), wanted $((items_before + 1)))"
  fi
  shot_of 103-sync-device-two "$(settings_window)"
else
  bad "Settings did not open on the second device"
fi
close_settings || true

# The exit status is the whole point: zero when everything passed. The ERR
# trap above would only add noise here.
trap - ERR
note "gtk-shell-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
