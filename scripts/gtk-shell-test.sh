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
#  16. Escape closes it
#  17. a settings.shortcuts override leaves Ctrl+P dead
#  18. the rebound Ctrl+Shift+P opens the palette
#  19. the Ctrl+K alias still opens it
#  20. Ctrl+N (an untouched default) still creates a note
# Real app data is untouched: XDG dirs point into a temp directory, which is
# kept only when a check fails (or with --keep). Screenshots land in --out.
#
# Deps (one line on Ubuntu/Debian):
#   sudo apt-get install -y xvfb xdotool imagemagick sqlite3 \
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
    -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
    *) echo "gtk-shell-test: unknown option $1" >&2; exit 2 ;;
  esac
done

for tool in Xvfb xdotool import sqlite3; do
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
printf '{"vault_path": "%s"}' "$vault" >"$config_home/com.writan.desktop/config.json"

Xvfb "$display" -screen 0 1280x800x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb_pid=$!
export DISPLAY="$display"
for _ in $(seq 1 50); do
  xdotool getdisplaygeometry >/dev/null 2>&1 && break
  sleep 0.1
done

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
palette_window() { xdotool search --onlyvisible --name '^Command palette$' 2>/dev/null | head -1; }

wait_title() { # substring, seconds
  local want="$1" deadline=$(( $(date +%s) + $2 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    case "$(xdotool getwindowname "$window")" in *"$want"*) return 0 ;; esac
    sleep 0.2
  done
  return 1
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

# 14-16. Command palette: Ctrl+P opens it, typing filters, Enter opens the
# note, Escape closes the panel.
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
xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
palette=""
for _ in $(seq 1 25); do
  palette="$(palette_window || true)"
  [ -n "$palette" ] && break
  sleep 0.2
done
if [ -n "$palette" ]; then
  ok "Ctrl+P opened the command palette"
else
  bad "Ctrl+P did not open a window titled 'Command palette'"
fi
shot 90-palette
[ -n "$palette" ] && shot_of 91-palette-window "$palette"

if [ -n "$palette" ]; then
  xdotool windowfocus --sync "$palette"
  xdotool type --delay 25 -- "delta"
  sleep 1.0
  shot_of 92-palette-filtered "$palette"
  xdotool key --clearmodifiers Return
  sleep 1.0
  if [ -z "$(palette_window || true)" ] && wait_title "Delta — Writan" 5; then
    ok "typing 'delta' and Enter opened the note from the palette"
  else
    bad "the palette did not open Delta (title: '$(xdotool getwindowname "$window")')"
  fi
else
  bad "skipping the palette filter check: no palette window"
fi
shot 93-delta-open

xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+p
sleep 1.0
if [ -n "$(palette_window || true)" ]; then
  xdotool key --clearmodifiers Escape
  sleep 0.8
  if [ -z "$(palette_window || true)" ]; then
    ok "Escape closed the command palette"
  else
    bad "Escape did not close the palette"
  fi
else
  bad "the palette did not reopen for the Escape check"
fi

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

xdotool key --clearmodifiers ctrl+p
sleep 1.0
if [ -z "$(palette_window || true)" ]; then
  ok "the override left Ctrl+P dead"
else
  bad "Ctrl+P still opened the palette after the rebind"
fi
shot 94-rebound

xdotool key --clearmodifiers ctrl+shift+p
palette=""
for _ in $(seq 1 25); do
  palette="$(palette_window || true)"
  [ -n "$palette" ] && break
  sleep 0.2
done
if [ -n "$palette" ]; then
  ok "Ctrl+Shift+P opened the palette (the rebind is live)"
  xdotool key --clearmodifiers Escape
  sleep 0.8
else
  bad "the rebound Ctrl+Shift+P did not open the palette"
fi

xdotool windowfocus --sync "$window"
xdotool key --clearmodifiers ctrl+k
sleep 1.0
if [ -n "$(palette_window || true)" ]; then
  ok "the Ctrl+K alias still opens the palette"
  xdotool key --clearmodifiers Escape
  sleep 0.8
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

note "gtk-shell-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
