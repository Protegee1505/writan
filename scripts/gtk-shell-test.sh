#!/usr/bin/env bash
# Headless functional smoke test for the GTK shell (app-linux, M3.5).
#
#   scripts/gtk-shell-test.sh --bin /tmp/opencode/target/release/writan
#
# Runs the real app on an Xvfb display against a throwaway vault and checks
# the autosave/watcher behaviors end to end:
#   1. startup opens the remembered vault and shows its first note
#   2. typing lands in the Markdown file on disk (debounced autosave)
#   3. an external edit while the buffer is clean reloads the editor
#   4. a new file in the vault reaches the index and the list
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
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
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
xdotool windowfocus --sync "$window"
xdotool windowraise "$window"

shot() { import -window "$window" "$out/$1.png" 2>/dev/null || true; }

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
tags: [smoke]
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

# The list places Beta second (Alpha is pinned); click a few candidate rows
# until the editor opens it.
opened=""
for y in 175 223 127; do
  xdotool mousemove --sync 150 "$y"
  xdotool click 1
  sleep 0.6
  case "$(xdotool getwindowname "$window")" in
    *"Beta — Writan"*) opened="$y"; break ;;
  esac
done
if [ -n "$opened" ]; then
  ok "the refreshed list shows and opens the new note (row y=$opened)"
else
  bad "clicking the list did not open Beta"
fi
shot 20-beta-selected

note "gtk-shell-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
