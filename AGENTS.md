# AGENTS.md — working in the Writan repo

Writan is a fast, offline-first, E2EE-syncable Markdown note app. Markdown files
on disk are the source of truth; the SQLite index is a rebuildable cache. Core
logic is Rust (`crates/`), UI is Tauri 2 + Svelte 5 + TypeScript (`app/`).

Read before changing anything:

1. `docs/PROGRESS.md` — current state, verification evidence, next actions.
2. `docs/PLAN.md` — milestones, exit criteria, budgets, scope. Keep its status
   line current when a milestone moves.
3. `docs/adr/` — decisions already made (stack, vault format). Add a new ADR
   before changing anything architectural (protocol, file format, storage,
   crypto).

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
