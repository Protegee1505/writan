#!/usr/bin/env bash
# Runs on every start/resume of the codespace: bring the agent UI back up.
set -uo pipefail
export PATH="$HOME/.local/bin:$HOME/.opencode/bin:$PATH"

if pgrep -f "openchamber" >/dev/null 2>&1; then
  echo "OpenChamber already running (port 3000)."
  exit 0
fi

args=(--port 3000)
# Optional extra lock: set OPENCHAMBER_UI_PASSWORD as a Codespaces secret
# (github.com/settings/codespaces). Forwarded ports already require a GitHub
# login; this adds a UI password on top.
if [ -n "${OPENCHAMBER_UI_PASSWORD:-}" ]; then
  args+=(--ui-password "$OPENCHAMBER_UI_PASSWORD")
fi

nohup openchamber "${args[@]}" >"$HOME/.openchamber-server.log" 2>&1 &
echo "OpenChamber starting on port 3000 (log: ~/.openchamber-server.log)."
