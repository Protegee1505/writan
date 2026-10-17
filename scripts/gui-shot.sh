#!/usr/bin/env bash
# Run a Linux Writan build under Xvfb and capture walkthrough screenshots.
#
#   scripts/gui-shot.sh --bin /tmp/writan/target/release/writan
#   scripts/gui-shot.sh --theme light --out /tmp/shots-light
#
# The script never touches your real app data: XDG_CONFIG_HOME/XDG_DATA_HOME
# point into a temp dir and the fixture vault (tools/visual/fixtures/vault)
# is copied there too. Screenshots land in the output dir (default
# /tmp/writan-shots); hand them to an agent, or read them yourself.
#
# Deps (one line on Ubuntu/Debian):
#   sudo apt-get install -y xvfb xdotool imagemagick \
#     libgtk-4-1 libadwaita-1-0 libgtksourceview-5-0
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${WRITAN_BIN:-target/release/writan}"
out="${WRITAN_SHOTS_OUT:-/tmp/writan-shots}"
vault=""
actions="$repo/tools/visual/actions/gtk-shell.actions"
theme="dark"
screen="1280x800"
strict=""
keep=""

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) bin="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --vault) vault="$2"; shift 2 ;;
    --actions) actions="$2"; shift 2 ;;
    --theme) theme="$2"; shift 2 ;;
    --screen) screen="$2"; shift 2 ;;
    --strict) strict="--strict"; shift ;;
    --keep) keep=1; shift ;;
    -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "gui-shot: unknown option $1" >&2; exit 2 ;;
  esac
done

for tool in Xvfb xdotool import convert; do
  command -v "$tool" >/dev/null || {
    echo "gui-shot: $tool is missing; see the dep line in this script's header" >&2
    exit 1
  }
done
[ -x "$bin" ] || { echo "gui-shot: not executable: $bin" >&2; exit 1; }
[ -f "$actions" ] || { echo "gui-shot: no action script: $actions" >&2; exit 1; }

work="$(mktemp -d /tmp/writan-gui-XXXXXX)"
display=":${WRITAN_SHOTS_DISPLAY:-93}"
shots="$out"

cleanup() {
  [ -n "${app_pid:-}" ] && kill "$app_pid" 2>/dev/null || true
  [ -n "${xvfb_pid:-}" ] && kill "$xvfb_pid" 2>/dev/null || true
  if [ -z "$keep" ]; then rm -rf "$work"; else echo "gui-shot: kept $work"; fi
}
trap cleanup EXIT

# Fixture vault, so the list, tags and editor have something to show.
src_vault="${vault:-$repo/tools/visual/fixtures/vault}"
cp -r "$src_vault" "$work/vault"
mkdir -p "$work/config/com.writan.desktop" "$work/data" "$shots"
printf '{"vault_path": %s}' "\"$work/vault\"" \
  > "$work/config/com.writan.desktop/config.json"

Xvfb "$display" -screen 0 "${screen}x24" -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb_pid=$!
for _ in $(seq 1 50); do
  xdotool getdisplaygeometry >/dev/null 2>&1 && break
  sleep 0.1
done

env \
  DISPLAY="$display" \
  WRITAN_VAULT="$work/vault" \
  XDG_CONFIG_HOME="$work/config" \
  XDG_DATA_HOME="$work/data" \
  GTK_THEME="Adwaita:${theme}" \
  GSK_RENDERER=cairo \
  "$bin" >"$work/app.log" 2>&1 &
app_pid=$!

node "$repo/tools/visual/drive.mjs" \
  --backend x11 --display "$display" --out "$shots" --actions "$actions" $strict

echo "gui-shot: app log: $(wc -l <"$work/app.log") lines (see $work/app.log if kept)"
ls -1 "$shots" | sed 's/^/shot: /'
