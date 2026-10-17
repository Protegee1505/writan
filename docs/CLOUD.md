# Cloud-first development

The dev machines are disposable: the cafe box resets on every reboot and the
Fedora laptop has ~5 GB free. Nothing that matters may live on them — **the
repo + CI are the workspace**, and local machines are browsers. This document
is the workflow guide for that setup.

## What runs where

| Work | Where | How |
| --- | --- | --- |
| Agent sessions (edit/run/test) | Codespace | OpenChamber UI on forwarded port 3000 |
| fmt / clippy / tests / deny | GitHub Actions | `ci.yml`, every push |
| Installers (Windows/Linux) | GitHub Actions | `artifacts.yml` (dispatch or `v*` tag) |
| Web preview | Actions → Pages | `web.yml` (main pushes touching `app/`, tags, manual) |
| Eyeballing the running app | cafe box / laptop | download artifacts (`gh run download`) |
| GTK shell IME acceptance | Fedora laptop | needs a real desktop + ibus/fcitx5 |
| Android device test (M5) | phone | sideload the debug APK from `artifacts.yml` |

## Daily flow

1. **Open or resume the codespace** — github.com/codespaces (or repo → Code →
   Codespaces). Resuming takes ~30 s; creating a fresh one builds the
   devcontainer (a few minutes, once).
2. **Open the agent UI** — Ports tab → port 3000 ("OpenChamber"); optionally
   install it as a PWA. First time in a fresh codespace: sign in to the model
   provider (`/connect` in OpenCode or OpenChamber → Providers — the free
   `opencode/deepseek-v4.1-flash-free` model works like on the cafe box).
3. **Work.** Agents edit files and run `cargo fmt/clippy/test`,
   `npm --prefix app run check` — the image carries the same apt deps as CI,
   so compiles work here.
4. **Push.** `ci.yml` verifies every push; a push to `main` touching `app/`
   also refreshes the Pages preview.
5. **Stop the codespace** when done (it also auto-stops after ~30 min idle).
   Stopping is the one quota habit that matters.

CLI alternative: `gh codespace list|start|stop|ssh -c <name>` — needs the
`codespace` scope once: `gh auth refresh -s codespace`.

## Quota discipline (personal free tier, no card)

- 120 core-hours/month ⇒ a 2-core machine runs ≈ 60 clock-hours (≈ 3 h/day,
  every day). 4-core burns 2× per minute — use for short bursts only.
- 15 GB-month storage counts while stopped too; delete stale codespaces
  (idle ones auto-delete after 30 days).
- Exceeding the quota pauses Codespaces until the next month — it can never
  bill you.
- Actions on this public repo uses unmetered standard runners: prefer CI for
  anything heavy (`gh workflow run artifacts.yml`), keep the codespace for
  interactive loops.
- Pairing/relay keeps devices *reachable*, not the codespace *awake*: wake it
  first (`gh codespace start`, or github.com/codespaces).

## Fallback: Google Cloud Shell (50 h/week, no card)

Use when the Codespaces quota runs out or a codespace misbehaves. The VM is
fresh every session (`/usr` resets; 5 GB `$HOME` persists), so:

```sh
git clone https://github.com/Protegee1505/writan && cd writan
sudo apt-get update && sudo apt-get install -y build-essential pkg-config \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
  libxdo-dev libssl-dev libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev
curl -fsSL https://opencode.ai/v2/install | bash
curl -fsSL https://raw.githubusercontent.com/openchamber/openchamber/main/scripts/install.sh | bash
openchamber   # then Cloud Shell's Web Preview → port 3000
```

The 5 GB home is tight for a full Rust `target/` — keep real builds in CI.

## Platform builds

- **Windows** — `artifacts.yml` → `writan-windows-x64` (portable exe, MSI,
  NSIS). Unsigned, so SmartScreen warns on first run; a free OSS signing
  route (SignPath.io) exists if wanted later.
- **Linux** — `artifacts.yml` → `writan-linux-x64` (GTK shell binary +
  measurements). Tauri deb/rpm/AppImage builds ride the same ubuntu runner
  when needed.
- **Android (M5)** — run `tauri android init` inside a codespace (add JDK 17
  + SDK/NDK then), commit `app/src-tauri/gen/android`, add the `android` job
  (debug APK). Device install = sideload.
- **Web** — `web.yml` → Pages build of the static SPA shell
  (https://protegee1505.github.io/writan/). Making it *functional* in the
  browser (the `invoke`-based code paths) is a scope decision + ADR, planned
  around M5/M6.
- **macOS/iOS** — out of scope by decision (Apple toolchain + paid signing).

## Resuming agent work after a wipe

- Codespaces: sessions and provider auth persist in the codespace `$HOME`
  while it exists; deleting a codespace loses only agent chat history.
- The durable record is the repo: `docs/PROGRESS.md` (current state + next
  actions), commits, and GitHub issues/PRs if used. End every session with a
  pushed commit — that is the resume point.
- Nothing secret is in the repo: provider sign-in is per codespace; optional
  Codespaces secrets (e.g. `OPENCHAMBER_UI_PASSWORD`) live in the GitHub
  account, not here.

## Security notes

- Forwarded ports are private, GitHub-authenticated URLs; an optional
  `OPENCHAMBER_UI_PASSWORD` Codespaces secret adds a UI password on top.
- The repo-root `opencode.json` mirrors the agent permission profile
  (ask-by-default bash with an allowlist for `git/gh/npm/cargo/...`) so
  codespace and local agents behave the same.

## Rollback

Everything here is additive — the local workflow is untouched:

1. `git revert` the cloud commits (`.devcontainer/`, `web.yml`, doc edits).
2. Delete the codespace (github.com/codespaces) — quota released.
3. Settings → Pages → Unpublish (the preview holds no state).
4. No repo secrets exist, so nothing to rotate.

## Troubleshooting

- OpenChamber missing after a resume: `bash .devcontainer/post-start.sh`, or
  check `openchamber status` / `~/.openchamber-server.log`.
- Fresh codespace missing tools: re-run `bash .devcontainer/post-create.sh`.
