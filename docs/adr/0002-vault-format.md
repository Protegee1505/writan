# ADR-0002: Vault format, index, and trash

Status: accepted. Date: 2026-10-16.

## Context

Notes must survive without Writan: readable, greppable, diffable, editable in any
tool. The index must never be a source of truth, and syncing the vault must not
sync app state.

## Decisions

1. **Plain Markdown + YAML frontmatter is the source of truth.** The body is
   never touched by Writan; only frontmatter is rewritten on save.
2. **Frontmatter keys:** `id` (UUID v4, stable identity), `title`, `tags`
   (list), `pinned` (written only when true), `created` / `updated` (RFC 3339
   UTC). Unknown keys are preserved verbatim (mapping round-trip). Defaults are
   removed from the file rather than written as `false` / `[]`.
3. **File naming:** `<slug>-<first 8 hex of id>.md`, made collision-free by
   suffixing. Identity is the `id`, never the path — renames and moves are
   harmless.
4. **Adoption:** any `.md` file in the vault without an `id` gets one on
   reconcile. Title comes from an existing `title:`, else the first `#` heading,
   else the file stem. Adoption timestamps are set at adoption time; the body is
   preserved.
5. **Atomic writes:** temp file in the same directory + fsync + rename. Temp
   names start with `.`; hidden files and directories are never indexed,
   watched, or walked.
6. **Index:** SQLite (rusqlite, bundled) at `<state-dir>/index.sqlite`, with
   FTS5 trigram tokenizer over title + body. Fully rebuildable at any time
   (`Workspace::rebuild_index`); never authoritative.
7. **Search:** user terms are always quoted, so raw input can never break MATCH
   syntax. Terms of ≥3 chars use trigram FTS with bm25 ranking; shorter terms
   (e.g. 2-char CJK words) fall back to LIKE substring search, which trigram
   cannot represent. Tag / folder / pinned filters compose with `AND`.
8. **Deletion:** notes move to `<state-dir>/trash/<id>.md` plus an `<id>.json`
   metadata sidecar (`original_path`, `deleted_at`, title, tags). Restore
   prefers the original path, else `<stem> (restored).md`. Trash lives outside
   the vault.
9. **Self-write suppression:** index rows carry `mtime_ns` + `size`; watcher
   events whose stat matches the row are skipped without re-parsing. `hash`
   (blake3 of the whole file) is recorded for future sync base comparisons.
10. **All app state** (index, trash, sync state, keys) stays under the OS
    app-data directory — never inside the vault.
11. **YAML parser (amended 2026-10-17):** frontmatter round-trips through a
    `serde_json` value tree with `preserve_order` (so key order survives
    rewrites), parsed and emitted by `serde-saphyr` (saphyr lineage, pure
    Rust). Strings that YAML 1.1 would read as timestamps or numbers are
    emitted quoted, so `created` / `updated` always re-parse as strings. This
    replaces `serde_yaml_ng`, which pulled the archived `unsafe-libyaml`
    (RUSTSEC-2024-0449).

## Consequences

- Vaults are portable and tool-agnostic; deleting the index costs a rebuild,
  never data.
- Our own saves bounce off the watcher as cheap no-ops.
- Unreadable or malformed notes are skipped and counted (`ReconcileStats.errors`),
  never auto-deleted or overwritten.
