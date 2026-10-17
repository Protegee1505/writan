# ADR-0005: Sync blob format and key hierarchy

Status: accepted. Date: 2026-10-17.

## Context

- PLAN's sync model: per-note encrypted blobs on dumb targets (folder,
  S3-compatible, WebDAV, writan-server), Joplin-style; version vectors; E2EE
  with Argon2id → KEK wrapping a vault key; XChaCha20-Poly1305 with the note id
  as associated data; bounded diff3 merge with explicit conflict copies.
- The primitives already exist: `writan-sync` has `DeviceId` (Uuid) and
  `VersionVector` (`bump` / `observe` / `merge` / `contains` /
  `is_concurrent_with`); `writan-crypto` has `derive_kek` (Argon2id, m = 19
  MiB, t = 2, p = 1, 16-byte salt) and `seal` / `open` (random 24-byte nonce ‖
  XChaCha20-Poly1305, 32-byte keys).
- Targets are dumb: no locking, no transactions, and not all kinds can rename
  (S3 cannot). The format must be safe under create-only writes and retries.
- The target must leak as little as possible: no titles, paths, or tags in
  filenames or plaintext headers.
- Rejected layouts: one big encrypted archive per device (no partial fetch, no
  concurrent appends, rewrite = re-upload everything); one mutable file per
  note (no history, no merge base, torn writes); server-side merge (targets
  must stay dumb — writan-server is an optional host, not a merge authority).
- M3 chunks built on this ADR: blob codec, merge engine, folder target +
  engine, keys/recovery, app integration. Later chunks must not need to change
  this layout.

## Decisions

1. **Target tree.**
   - `keys/<generation>.json` — the keyring (decisions 7–9);
   - `items/<note-uuid>/<device-uuid>/<counter>.enc` — one note version;
   - `tombstones/<note-uuid>/<device-uuid>/<counter>.enc` — one deletion of
     that note id (kept separate so a deleted note's subtree can be dropped
     wholesale by future GC without walking item history).
   UUIDs are lowercase hyphenated. `<counter>` is the writer's 1-based
   version-vector counter, decimal. Blobs are create-only and immutable: a
   name is written once and never rewritten.
2. **Blob body.** A `*.enc` file is
   `writan_crypto::seal(vault_key, aad, payload)`; the plaintext is compact
   JSON:
   - item: `{"format":"writan-note/1","id":…,"rel_path":…,"title":…,
     "tags":[…],"pinned":…,"created":…,"updated":…,"body":…,
     "vv":{"<device>":<counter>,…}}`
   - tombstone: `{"format":"writan-tombstone/1","id":…,"deleted_at":…,
     "vv":{…}}`
   `vv` is the writer's full version vector after its own bump, so the merge
   engine never guesses concurrency from timestamps alone. `rel_path` travels
   so a fresh device can materialise the file; it is a hint — identity is the
   `id` (ADR-0002 §3).
3. **AAD.** `aad = kind_byte ‖ uuid-16-bytes`, with `0x01` = item and `0x02` =
   tombstone. The note id is the core of the AAD, as the invariants require;
   the kind byte additionally blocks cross-kind substitution without relying
   on a post-decrypt format check, so no blob can be moved to another note or
   the other tree.
4. **Deletion is a version.** A tombstone joins the same version space as the
   item: a later item version with a dominating vector (a restore from trash,
   for example) undeletes it. A note reads as deleted when the merged vector
   of its tombstone versions dominates all of its item versions.
5. **Writer determinism.** A device seals each `(note, counter)` exactly once,
   caches the sealed bytes until the upload is confirmed (existence check where
   the target allows), and retries the same bytes otherwise. Since counters are
   per device, two devices can never collide on a name, and a crash-retry can
   never fork a name's content — random nonces stay safe without reuse.
6. **Local sync state lives in app-data** (`<state-dir>/sync/…`, ADR-0002
   §10), never in the vault: self device id, per-note last-uploaded counter,
   last-observed vectors, the merge base (blob ref + body hash), and the
   unconfirmed-seal cache. The target tree carries no manifest and no index: it
   can be read from blobs alone, and a device bootstraps by walking `items/`.
7. **Key hierarchy.** Vault key: random 32-byte symmetric key, created when
   sync is first enabled. Key-encryption keys: `KEK_password =
   argon2id(password, salt_password)` and `KEK_recovery = argon2id(recovery
   code, salt_recovery)`, same parameters as the crypto crate (m = 19 MiB,
   t = 2, p = 1; the derivation cost is paid once per unlock). The keyring
   holds the vault key sealed under each KEK, so either credential unlocks;
   password change and recovery regeneration each rewrite only their own wrap.
8. **Recovery code.** 32 random bytes, shown once as Crockford base32 grouped
   in fours (256 bits of entropy). It still goes through the same Argon2id
   derivation as the password — one code path, no special-casing. Export is
   mandatory before the first keyring is created: the engine refuses to enable
   sync until the user confirms it.
9. **Keyring file.** `keys/<generation>.json`, generation 1, 2, 3…, never
   rewritten in place:
   ```json
   { "format": "writan-keyring/1", "vault_id": "<uuid>", "generation": 1,
     "kdf": { "algo": "argon2id", "m_kib": 19456, "t": 2, "p": 1,
              "salt_password": "<b64>", "salt_recovery": "<b64>" },
     "wraps": [ { "purpose": "password", "blob": "<b64>" },
                { "purpose": "recovery", "blob": "<b64>" } ] }
   ```
   A wrap is `seal(kek, aad = "writan-keyring/1" ‖ 0x00 ‖ purpose ‖
   generation-be-u32, vault_key)`. Readers use the highest generation that
   parses and has a wrap their credential opens; older generations stay as a
   fallback if the newest is torn mid-upload (append-only, so a torn file can
   only be the newest). First device to enable sync defines the `vault_id` and
   vault key; a second device joins by unlocking the existing keyring — a
   target whose `vault_id` differs is refused, never merged.
10. **Unlock UX / keychain.** After a successful unlock the vault key may be
    stored in the OS keychain (Windows Credential Manager / Linux Secret
    Service; Android Keystore is M5) so later launches skip the prompt;
    deleting that entry locks sync again. The vault key is never written to
    `config.json` or any plaintext file.
11. **Target validity.** The sync target must not be the vault or sit inside
    it (and vice versa) — validated when the target is configured. The engine
    only creates `keys/`, `items/`, `tombstones/`; unknown files and
    directories are ignored and never deleted.
12. **Forward compatibility.** Unknown `format` strings and unknown top-level
    directories are left untouched by readers. New blob kinds arrive as new
    format strings and directories in a future ADR revision. Layout and
    counter semantics are part of the format: changing them bumps the major
    format string.
13. **Known limitation.** Blob sizes leak approximate note length and file
    mtimes leak edit activity — accepted for v1 (no padding, no compression).
    Filenames leak note existence, the device set, and per-device counts, and
    nothing else.

## Consequences

- The merge engine (next chunk) consumes plaintext payloads plus `vv` and
  produces either a merged body or an explicit conflict copy; merge policy is
  not encoded in the format.
- A complete note (frontmatter map + body) is always recoverable from its
  newest dominating item version, so a fresh device bootstraps from `items/`
  alone.
- Many small immutable files: proven at notes-app scale (Joplin) and fine for
  folder targets and S3-compatible stores; GC of old versions is out of v1.
- Keyring generations accumulate at a few hundred bytes each — negligible.
- Codec-chunk tests to land with the implementation: seal/open round-trip
  through the blob tree, AAD kind separation (item blob refused in the
  tombstones tree and vice versa), tombstone/undelete resolution by vectors,
  corrupt-newest keyring falls back to the previous generation, and a foreign
  `vault_id` is refused.
