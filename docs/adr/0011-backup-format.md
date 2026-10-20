# ADR-0011: Backup and import format

Status: accepted. Date: 2026-10-20.

## Context

- M6 needs the last missing v1 feature: a user can move a vault between
  machines, keep an offline copy, and bring an existing Markdown folder in.
  Sync (M3–M4) is not a backup — a delete propagates — and Android v1 keeps the
  vault app-private, so uninstalling the app loses it (ADR-0009).
- The vault is already plain Markdown + frontmatter + attachments on disk.
  A backup could be a folder copy, but one file is easier to move and to keep
  next to other backups, and it can carry a manifest.
- The E2EE primitives in `writan-crypto` seal one buffer with one nonce
  (`seal`/`open`, 24-byte prefix). A passphrase-protected archive of a large
  vault needs a chunked, streaming AEAD format and its own design; that is not
  a v1 blocker while plaintext files are already on disk.

## Decisions

1. **A backup is a plaintext zip**, format marker `writan-backup/1`, with
   `writan-backup.json` as the first entry:
   `{format, created, appVersion, notes, attachments, bytes}`. The manifest is
   bookkeeping, not content: import ignores it and does not count it as
   skipped. The UI says "unencrypted" next to export. Passphrase protection is
   a follow-up with its own ADR (chunked AEAD, not the one-shot `seal`).
2. **Import only ever adds.** No file already in the vault is overwritten:
   - a note whose id already exists gets a fresh UUID and a ` (imported)` title
     suffix (not doubled on re-import); `ImportReport.renamed` counts these;
   - a path collision gets a unique name (`map-2.png`);
   - text from foreign Markdown gets `created`/`updated` = now and a path
     derived from its title + an 8-char id under its source directory.
3. **`import_path` accepts a zip, a folder, or a single `.md`.** Folder import
   adopts plain Markdown, so `~/Notes` from another app can be brought in
   without rewriting it first.
4. **Hostile input is rejected or neutralized** before any write: zip-slip
   paths, absolute paths and drive letters; Windows reserved device names
   (`CON.md` → `_CON.md`); dangerous characters are replaced, not dropped;
   symlinks, dotfiles, OS junk (`Thumbs.db`, `desktop.ini`, `.DS_Store`,
   `__MACOSX`) are skipped and counted. Caps: 100 000 entries, 2 GiB total,
   256 MiB per entry, checked from the central directory in a pre-pass.
   Encrypted archives are refused with a clear error.
5. **Export is safe and repeatable**: it walks only real Markdown/attachment
   files, skips dotfiles/symlinks/OS junk, refuses a destination inside the
   vault (it would be indexed and backed up recursively), writes to a sibling
   temp file and renames. Folder export refuses a non-empty folder unless it is
   an earlier backup (has a manifest). Default desktop name:
   `writan-backup-YYYY-MM-DD.zip`.
6. **Archive codecs**: zip crate 9 with `deflate-flate2-zlib-rs` only (pure
   Rust; no C zlib, no AES/zstd features). Text is deflated, images/media are
   stored; entries are `0o644`.
7. **Android export/import is hidden in v1.** Rust cannot write an SAF/content
   URI, and the app-private vault has no user-visible location; export there is
   a share-sheet follow-up. The desktop shells expose the actions.

## Consequences

- A backup is exactly as private as the vault on disk: plaintext. Users are
  told so; it is not a substitute for sync encryption.
- Imported files are ordinary vault files, so the index stays rebuildable and
  both shells must `reconcile()` after an import to show the new notes.
- The format is versioned and the manifest is optional on read, so a folder
  copied by hand still imports.
