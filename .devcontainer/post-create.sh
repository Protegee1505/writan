#!/usr/bin/env bash
# One-time setup for a fresh codespace / dev container.
# Re-runnable by hand if a network step failed:
#   bash .devcontainer/post-create.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Keep user-level installs on PATH for this script and future shells.
export PATH="$HOME/.local/bin:$HOME/.opencode/bin:$PATH"
if ! grep -qs "writan-cloud: user-level installs" "$HOME/.bashrc"; then
  printf '\n# writan-cloud: user-level installs\nexport PATH="$HOME/.local/bin:$HOME/.opencode/bin:$PATH"\n' >> "$HOME/.bashrc"
fi

# Git identity — matches the existing commit log; env can override.
git config --global user.name "${GIT_USER_NAME:-Protegee1505}"
git config --global user.email "${GIT_USER_EMAIL:-Protegee1505@users.noreply.github.com}"

# Frontend + Rust dependencies, so the first agent run is warm.
# (The first cargo call also installs the rust-toolchain.toml toolchain.)
npm ci --prefix app
cargo fetch

# The agent stack: OpenCode (agent) + OpenChamber (its web GUI).
command -v opencode >/dev/null || curl -fsSL https://opencode.ai/v2/install | bash
command -v openchamber >/dev/null || curl -fsSL https://raw.githubusercontent.com/openchamber/openchamber/main/scripts/install.sh | bash

echo "post-create done: OpenChamber starts on port 3000 via post-start."
