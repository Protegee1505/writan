# Writan

A modern, fast, offline-first note-taking app for Linux, Windows and Android.
Notes are plain Markdown files on disk; everything else is built around that.

- **Core in Rust** — vault, crypto, sync and the optional server are Rust crates.
- **Desktop & Android UI** — Tauri 2 + Svelte 5 + TypeScript.
- **Editor** — CodeMirror 6 with live preview; Markdown is the source of truth.
- **Sync** — end-to-end encrypted notes pushed to *dumb* storage (any S3-compatible
  bucket, WebDAV, a plain folder, or the bundled Writan server). Free tiers are
  enough: Backblaze B2 (10 GB free, no card) or Cloudflare R2 are the reference
  backends.
- **Merging** — bounded three-way diff of the Markdown body; unresolvable edits
  become explicit "conflict copy" files, never silent overwrites.

## Repository layout

```
crates/
  writan-core/     domain types, vault model, frontmatter handling
  writan-crypto/   Argon2id key derivation, XChaCha20-Poly1305 envelopes
  writan-sync/     version vectors, merge logic, storage targets
  writan-server/   optional self-hostable sync server (axum + SQLite)
app/               Tauri 2 + Svelte 5 application (desktop + Android)
docs/adr/          architecture decision records
```

## Development

Requires a stable Rust toolchain (MSVC on Windows) and Node.js 22+.

```sh
cargo test --workspace     # core crates
cargo clippy --workspace --all-targets -- -D warnings
```

The desktop app lives in `app/` (see `docs/adr/0001-stack.md` for the full
rationale). Status: **M0 — bootstrap**.

## License

MIT OR Apache-2.0 (final choice recorded in ADR-0001; both files land in M6).
