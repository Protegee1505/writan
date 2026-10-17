# ADR-0006 — S3 sync target

Date: 2026-10-17
Status: accepted

## Context

M3's exit criteria require encrypted blob upload/download against *folder and
S3 targets*. The folder target (`FolderTarget`) proved the engine end-to-end
(ADR-0005), and the N-device simulator runs nightly against it. S3 is the second
target: a bucket is just another create-only store — blobs are sealed before
they leave the device, so the provider sees only opaque `.enc` objects.

Constraints carried over from ADR-0005: create-only writes, retries that must
succeed on identical bytes and refuse different bytes, no rename, no delete
(the sync protocol never removes objects), and no secrets on disk beyond what
the keychain holds.

## Decisions

1. **Plain S3 REST over `ureq`.** The target speaks the small subset it needs
   — `PUT`, `GET`, `LIST` (V2, paginated) — over `ureq` (blocking, rustls) and
   `quick-xml` for the listing envelope. No AWS SDK: the SDK is heavy, async
   first, and hides exactly the byte-level behaviour this target must expose.
2. **Hand-rolled SigV4, pinned by AWS's own test suite.** The signer is a pure
   function over (method, URI, query, signed headers, payload hash, timestamp,
   scope). Unit tests reproduce three vectors from the `aws-sig-v4-test-suite`
   (vanilla GET, sorted query keys, form-encoded POST body) with the example
   key pair, so correctness does not depend on a network service.
3. **Create-only via `If-None-Match: *`.** A `PUT` that hits an existing object
   returns 412; the target then `GET`s the object and compares bytes: identical
   → Ok (a safe retry), different → `TargetError::Diverged` — the same contract
   `FolderTarget` offers with `create_new`.
4. **Path-style addressing, configurable endpoint.** Works with MinIO (nightly
   CI service container) and with self-hosted stores; AWS itself also accepts
   path-style for this usage.
5. **Transport seam.** `S3Target<T: S3Transport>` keeps request building,
   signing and XML parsing separate from the HTTP call, so object-level logic
   (412-compare, listing pagination) is unit-tested with a canned transport and
   the live path is exercised by the nightly simulator against MinIO.
6. **Credentials.** Endpoint, region, bucket and access-key id belong in
   `config.json`; the secret access key never does — it lives in the OS
   keychain next to the vault key (same policy as ADR-0005 §10). The sync
   engine's keyring stays the only thing that protects note content.
7. **Nightly e2e.** A MinIO service container runs the N-device simulator over
   `S3Target` in `nightly.yml`; CI needs no cloud account.
8. **Dependencies.** `ureq` (MIT OR Apache-2.0), `hmac`/`sha2` (MIT OR
   Apache-2.0), `quick-xml` (MIT, already in the tree via Tauri). deny.toml
   gains `CDLA-Permissive-2.0` for the Mozilla CA-bundle data crate
   `webpki-roots` that rustls pulls in — data, not code.

## Consequences

- The sync engine gains no S3-specific branches: `S3Target` implements the same
  `SyncTarget` trait as `FolderTarget`.
- Listing a bucket per pass costs one `LIST` plus one `GET` per candidate blob;
  fine for note-vault sizes, revisitable if buckets grow.
- SigV4 is our code to keep right; the AWS test vectors plus the MinIO e2e run
  bound the risk.
- Session-token credentials (STS) are not supported in v1 — long-lived access
  keys and self-hosted stores are the supported shape.
