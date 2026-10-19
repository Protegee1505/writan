# ADR-0008: One-password sign-in (verifier auth, password-derived vault key)

Status: accepted. Date: 2026-10-18.

## Context

- The M4.5 goal: a fresh machine should need only address + email + password
  to get all notes — "log in and they are there". Today a server sign-in takes
  *two* secrets: the account password (sent to the server for login, hashed
  server-side) and the sync password (derives the KEK locally, never leaves
  the device; ADR-0007 §2, ADR-0005 §7). The split is real — the account
  password cannot be reused as the key password *while the server sees it*,
  because the server holds the public keyring salt and could derive the KEK —
  but the user experience is two fields that mean nearly the same thing.
- The fix: derive two values from one password P with two different salts —
  `KEK = Argon2id(P, salt_keyring)` (unchanged, ADR-0005 §7) and
  `verifier = Argon2id(P, salt_auth)` — and send only the verifier. The
  server never sees P, so it cannot derive the KEK; the two-secret problem
  disappears without weakening the dumb-store promise.
- Pre-1.0: there are no third-party accounts to migrate, so the auth API can
  change under `/api/v1` — an explicit exception to ADR-0007 §12's "a breaking
  change means a new prefix", recorded here because §12 is otherwise binding
  and because server and clients must ship together.
- One-password makes "Change password" and "New recovery code" necessary
  (there is no way to rotate either secret today) and makes the recovery code
  the only backup for a forgotten password.

## Decisions

1. **One password, two derivations.** The user's password P (≥ 8 bytes after
   trimming nothing; ≤ 1024 bytes) is used client-side for both purposes:
   - `KEK = Argon2id(P, salt_keyring)` wraps the vault key in the keyring
     (ADR-0005 §7–9, unchanged);
   - `verifier = Argon2id(P, salt_auth)` authenticates to the server.
   The server stores the account's `salt_auth` (16 random bytes) and
   `SHA-256(verifier)` — the verifier is already high-entropy, so a single
   SHA-256 is enough at rest (the ADR-0007 §3 device-token rule). It never
   receives P or the KEK and runs no KDF on login. The password-length policy
   is enforced client-side (a hostile client is its own problem on a
   self-hosted server) and by the admin CLI.

2. **Wire shape** (base64 Standard for `salt`/`verifier`, camelCase DTOs):
   ```
   GET  /api/v1/accounts/salt?email=…   -> {salt}
   POST /api/v1/accounts                 {email, salt, verifier, deviceId, deviceName}
   POST /api/v1/sessions                 {email, verifier, deviceId, deviceName}
   POST /api/v1/account/verifier         {oldVerifier, salt, verifier}   (Bearer)
   ```
   Registration picks `salt_auth` client-side (one round trip less). Login
   fetches the salt first, derives the verifier, sends it. `salt` is 16 bytes,
   `verifier` 32 bytes; anything else is 400. A wrong verifier answers the
   same 401 as an unknown email, counts toward the ADR-0007 §2 lockout, and
   the lockout shapes are unchanged (5 → 60 s doubling, capped at 15 min).

3. **Unknown emails stay hidden.** `GET /accounts/salt` answers a dummy salt
   for an email with no account: `HMAC-SHA256(secret, normalized_email)`
   truncated to 16 bytes, with `secret` 32 random bytes generated on first use
   and kept in `meta`. Clients cannot tell dummy from real, and login answers
   the same 401 either way; registration still reveals a taken email (as in
   M4, unchanged).

4. **Schema v2, and legacy accounts.** The `accounts` table drops
   `password_salt`/`password_hash` and gains nullable `auth_salt` and
   `verifier_hash`; `meta.schema_version` goes 1 → 2 and `db::open` performs
   the migration (rename-free: `ALTER TABLE … ADD COLUMN` +
   `ALTER TABLE … DROP COLUMN`; the bundled SQLite is ≥ 3.35). An account with
   `auth_salt IS NULL` is unmigrated: login answers 403 with
   "this account predates this server version; the server owner must run
   `writan-server reset-password EMAIL`", and no other endpoint trusts it
   (vault/device rows still work through tokens). `reset-password` now mints
   `salt_auth` + verifier hash from the new password at the CLI and revokes
   every device, so it doubles as the one-time migration step. Fresh servers
   need nothing.

5. **Joining a keyring created under another password.** The join flow
   (`connect_server`, `join_with_code`) takes the one password P, signs in
   with it, then unlocks the keyring. When the keyring was created earlier
   with a different sync password (a v0.1.0 vault), the direct unlock fails
   and the caller may pass one *fallback credential* (the old sync password or
   the recovery code). If it opens the keyring, a new generation is appended
   whose password wrap uses P — the vault key is unchanged — and sync
   proceeds; the vault thereby "adopts" the new password. No fallback, or a
   fallback that does not open either, is the existing join error with UI
   wording that points at the old credential prompt.

6. **Change password** (settings, both shells) = one action, keyring first,
   account second — so a failure never locks the user out:
   1. the old password must open the keyring (proves it and yields the vault
      key);
   2. a new generation is appended carrying only a password wrap for the new
      P (fresh `salt_password`; the unused recovery salt is random bytes —
      the file shape of ADR-0005 §9 is unchanged);
   3. for server targets, `POST /api/v1/account/verifier` replaces
      `salt_auth`/verifier hash after checking `oldVerifier`, and revokes
      every other device of the account (the caller keeps its token).
   Ordering means an interrupted change leaves extra keyring generations but
   never a keyring the new password cannot open. Rotation does not rewrite
   older generations (targets are append-only), so an old password can still
   open older keyring generations offline; the account verifier, however, is
   replaced immediately. Fully revoking old keyring wrappers needs a vault-key
   rollover and is out of v1 — recorded as a known limitation.

7. **New recovery code** (settings): appends a generation carrying only a
   recovery wrap for a fresh code, generated on an unlocked device (no
   password needed; the vault key is in memory). The new code is shown once,
   as always; the old code still opens older generations the same way an old
   password does (limitation above). The first keyring still requires a
   recovery-code export (ADR-0005 §8).

8. **Shell UX.** One "Password" field in the connect/join dialogs; the
   account-password field is gone. Copy says the password unlocks notes on a
   new device and signs the account in — never both secrets at once again.
   The connected panel gains "Change password" and "New recovery code" rows.
   Pairing copy is unchanged: the QR still carries access only, and the
   joining device still types the password (or uses a recovery code).

9. **Build-time default address.** Both shells read an optional
   `WRITAN_DEFAULT_SERVER` at compile time to prefill the server address
   field (personal builds; release builds leave it empty). It is a
   convenience only — no baked-in credentials.

## Consequences

- Old (v0.1.0) clients cannot authenticate against a v2 server, and old
  servers cannot serve v2 clients: server and clients ship together in
  v0.2.0. The v0.1.0 release remains the pair for its own server image.
- The server no longer runs Argon2id per login (the cost moved entirely
  client-side): logins are cheaper and the DoS surface shrinks.
- A forgotten password is recoverable only with the recovery code — or, on a
  self-hosted server, by `reset-password`, which rotates the *account*
  credential but does **not** recover notes; only the keyring's recovery wrap
  does. The UI and the CLI must keep saying so.
- An attacker with the verifier hash and a weak password can brute-force P
  offline (Argon2 per guess) — the same exposure the keyring already has; a
  strong password and the recovery code stay the anchors.
- `docs/SERVER.md`, the CLI usage text, the pairing copy and the alternative
  guidance about "account password vs sync password" are superseded wherever
  they describe two secrets.
- M4.5 chunks: (A) ADR + server v2 + CLI; (B) writan-sync client + keyring
  appends; (C) both shells; (D) server binary in releases + install script;
  (E) docs + v0.2.0.
