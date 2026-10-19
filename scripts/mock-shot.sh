#!/usr/bin/env bash
# Drive the mock-mode frontend in headless Chromium and capture screenshots.
#
#   scripts/mock-shot.sh --serve          # starts Vite too, stops it afterwards
#   scripts/mock-shot.sh                  # uses an already running dev server
#   scripts/mock-shot.sh --url http://127.0.0.1:5173/?mock=1
#   scripts/mock-shot.sh --size 390x844   # phone-sized window (mobile.actions)
#
# This is the fastest loop in the harness: seconds per iteration, real Svelte
# UI, in-memory backend (app/src/lib/mock.ts). Chromium is the same engine as
# WebView2, so what it renders is a close preview of the Windows app.
#
# Needs a Chromium binary: either set CHROME_BIN, or `npx playwright install
# chromium-headless-shell` once (the script finds it under the Playwright
# browsers cache, $XDG_CACHE_HOME/ms-playwright or ~/.cache/ms-playwright).
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
url="http://127.0.0.1:5173/?mock=1"
out="/tmp/writan-mock-shots"
actions="$repo/tools/visual/actions/tauri.actions"
size="1200,800"
port="9222"
serve=""
strict=""

while [ $# -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --actions) actions="$2"; shift 2 ;;
    --size) size="$2"; shift 2 ;;
    --serve) serve=1; shift ;;
    --strict) strict="--strict"; shift ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "mock-shot: unknown option $1" >&2; exit 2 ;;
  esac
done

chrome="$(
  if [ -n "${CHROME_BIN:-}" ]; then echo "$CHROME_BIN";
  else find "${PLAYWRIGHT_BROWSERS_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/ms-playwright}" \
      "$HOME/.cache/ms-playwright" \
      -maxdepth 3 -type f \( -name chrome -o -name chrome-headless-shell -o -name headless_shell \) 2>/dev/null |
    sort | sed -n 1p || true;
  fi
)"
[ -n "$chrome" ] || {
  echo "mock-shot: no Chromium found; run: npx playwright install chromium-headless-shell, or set CHROME_BIN" >&2
  exit 1
}
# The playwright headless shell is always headless and rejects --headless flags.
headless="--headless=new"
case "$(basename "$chrome")" in headless_shell | chrome-headless-shell) headless="" ;; esac

[ -d "$repo/tools/visual/node_modules" ] || {
  echo "mock-shot: driver deps missing; run: npm ci --prefix tools/visual" >&2
  exit 1
}

cleanup() {
  [ -n "${chrome_pid:-}" ] && kill "$chrome_pid" 2>/dev/null || true
  [ -n "${serve_pid:-}" ] && kill "$serve_pid" 2>/dev/null || true
}
trap cleanup EXIT

if [ -n "$serve" ]; then
  (cd "$repo/app" && npm run dev -- --host 127.0.0.1 --port 5173 --strictPort) \
    >/tmp/writan-mock-vite.log 2>&1 &
  serve_pid=$!
  ok=""
  for _ in $(seq 1 60); do
    curl -sf -o /dev/null "$url" && { ok=1; break; }
    sleep 0.5
  done
  [ -n "$ok" ] || { echo "mock-shot: dev server did not come up (see /tmp/writan-mock-vite.log)" >&2; exit 1; }
fi

profile="$(mktemp -d /tmp/writan-chrome-XXXXXX)"
"$chrome" \
  $headless --no-sandbox --disable-gpu --hide-scrollbars \
  --user-data-dir="$profile" \
  --remote-debugging-port="$port" \
  --window-size="$size" \
  "$url" >/tmp/writan-chrome.log 2>&1 &
chrome_pid=$!

for _ in $(seq 1 40); do
  curl -sf -o /dev/null "http://127.0.0.1:$port/json/version" && break
  sleep 0.5
done

node "$repo/tools/visual/drive.mjs" \
  --backend cdp --cdp "http://127.0.0.1:$port" --out "$out" --actions "$actions" $strict
rm -rf "$profile"
ls -1 "$out" | sed 's/^/shot: /'
