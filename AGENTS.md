# AGENTS.md — working in the Writan repo

Writan is a fast, offline-first, E2EE-syncable Markdown note app. Markdown files
on disk are the source of truth; the SQLite index is a rebuildable cache. Core
logic is Rust (`crates/`), UI is Tauri 2 + Svelte 5 + TypeScript (`app/`).

Read before changing anything:

1. `docs/PROGRESS.md` — milestone history, current state, active blockers.
2. `docs/PLAN.md` — upcoming milestones, exit criteria, the next tasks,
   budgets, scope. Keep its status line current when a milestone moves.
3. `docs/adr/` — decisions already made (stack, vault format, sync). Add a new
   ADR before changing anything architectural (protocol, file format, storage,
   crypto); keep `docs/adr/README.md` (the index) current.

## Documentation maintenance

`PLAN.md`, `PROGRESS.md` and `adr/` are shared memory between agents: keep them
lean enough to read in full.

- `PLAN.md` = future: upcoming milestones + the next 3–5 tasks. Completed work
  leaves PLAN immediately; one summary line goes to `PROGRESS.md`.
- `PROGRESS.md` = past/present: milestone- and feature-level history with dates,
  one evidence line per entry (commit / CI run), and a short blockers section.
  No commit-by-commit logs, no daily journals, no code walkthroughs — point at
  `path:line` instead.
- ADRs = decisions: never rewrite or delete an accepted ADR; amend it with a
  dated note or supersede it with a new ADR, and keep `adr/README.md` current.
- Write for the next agent: if a line won't help someone continue the work,
  don't write it. One fact per line. Every update should hold or shrink the
  file — when you add, condense or delete something else.

## Definition of done (run before every commit)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix app run check
npm --prefix app run build
```

All green. Add tests for new core behavior: unit tests live next to modules in
`crates/writan-core/src/*.rs`, integration tests in `crates/writan-core/tests/`.

## Cloud-first workflow (dev machines are disposable)

- The cafe box resets on reboot and the Fedora laptop is tiny: GitHub is the
  source of truth. Push before you stop working, every session.
- Run agents in the cloud, not on a local machine. Primary host: Google Cloud
  Shell (`scripts/cloudshell-setup.sh` — the free models answer from its IP) —
  browser → Cloud Shell → OpenChamber on port 3000. The Codespaces
  environment (`.devcontainer/`) is the optional full-toolchain alternative.
  Daily flow, quotas and platform builds: `docs/CLOUD.md`.
- Keep heavy work in CI: `ci.yml` verifies every push, `artifacts.yml` builds
  installers, `web.yml` builds and deploys the Pages preview.
- Local machines are for light checks (`cargo fmt`, `npm --prefix app run
  check`) and for running downloaded artifacts to eyeball the real app.

## Layout

- `crates/writan-core` — vault (Markdown + frontmatter), index (SQLite + FTS5
  trigram), watcher, trash, workspace orchestration. Most logic belongs here.
- `crates/writan-crypto` — Argon2id KEK derivation, XChaCha20-Poly1305 seal/open.
- `crates/writan-sync` — device ids, version vectors (sync engine lands in M3).
- `crates/writan-server` — optional self-hosted sync server (axum; grows in M4).
- `app/` — Tauri app. Rust side: `app/src-tauri` (`state.rs`, `commands.rs`,
  `lib.rs`). Svelte side: `app/src` (runes store `lib/store.svelte.ts`,
  components `lib/components/`, typed IPC `lib/api.ts`).

## Invariants (do not break without an ADR)

- Markdown files are the source of truth; the index must always be rebuildable
  from files alone. Never edit notes only in the database.
- Never write app state (config, index, trash, keys) inside the vault
  directory — only in the OS app-data directory.
- E2EE parameters: Argon2id m=19 MiB, t=2, p=1 → KEK wraps the vault key;
  XChaCha20-Poly1305 with a 24-byte nonce prefix; note id as AAD. A recovery-key
  export is mandatory before sync ships.
- Merge: bounded diff3 (max 5 000 lines / 1 s) + ~60 s LWW window + explicit
  conflict copies; never silent overwrites; no CRDT in v1 (keep merge behind a
  trait).
- No telemetry.

## Conventions

- Rust: edition 2024 (let-chains fine); errors are `thiserror` enums in core
  crates; Tauri commands map errors to `String` at the boundary; DTOs use
  `#[serde(rename_all = "camelCase")]`.
- Frontend: Svelte 5 runes (`$state`/`$effect`, stores in `.svelte.ts`),
  TypeScript strict; talk to Rust only through `lib/api.ts`.
- Tauri 2 auto-converts camelCase JS arguments to snake_case Rust parameters.
- Settings-first: every new user-visible feature registers its options in the
  settings registry (`app/src/lib/settings.svelte.ts`), exposes its actions in
  the command palette, and adds global shortcuts to `app/src/lib/keymap.ts` —
  no hardcoded key handlers. Preferences persist to app-data `config.json`
  (ADR-0003), never to localStorage or the vault.
- Commit messages: `Mn: summary` for milestone work, `docs:` / `fix:` otherwise.

## Environment notes

- Windows: if `cargo` is not on PATH in a fresh shell, use
  `%USERPROFILE%\.cargo\bin\cargo.exe`. PowerShell may print a cosmetic
  exit-code-1 / NativeCommandError when cargo logs to stderr.
- If Tauri build scripts fail with `os error 3` and paths from an old checkout
  location, delete `target/debug/build` folders matching `^(tauri|wry|writan-app)`
  and rebuild.
- Run the app with `npm --prefix app run tauri dev`; it asks for a notes folder
  on first run and remembers it in `config.json` under the app-data dir.
