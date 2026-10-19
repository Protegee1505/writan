# ADR-0007: noteserver — protocol, authentication, storage

Status: accepted. Date: 2026-10-18.

> Note (2026-10-18): decision 2 (accounts and passwords) is superseded by
> [ADR-0008](0008-one-password-signin.md) — one password, derived client-side
> into the KEK and a server verifier; that ADR is also the recorded exception
> to decision 12's new-prefix rule.

## Context

- M4's scope (PLAN.md): axum + SQLite server with email+password and QR
  pairing, a blob API and SSE change events; Docker image; deploy doc for
  free tiers. The server is optional — folder (ADR-0005) and S3 (ADR-0006)
  stay first-class targets, and writan-server is a third target of the same
  kind, not a new sync model.
- ADR-0005 fixed the blob tree, create-only writes and "targets are dumb":
  writan-server is a host, not a merge authority, and never sees plaintext
  or keys. ADR-0005 §§6–10 (keyring, recovery codes, keychain) stay exactly
  as decided; the server adds *access* control on top, not crypto.
- An account password cannot double as the E2EE password: the sync password
  derives the KEK locally (ADR-0005 §7) and must never leave the device,
  while a server credential authenticates HTTP requests. Two secrets, two
  purposes — the UI must word them apart.
- M0 left `crates/writan-server` as a `/health` skeleton; ADR-0005 §"Context"
  already listed writan-server as an optional host.

## Decisions

1. **Role and trust.** The server is a dumb ciphertext store plus a change
   feed. It persists the ADR-0005 blob tree, accounts, device tokens and
   pairing codes — never plaintext, titles, keys or the E2EE password, and
   it never merges. No web UI (v1 scope excludes a web app), no telemetry.
   Blobs are immutable: the server never rewrites or deletes one, and a
   path outside the engine's own conventions is stored as opaque bytes
   (same forward-compatibility rule as the folder target, ADR-0005 §12).

2. **Accounts and passwords (server-side auth ≠ E2EE).** An account is an
   email (the login id, trimmed and lowercased, unverified) plus a password
   hashed with Argon2id m = 19 MiB, t = 2, p = 1 — the same
   `writan-crypto::derive_kek` (16-byte salt) as ADR-0005 §7, so the
   workspace keeps one KDF. Password policy: 8–1024 bytes. No SMTP in v1:
   no verification mail, no reset mail. Failed logins count per account
   (5 consecutive → 60 s lockout, doubling, capped at 15 min; reset on
   success); a login for an unknown email still pays one derivation, so
   timing does not reveal account existence. `WRITAN_REGISTRATION=closed`
   refuses new accounts. Password reset is the admin CLI
   `writan-server reset-password <email>`, which also revokes every device
   token of that account (devices sign in again).

3. **Device tokens.** Every device authenticates with
   `Authorization: Bearer <token>`; the token is 32 random bytes in
   base64url (43 characters), shown exactly once and stored only as its
   SHA-256 (high-entropy secret: no KDF needed at rest). The device id is
   the client's sync `DeviceId` (uuid), supplied at registration; a device
   re-registering under the same account replaces its token and the old one
   dies. `last_seen_at` is written at most every 5 minutes.
   `DELETE /api/v1/sessions` deletes the calling device row (logout).

4. **Pairing and QR.** An authenticated device mints a pairing code
   (`POST /api/v1/pairings`): 32 random bytes, single use, TTL 600 s
   (`WRITAN_PAIRING_TTL_SECS`), stored hashed. Redeeming it
   (`POST /api/v1/pairings/redeem`) registers the new device and returns
   its token. The QR payload is compact JSON
   `{"format":"writan-pair/1","url":…,"code":…,"email":…}`; desktop shells
   render it (M4), Android scans it (M5). Pairing grants **blob access
   only** — the vault key never travels; the joining device still unlocks
   the keyring with the sync password or recovery code (ADR-0005 §7–9).
   That is deliberate and must stay visible in the UI copy.

5. **Vaults.** A vault is the keyring `vault_id` (uuid), owned by exactly
   one account in v1; cross-account sharing is out of scope. The owning
   device creates it explicitly with `PUT /api/v1/vaults/{id}` (idempotent:
   201 created, 200 already owned). `GET /api/v1/vaults` lists ids and
   creation times. Every blob or change request for an unknown or foreign
   vault is 404 — accounts cannot probe each other's vault ids.

6. **Blob API.** Paths are validated as safe relative segments (no empty,
   `.` or `..` segment, no leading slash, ≤ 512 bytes); anything else
   within that rule is accepted. `GET …/blobs` lists paths;
   `GET …/blobs/{*path}` reads; `PUT …/blobs/{*path}` is create-only with
   exactly the `SyncTarget::write` semantics (ADR-0005 dfn: identical bytes
   → 204 for a retried write, different bytes → 409). This makes the client
   target (`writan-sync::server`) a direct `SyncTarget` implementation.

7. **Change feed.** Each vault keeps a monotonic sequence; every created
   blob takes `seq = ++vault.next_seq` in the same transaction as its
   insert, so the feed is append-only (create-only blobs; no delete
   events — GC is out of v1, ADR-0005 §"Consequences").
   `GET …/changes?since={seq}` returns
   `{events:[{seq,path,deviceId,size,at}],next}`.
   `GET …/events` streams the same events as SSE with `id:` = seq and
   `Last-Event-ID` resume. Events are advisory: correctness never depends
   on them — a client that misses one catches up via `/changes` or a
   listing.

8. **Storage.** One SQLite file (`WRITAN_DATA_DIR/writan-server.sqlite`),
   WAL, `foreign_keys=ON`, `busy_timeout=5s`, `synchronous=NORMAL`, schema
   version in `meta`. Blobs are rows (BLOB column) — note-scale payloads,
   one file to back up (`sqlite3 .backup` copied hot is safe with WAL).
   v1 uses a single connection behind a mutex; a pool is a later change
   that needs no format change.

9. **Limits.** Defaults, all environment-tunable:
   `WRITAN_MAX_BLOB_KIB=4096` (413 for larger bodies),
   `WRITAN_VAULT_QUOTA_MIB=512`, `0` = unlimited (507 once a write would
   exceed it). Request bodies are capped at the blob limit plus slack.

10. **Ops.** Everything is environment configuration (`WRITAN_PORT=8787`,
    `WRITAN_DATA_DIR`, `WRITAN_REGISTRATION`, the limits above,
    `WRITAN_PAIRING_TTL_SECS`). `GET /health` answers
    `{status,version,schema}` for monitors. The server speaks plain HTTP:
    TLS terminates at a reverse proxy (docs/SERVER.md carries a Caddy
    example and the free-tier notes, including Oracle Always Free's idle
    reclaim — run companions or switch to PAYG). Releases ship a
    multi-stage Dockerfile; CI builds the image on every relevant change
    and pushes to GHCR on `v*` tags and manual dispatch.

11. **Client integration.** `writan-sync::server` implements
    `SyncTarget` over HTTP (`ServerTransport` trait + a `ureq` impl, the
    same shape as ADR-0006's S3 target). `SyncKind` gains `server`;
    `config.json` keeps `{url,email}` (never the token) and the token lives
    in the OS keychain (`com.writan.desktop.server`, account
    `<url>/<email>`). Both shells offer the same connect/join flows
    (parity rule). v1 clients poll `/changes?since=`; consuming SSE is an
    optional follow-up.

12. **API stability.** Endpoints live under `/api/v1`; DTO fields are
    camelCase (the repo's DTO convention); clients ignore unknown response
    fields and the server ignores unknown request fields. A breaking change
    means a new prefix. Timestamps are RFC 3339.

## Consequences

- The server learns: account email, vault and device ids, blob paths
  (`items/<uuid>/<uuid>/<n>.enc` — existence and counts, no titles), sizes,
  timing. That is the accepted leak set of ADR-0005 §13 plus the email.
- SQLite keeps deployment to one file and one binary; attachments sync is
  not v1, so payloads stay note-scale. Revisit row-vs-file blob storage
  only if that changes.
- One account per vault: "share a vault with another person" is
  unsupported in v1. Adding it later is a members table plus roles —
  additive, no blob or wire format change.
- No GC plus a quota means a vault fills monotonically until the user
  raises the quota or prunes history (v2 GC).
- Server tests run inside the existing workspace test job (axum `oneshot`
  against an in-memory or temp-file SQLite); no extra CI job until the
  Docker build (chunk G).
- An implementation-order note for M4: chunk A accounts/sessions/storage,
  B blob API + vaults, C changes/SSE, E client target (engine round-trip
  against the real server), D pairing, H QR display, F shell wiring,
  G Docker + deploy doc + CI.
