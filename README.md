# Writan

A modern, fast, offline-first note-taking app for Linux, Windows and Android.
Notes are plain Markdown files on disk; everything else is built around that.

- **Core in Rust** — vault, crypto, sync and the optional server are Rust crates.
- **Desktop & Android UI** — Tauri 2 + Svelte 5 + TypeScript.
- **Editor** — CodeMirror 6; Markdown is the source of truth.
- **Search** — SQLite FTS5 with trigram tokenizer (works for CJK and substrings).
- **Sync** — end-to-end encrypted notes pushed to *dumb* storage (any S3-compatible
  bucket, WebDAV, a plain folder, or the bundled Writan server). Free tiers are
  enough: Backblaze B2 (10 GB free, no card) or Cloudflare R2 are the reference
  backends.
- **Merging** — bounded three-way diff of the Markdown body; unresolvable edits
  become explicit "conflict copy" files, never silent overwrites.

## Status

Active development. **M0 (bootstrap), M1 (vault core) and the first pass of
M2 (desktop MVP) are complete** — the app can already open a notes folder,
create/edit/search/pin/trash notes with autosave and external-edit tracking.

- Milestones & exit criteria: [`docs/PLAN.md`](docs/PLAN.md)
- Living progress log + next actions: [`docs/PROGRESS.md`](docs/PROGRESS.md)
- Decisions: [`docs/adr/`](docs/adr/)
- Working with AI agents in this repo: [`AGENTS.md`](AGENTS.md)

## Try it (development)

Requires a stable Rust toolchain (MSVC on Windows) and Node.js 22+.

```sh
npm --prefix app install
npm --prefix app run tauri dev   # opens the app window; first compile is slow
```

On first run the app asks for a notes folder; it is remembered in `config.json`
under the OS app-data directory. Nothing app-related is ever written into the
vault except your Markdown files.

## Verify

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix app run check
npm --prefix app run build
```

## Repository layout

```
crates/
  writan-core/     vault (Markdown + frontmatter), SQLite/FTS5 index,
                   file watcher, trash, workspace orchestration
  writan-crypto/   Argon2id key derivation, XChaCha20-Poly1305 envelopes
  writan-sync/     device ids, version vectors, merge logic, storage targets
  writan-server/   optional self-hostable sync server (axum + SQLite)
app/               Tauri 2 + Svelte 5 application (desktop + Android)
docs/adr/          architecture decision records
```

## License

MIT OR Apache-2.0 (both files land in M6; see `docs/adr/0001-stack.md`).
