# ADR-0001: Stack and architecture

- Status: accepted
- Date: 2026-10-16

## Context

Writan is a cross-platform (Linux, Windows, Android), offline-first note app
that must feel fast and lightweight. The user wants Markdown-compatible notes,
a polished UI, reliable sync without meaningful server bills, and end-to-end
encryption. The core should be Rust.

## Decisions

1. **App framework: Tauri 2.12 + Svelte 5 + TypeScript.**
   Tauri is the only mature shipped-UI framework that is Rust-centric on the
   backend, targets desktop *and* Android from one codebase, and yields small
   binaries for a webview app (~3–10 MB plus system webview). Dioxus 0.7 is
   the runner-up (but also renders through the system webview on desktop and
   mobile), Slint 1.18 would require building a rich-text editing model by
   hand, and Flutter + flutter_rust_bridge costs a second language and ~80–200 MB
   RAM. A native-Skia UI (Freya 0.5) stays an option: if WebKitGTK editing
   problems on Linux turn out to be fatal for CodeMirror, we run a one-week
   Freya spike at the end of M2.
2. **Editor: CodeMirror 6.** Notes are plain Markdown, and CM6 edits Markdown
   natively, so there is zero round-trip risk (TipTap/Milkdown convert between
   rich-text JSON and Markdown; TipTap also has an open Windows IME freeze bug
   in Tauri, tauri-apps/tauri#15436). Live preview is done with CM6
   decorations; `comrak` renders the read-only view.
3. **Storage: Markdown files are the source of truth.** A note is one `.md`
   file with YAML frontmatter (`id`, `title`, `tags`). SQLite (rusqlite,
   bundled, FTS5) is a *rebuildable* cache for search and metadata. Sync state,
   keys and the index live in the OS app-data directory, never in the vault.
4. **Sync: encrypted per-item blobs on dumb storage.** Each device uploads
   `items/<note-id>/<device>/<counter>.enc` plus tombstones. Storage targets:
   S3-compatible (Backblaze B2 free 10 GB is the documented default, Cloudflare
   R2 the faster alternative), WebDAV, a plain folder (Syncthing users), and
   the optional bundled server. No smart backend is required for the product to
   work.
5. **Merging: bounded three-way diff, never silent overwrites.** When two
   devices both edit a note, the later-syncing side runs a line-based diff3
   against the stored base (Joplin's 2026 model, including its bounds:
   max edit length 5,000 lines, 1 s time out). Unresolvable overlaps become a
   visible "conflict copy" file. Edits within ~60 s of each other resolve to
   the newer one (Notesnook-style window). CRDTs (Loro 1.16) were considered
   and are *not* used in v1; the merge logic sits behind a trait so real-time
   co-editing can be added later without changing the storage model.
6. **E2EE: Argon2id → KEK → XChaCha20-Poly1305.** Password-derived KEK
   (m = 19 MiB, t = 2, p = 1, OWASP 2026 minimum) unwraps a random vault key;
   blobs carry versioned headers and bind the note id as associated data.
   A recovery key export is mandatory at vault creation. Unlock material is
   kept in the OS keychain via `keyring` (Android Keystore where available).
7. **Optional server: `writan-server` (axum + SQLite, single binary/Docker).**
   It stores only blobs, exposes `/health` plus the blob API, supports
   email+password or QR device pairing, and pushes change events over SSE.
   It fits free tiers (Oracle Always Free ARM, or a home PC).

## Consequences

- ~85% of non-UI code is pure Rust crates, testable without a browser.
- Linux WebKitGTK editor bugs (contenteditable, IME) must be validated early
  in M2; Windows and Android use Chromium-based webviews and are low-risk.
- No vendor lock-in: losing the server only loses push, not data.
- Cost floor for users: $0 (B2/R2 free tiers; E2EE means the provider only
  ever stores ciphertext).
