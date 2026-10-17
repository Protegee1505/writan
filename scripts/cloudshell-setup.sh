#!/usr/bin/env bash
# Writan Cloud Shell bootstrap — safe to re-run every session.
#
#   curl -fsSL https://raw.githubusercontent.com/Protegee1505/writan/main/scripts/cloudshell-setup.sh | bash
#
# Google Cloud Shell persists $HOME (5 GB) but resets /usr, so this script
# (re)creates what each session needs and starts the agent UI on port 3000
# (Cloud Shell → Web Preview → port 3000). Heavy builds belong in GitHub CI.
set -euo pipefail

info() { printf '\n== %s\n' "$1"; }

REPO_DIR="${WRITAN_DIR:-$HOME/writan}"
REPO_URL="https://github.com/Protegee1505/writan.git"

# ---------------------------------------------------------------- repository
info "repository"
if [ ! -d "$REPO_DIR/.git" ]; then
  git clone "$REPO_URL" "$REPO_DIR"
else
  git -C "$REPO_DIR" status --short --branch
fi

# User-level installs (opencode, openchamber) live on $HOME, which persists.
export PATH="$HOME/.opencode/bin:$HOME/.local/bin:$PATH"

# ---------------------------------------------------------------------- node
# OpenChamber needs Node >= 24.14; nvm keeps it on $HOME across sessions.
info "node"
export NVM_DIR="$HOME/.nvm"
if [ -s "$NVM_DIR/nvm.sh" ]; then
  # shellcheck disable=SC1091
  . "$NVM_DIR/nvm.sh"
fi
node_major=""
if command -v node >/dev/null 2>&1; then
  node_major="$(node --version | sed -n 's/^v\([0-9]*\).*/\1/p')"
fi
if [ -z "$node_major" ] || [ "$node_major" -lt 24 ]; then
  if ! command -v nvm >/dev/null 2>&1; then
    curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
    # shellcheck disable=SC1091
    . "$NVM_DIR/nvm.sh"
  fi
  nvm install 24
  nvm alias default 24 >/dev/null
fi
if command -v nvm >/dev/null 2>&1; then
  nvm use default >/dev/null 2>&1 || true
fi
node --version

# ------------------------------------------------------------------ opencode
info "opencode"
if ! command -v opencode >/dev/null 2>&1; then
  curl -fsSL https://opencode.ai/v2/install | bash
fi
opencode --version

# ---------------------------------------------------------------- openchamber
info "openchamber"
if ! command -v openchamber >/dev/null 2>&1; then
  npm config set allow-scripts=node-pty --location=user
  curl -fsSL https://raw.githubusercontent.com/openchamber/openchamber/main/scripts/install.sh | bash
  export PATH="$HOME/.local/bin:$PATH"
fi
if ! command -v openchamber >/dev/null 2>&1; then
  echo "openchamber is not on PATH after install" >&2
  exit 1
fi

# ------------------------------------------------------------ agent UI :3000
info "agent UI"
http_code() { curl -s -o /dev/null -w '%{http_code}' http://localhost:3000/ 2>/dev/null || true; }
code="$(http_code)"
if [ "$code" != "200" ]; then
  cd "$REPO_DIR"
  nohup openchamber --port 3000 > "$HOME/.openchamber-server.log" 2>&1 &
  for _ in $(seq 1 25); do
    sleep 1
    code="$(http_code)"
    if [ "$code" = "200" ]; then
      break
    fi
  done
fi

if [ "$code" = "200" ]; then
  cat <<'EOF'

OpenChamber is up on port 3000.
  Cloud Shell -> Web Preview (top-right) -> Change port -> 3000.

A plain agent TUI also works:  cd ~/writan && opencode

Quota habits: heavy builds belong in GitHub CI; the clone lives in ~/writan
— push your work before closing the session.
EOF
else
  echo "OpenChamber did not answer on :3000 — see ~/.openchamber-server.log" >&2
  exit 1
fi
