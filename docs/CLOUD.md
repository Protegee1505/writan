# Cloud-first development

The dev machines are disposable: the cafe box resets on every reboot and the
Fedora laptop has ~5 GB free. Nothing that matters may live on them — **the
repo + CI are the workspace**, and local machines are browsers. The agent
host is Google Cloud Shell (the free models answer from its IP); this
document is the workflow guide for that setup.

## What runs where

| Work | Where | How |
| --- | --- | --- |
| Agent sessions (edit/run/test) | **Cloud Shell** | `scripts/cloudshell-setup.sh` → OpenChamber via Web Preview on port 3000; free models answer here |
| Optional full editor env (cargo/Vite/apt) | Codespace | OpenChamber on forwarded port 3000 — free models do **not** answer there |
| fmt / clippy / tests / deny | GitHub Actions | `ci.yml`, every push |
| Installers (Windows/Linux) | GitHub Actions | `artifacts.yml` (dispatch or `v*` tag) |
| Web preview | Actions → Pages | `web.yml` (main pushes touching `app/`, tags, manual) |
| Eyeballing the running app | cafe box / laptop | download artifacts (`gh run download`) |
| GTK shell IME acceptance | Fedora laptop | needs a real desktop + ibus/fcitx5 |
| Android device test (M5) | phone | sideload the debug APK from `artifacts.yml` |

## Daily flow — Cloud Shell (the agent host)

1. **Open** https://shell.cloud.google.com (no card; 50 h/week; `$HOME`
   persists, `/usr` resets).
2. **Bootstrap** — one line, safe to re-run every session:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/Protegee1505/writan/main/scripts/cloudshell-setup.sh | bash
   ```

   It clones `~/writan`, ensures Node 24+ / OpenCode / OpenChamber, and starts
   the agent UI.
3. **Open the agent UI** — Web Preview (top-right) → Change port → 3000. The
   free `opencode/*` models answer from Cloud Shell's IP (measured 2026-10-17),
   so no key or sign-in is needed.
4. **Work**, then **push.** `ci.yml` verifies every push; a push to `main`
   touching `app/` also refreshes the Pages preview.
5. Close the tab when done — VM time stops with the session.

Plain-TUI alternative inside Cloud Shell: `cd ~/writan && opencode`.

## Daily flow — codespace (optional full-toolchain env)

1. **Open or resume the codespace** — github.com/codespaces (or repo → Code →
   Codespaces). Resuming takes ~30 s; creating a fresh one builds the
   devcontainer (a few minutes, once).
2. **Agent UI** — Ports tab → port 3000 ("OpenChamber"). The free
   `opencode/*` models do *not* answer from codespace IPs — drive the agent
   from Cloud Shell instead, or add a provider key here (see "Model access
   from the cloud").
3. **Work.** Agents edit files and run `cargo fmt/clippy/test`,
   `npm --prefix app run check` — the image carries the same apt deps as CI,
   so compiles work here.
4. **Push.** Same as above.
5. **Stop the codespace** when done (it also auto-stops after ~30 min idle).
   Stopping is the one quota habit that matters.

CLI alternative: `gh codespace list|start|stop|ssh -c <name>` — needs the
`codespace` scope once: `gh auth refresh -s codespace`.

## Model access from the cloud

Measured 2026-10-17: the free `opencode/*` models are gated by **source IP**.

- **Codespaces (Azure IPs): blocked** — `Error: Rate limit exceeded. Please
  try again later.` for every free model, anonymous or signed in (Console
  Personal), over 15+ minutes of retries.
- **Cloud Shell (Google Cloud IPs): works** — verified with
  `opencode run --model opencode/deepseek-v4.1-flash-free` (exit 0).
- **Local residential IPs: work** — the cafe box answers instantly.

That is why the agent host is Cloud Shell: the free models need no key there.
If a host is ever blocked again, the fallbacks are:

- sign in from that host (OpenChamber → Providers, or `opencode auth login`);
- a free, no-card API key from another provider (Google AI Studio, Groq,
  OpenRouter `:free`, ...): `opencode auth login <provider>` in that host
  (auth persists in its `$HOME`), or a Codespaces secret;
- GitHub Copilot via `opencode auth login github-copilot` (needs a seat or
  the free tier; the cafe box's `GITHUB_TOKEN` integration is not licensed).

IP gates can decay — a blocked host is worth re-trying a day later.

## Quota discipline (free tiers, no card)

- **Cloud Shell:** 50 h/week VM time (≈ 7 h/day); sessions end on idle or tab
  close. `$HOME` is 5 GB — keep big `target/` out; heavy builds belong in CI.
- **Codespaces:** 120 core-hours/month ⇒ a 2-core machine runs ≈ 60
  clock-hours (≈ 3 h/day, every day). 4-core burns 2× per minute — short
  bursts only. 15 GB-month storage counts while stopped too; delete stale
  codespaces (idle ones auto-delete after 30 days). Exceeding the quota
  pauses Codespaces until the next month — it can never bill you.
- Actions on this public repo uses unmetered standard runners: prefer CI for
  anything heavy (`gh workflow run artifacts.yml`).
- Pairing/relay keeps devices *reachable*, not a codespace *awake*: wake it
  first (`gh codespace start`, or github.com/codespaces).

## Cloud Shell notes

- `$HOME` (5 GB) persists between sessions — `~/writan`, nvm's Node 24,
  OpenCode and OpenChamber all survive. `/usr` resets, so anything
  `apt`-installed is gone next session (the setup script needs no apt).
- Optional full toolchain in Cloud Shell: `sudo apt-get install` the Tauri
  deps (see `ci.yml`) plus `rustup` — enough for `cargo fmt`/`clippy` loops,
  but the 5 GB home is tight for a real `target/`; keep builds in CI.
- The Web Preview URL is Google-account-gated, so the UI is private without a
  password; set `OPENCHAMBER_UI_PASSWORD` and pass `--ui-password` for one on
  top.
- When a session ends, re-run the setup line — work lives in the repo and in
  `$HOME`.

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

- Cloud Shell: `$HOME` persists while the account keeps using it (Google
  reclaims it after long inactivity). `~/writan` carries your clone and any
  unpushed work — still push before ending a session.
- Codespaces: sessions and provider auth persist in the codespace `$HOME`
  while it exists; deleting a codespace loses only agent chat history.
- The durable record is the repo: `docs/PROGRESS.md` (current state + next
  actions), commits, and GitHub issues/PRs if used. End every session with a
  pushed commit — that is the resume point.
- Nothing secret is in the repo: provider sign-in is per codespace; optional
  Codespaces secrets (e.g. `OPENCHAMBER_UI_PASSWORD`) live in the GitHub
  account, not here.

## Security notes

- Cloud Shell's Web Preview URL is behind your Google account, not public.
- Forwarded ports are private, GitHub-authenticated URLs; an optional
  `OPENCHAMBER_UI_PASSWORD` Codespaces secret adds a UI password on top.
- The repo-root `opencode.json` mirrors the agent permission profile
  (ask-by-default bash with an allowlist for `git/gh/npm/cargo/...`) so
  codespace and local agents behave the same.

## Rollback

Everything here is additive — the local workflow is untouched:

1. `git revert` the cloud commits (`.devcontainer/`, `scripts/`, `web.yml`,
   doc edits).
2. Delete the codespace (github.com/codespaces) — quota released.
3. Cloud Shell keeps nothing after `rm -rf ~/writan` (or Google's expiry).
3. Settings → Pages → Unpublish (the preview holds no state).
4. No repo secrets exist, so nothing to rotate.

## Troubleshooting

- OpenChamber missing after a resume: `bash .devcontainer/post-start.sh`, or
  check `openchamber status` / `~/.openchamber-server.log`.
- Fresh codespace missing tools: re-run `bash .devcontainer/post-create.sh`
  (its output is logged to `~/.devcontainer-post-create.log`).
- `gh codespace ssh` on Windows can hang after printing output. Prefer a
  direct ssh: `gh codespace ssh --config | Out-File -FilePath $env:TEMP\cs-ssh.cfg`
  then `ssh -F $env:TEMP\cs-ssh.cfg cs.<name>.main '<base64-decode | bash>'` —
  with `GH_TOKEN`/`GITHUB_TOKEN` cleared in the shell (the env token has no
  `codespace` scope).
- First bring-up (2026-10-17) needed: the `sshd` devcontainer feature for
  `gh codespace ssh`, and Node ≥24 for OpenChamber (now pinned in
  `.devcontainer/`).
- Cloud Shell: re-run the setup line; log at `~/.openchamber-server.log`;
  if Web Preview is blank, check port 3000 answers locally:
  `curl -s -o /dev/null -w '%{http_code}\n' http://localhost:3000/`.
