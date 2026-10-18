//! SQLite storage (ADR-0007 §8): one file, WAL, single connection behind a
//! mutex. The schema already carries the whole M4 surface — accounts,
//! devices, vaults, blobs, pairings — so the later chunks only add queries;
//! `meta.schema_version` guards migrations.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::auth::LockoutPolicy;
use crate::error::ApiError;

/// Bump with any migration; stored under `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    password_salt BLOB NOT NULL,
    password_hash BLOB NOT NULL,
    created_at    INTEGER NOT NULL,
    failed_logins INTEGER NOT NULL DEFAULT 0,
    locked_until  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS devices (
    account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id           TEXT NOT NULL,
    name         TEXT NOT NULL,
    token_hash   BLOB NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (account_id, id)
);

CREATE TABLE IF NOT EXISTS vaults (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    next_seq   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS vaults_by_account ON vaults(account_id);

CREATE TABLE IF NOT EXISTS blobs (
    vault_id   TEXT NOT NULL REFERENCES vaults(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    bytes      BLOB NOT NULL,
    size       INTEGER NOT NULL,
    seq        INTEGER NOT NULL,
    device_id  TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, path)
);
CREATE INDEX IF NOT EXISTS blobs_by_seq ON blobs(vault_id, seq);

CREATE TABLE IF NOT EXISTS pairings (
    code_hash  BLOB PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER
);
";

/// An account row (no password material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub created_at: i64,
    pub failed_logins: i64,
    pub locked_until: i64,
}

/// Password material for one account.
#[derive(Debug, Clone)]
pub struct AccountAuth {
    pub account: Account,
    pub salt: [u8; 16],
    pub hash: [u8; 32],
}

/// A device row (no token material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// A vault row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    pub id: Uuid,
    pub created_at: i64,
}

/// Open (creating if needed) the database and apply the schema.
pub fn open(path: &Path) -> Result<Connection, ApiError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        None => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }
        Some(raw) => {
            let version: i64 = raw.parse().map_err(|_| {
                ApiError::Internal("meta.schema_version is not a number".to_string())
            })?;
            if version > SCHEMA_VERSION {
                return Err(ApiError::Internal(format!(
                    "database schema v{version} is newer than this server (v{SCHEMA_VERSION})"
                )));
            }
            if version < SCHEMA_VERSION {
                return Err(ApiError::Internal(format!(
                    "database schema v{version} is older than this server (v{SCHEMA_VERSION}); \
                     no migration path yet"
                )));
            }
        }
    }
    Ok(conn)
}

/// Insert an account; a second account for the same email is
/// [`ApiError::EmailTaken`]. The caller normalizes the email first.
pub fn create_account(
    conn: &Connection,
    email: &str,
    salt: &[u8; 16],
    hash: &[u8; 32],
    now: i64,
) -> Result<Account, ApiError> {
    let id = Uuid::new_v4();
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO accounts (id, email, password_salt, password_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.to_string(), email, salt.as_slice(), hash.as_slice(), now],
    )?;
    if inserted == 0 {
        return Err(ApiError::EmailTaken);
    }
    Ok(Account {
        id,
        email: email.to_string(),
        created_at: now,
        failed_logins: 0,
        locked_until: 0,
    })
}

/// Look an account up by its normalized email, with password material.
pub fn account_by_email(conn: &Connection, email: &str) -> Result<Option<AccountAuth>, ApiError> {
    let found = conn
        .query_row(
            "SELECT id, email, created_at, failed_logins, locked_until, password_salt, password_hash
             FROM accounts WHERE email = ?1",
            params![email],
            |row| {
                let account = Account {
                    id: uuid_from_row(row, 0)?,
                    email: row.get(1)?,
                    created_at: row.get(2)?,
                    failed_logins: row.get(3)?,
                    locked_until: row.get(4)?,
                };
                let salt: Vec<u8> = row.get(5)?;
                let hash: Vec<u8> = row.get(6)?;
                Ok((account, salt, hash))
            },
        )
        .optional()?;
    found
        .map(|(account, salt, hash)| {
            Ok(AccountAuth {
                account,
                salt: take16(salt)?,
                hash: take32(hash)?,
            })
        })
        .transpose()
}

/// Look an account up by id (no password material).
pub fn account_by_id(conn: &Connection, account_id: Uuid) -> Result<Option<Account>, ApiError> {
    conn.query_row(
        "SELECT id, email, created_at, failed_logins, locked_until FROM accounts WHERE id = ?1",
        params![account_id.to_string()],
        account_from_row,
    )
    .optional()
    .map_err(Into::into)
}

/// Replace the password hash, reset the lockout counters and revoke every
/// device of the account. Returns `false` when there is no such account.
pub fn reset_password(
    conn: &mut Connection,
    account_id: Uuid,
    salt: &[u8; 16],
    hash: &[u8; 32],
) -> Result<bool, ApiError> {
    let tx = conn.transaction()?;
    let updated = tx.execute(
        "UPDATE accounts
         SET password_salt = ?1, password_hash = ?2, failed_logins = 0, locked_until = 0
         WHERE id = ?3",
        params![salt.as_slice(), hash.as_slice(), account_id.to_string()],
    )?;
    if updated == 0 {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM devices WHERE account_id = ?1",
        params![account_id.to_string()],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Record a failed login and return the `locked_until` value that was
/// stored (0 while not locked).
pub fn record_login_failure(
    conn: &Connection,
    account_id: Uuid,
    policy: &LockoutPolicy,
    now: i64,
) -> Result<i64, ApiError> {
    let before: i64 = conn.query_row(
        "SELECT failed_logins FROM accounts WHERE id = ?1",
        params![account_id.to_string()],
        |row| row.get(0),
    )?;
    let failed_logins = before + 1;
    let locked_until = policy.until(failed_logins, now);
    conn.execute(
        "UPDATE accounts SET failed_logins = ?1, locked_until = ?2 WHERE id = ?3",
        params![failed_logins, locked_until, account_id.to_string()],
    )?;
    Ok(locked_until)
}

/// Forgive the failed-login counters after a successful login.
pub fn clear_login_failures(conn: &Connection, account_id: Uuid) -> Result<(), ApiError> {
    conn.execute(
        "UPDATE accounts SET failed_logins = 0, locked_until = 0 WHERE id = ?1",
        params![account_id.to_string()],
    )?;
    Ok(())
}

/// Register a device or replace its token: the same device id under the same
/// account keeps `created_at` but gets the new name, token and `last_seen_at`.
pub fn upsert_device(
    conn: &Connection,
    account_id: Uuid,
    device_id: Uuid,
    name: &str,
    token_hash: &[u8; 32],
    now: i64,
) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO devices (account_id, id, name, token_hash, created_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT (account_id, id) DO UPDATE SET
             name = excluded.name,
             token_hash = excluded.token_hash,
             last_seen_at = excluded.last_seen_at",
        params![
            account_id.to_string(),
            device_id.to_string(),
            name,
            token_hash.as_slice(),
            now
        ],
    )?;
    Ok(())
}

/// Resolve a bearer token hash to its account and device.
pub fn device_by_token(
    conn: &Connection,
    token_hash: &[u8; 32],
) -> Result<Option<(Account, Device)>, ApiError> {
    conn.query_row(
        "SELECT a.id, a.email, a.created_at, a.failed_logins, a.locked_until,
                d.id, d.name, d.created_at, d.last_seen_at
         FROM devices d
         JOIN accounts a ON a.id = d.account_id
         WHERE d.token_hash = ?1",
        params![token_hash.as_slice()],
        |row| {
            let account = Account {
                id: uuid_from_row(row, 0)?,
                email: row.get(1)?,
                created_at: row.get(2)?,
                failed_logins: row.get(3)?,
                locked_until: row.get(4)?,
            };
            let device = Device {
                id: uuid_from_row(row, 5)?,
                name: row.get(6)?,
                created_at: row.get(7)?,
                last_seen_at: row.get(8)?,
            };
            Ok((account, device))
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Mark a device as seen.
pub fn touch_device(
    conn: &Connection,
    account_id: Uuid,
    device_id: Uuid,
    now: i64,
) -> Result<(), ApiError> {
    conn.execute(
        "UPDATE devices SET last_seen_at = ?1 WHERE account_id = ?2 AND id = ?3",
        params![now, account_id.to_string(), device_id.to_string()],
    )?;
    Ok(())
}

/// Every device of an account, oldest first.
pub fn list_devices(conn: &Connection, account_id: Uuid) -> Result<Vec<Device>, ApiError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at, last_seen_at FROM devices
         WHERE account_id = ?1 ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![account_id.to_string()], |row| {
        Ok(Device {
            id: uuid_from_row(row, 0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            last_seen_at: row.get(3)?,
        })
    })?;
    let mut devices = Vec::new();
    for row in rows {
        devices.push(row?);
    }
    Ok(devices)
}

/// Delete one device; `false` when it did not exist.
pub fn delete_device(
    conn: &Connection,
    account_id: Uuid,
    device_id: Uuid,
) -> Result<bool, ApiError> {
    let deleted = conn.execute(
        "DELETE FROM devices WHERE account_id = ?1 AND id = ?2",
        params![account_id.to_string(), device_id.to_string()],
    )?;
    Ok(deleted > 0)
}

/// Create a vault owned by the account; `false` when the id already exists
/// (the caller then checks the owner, ADR-0007 §5).
pub fn create_vault(
    conn: &Connection,
    account_id: Uuid,
    vault_id: Uuid,
    now: i64,
) -> Result<bool, ApiError> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO vaults (id, account_id, created_at, next_seq) VALUES (?1, ?2, ?3, 0)",
        params![vault_id.to_string(), account_id.to_string(), now],
    )?;
    Ok(inserted > 0)
}

/// The account that owns a vault, if the vault exists at all.
pub fn owner_of_vault(conn: &Connection, vault_id: Uuid) -> Result<Option<Uuid>, ApiError> {
    let owner: Option<String> = conn
        .query_row(
            "SELECT account_id FROM vaults WHERE id = ?1",
            params![vault_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    owner
        .map(|raw| {
            Uuid::parse_str(&raw)
                .map_err(|_| ApiError::Internal("vault owner id is not a uuid".to_string()))
        })
        .transpose()
}

/// Every vault of an account, oldest first.
pub fn vaults_for(conn: &Connection, account_id: Uuid) -> Result<Vec<Vault>, ApiError> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at FROM vaults WHERE account_id = ?1 ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![account_id.to_string()], |row| {
        Ok(Vault {
            id: uuid_from_row(row, 0)?,
            created_at: row.get(1)?,
        })
    })?;
    let mut vaults = Vec::new();
    for row in rows {
        vaults.push(row?);
    }
    Ok(vaults)
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: uuid_from_row(row, 0)?,
        email: row.get(1)?,
        created_at: row.get(2)?,
        failed_logins: row.get(3)?,
        locked_until: row.get(4)?,
    })
}

fn uuid_from_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let raw: String = row.get(index)?;
    Uuid::parse_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn take16(bytes: Vec<u8>) -> Result<[u8; 16], ApiError> {
    bytes
        .try_into()
        .map_err(|_| ApiError::Internal("stored salt has the wrong length".to_string()))
}

fn take32(bytes: Vec<u8>) -> Result<[u8; 32], ApiError> {
    bytes
        .try_into()
        .map_err(|_| ApiError::Internal("stored hash has the wrong length".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::memory_db;

    fn policy() -> LockoutPolicy {
        LockoutPolicy {
            after: 2,
            base_secs: 10,
            cap_secs: 100,
        }
    }

    const SALT: [u8; 16] = [7; 16];
    const HASH: [u8; 32] = [9; 32];

    #[test]
    fn schema_is_recorded_and_reopens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("writan-server.sqlite");
        let conn = open(&path).expect("open");
        drop(conn);
        let conn = open(&path).expect("reopen");
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("schema_version");
        assert_eq!(version, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn account_roundtrip_and_duplicate() {
        let conn = memory_db();
        let account = create_account(&conn, "ada@example.com", &SALT, &HASH, 100).expect("create");
        let auth = account_by_email(&conn, "ada@example.com")
            .expect("lookup")
            .expect("found");
        assert_eq!(auth.account, account);
        assert_eq!(auth.salt, SALT);
        assert_eq!(auth.hash, HASH);
        assert!(account_by_id(&conn, account.id).expect("by id").is_some());
        let duplicate = create_account(&conn, "ada@example.com", &SALT, &HASH, 200)
            .expect_err("duplicate refused");
        assert!(matches!(duplicate, ApiError::EmailTaken));
    }

    #[test]
    fn failures_lock_and_clear() {
        let conn = memory_db();
        let account = create_account(&conn, "ada@example.com", &SALT, &HASH, 100).expect("create");
        assert_eq!(
            record_login_failure(&conn, account.id, &policy(), 1_000).expect("first"),
            0
        );
        assert_eq!(
            record_login_failure(&conn, account.id, &policy(), 1_000).expect("second"),
            1_010
        );
        clear_login_failures(&conn, account.id).expect("clear");
        let stored = account_by_id(&conn, account.id)
            .expect("lookup")
            .expect("found");
        assert_eq!(stored.failed_logins, 0);
        assert_eq!(stored.locked_until, 0);
    }

    #[test]
    fn reset_password_swaps_material_and_revokes_devices() {
        let mut conn = memory_db();
        let account = create_account(&conn, "ada@example.com", &SALT, &HASH, 100).expect("create");
        upsert_device(&conn, account.id, Uuid::new_v4(), "laptop", &[1; 32], 100).expect("device");
        let new_salt = [8; 16];
        let new_hash = [10; 32];
        assert!(reset_password(&mut conn, account.id, &new_salt, &new_hash).expect("reset"));
        let auth = account_by_email(&conn, "ada@example.com")
            .expect("lookup")
            .expect("found");
        assert_eq!(auth.salt, new_salt);
        assert_eq!(auth.hash, new_hash);
        assert!(list_devices(&conn, account.id).expect("devices").is_empty());
        assert!(
            !reset_password(&mut conn, Uuid::new_v4(), &new_salt, &new_hash).expect("missing"),
            "unknown account is reported"
        );
    }

    #[test]
    fn devices_upsert_replaces_the_token() {
        let conn = memory_db();
        let account = create_account(&conn, "ada@example.com", &SALT, &HASH, 100).expect("create");
        let device_id = Uuid::new_v4();
        upsert_device(&conn, account.id, device_id, "laptop", &[1; 32], 100).expect("first");
        assert!(
            device_by_token(&conn, &[1; 32])
                .expect("by token")
                .is_some()
        );
        upsert_device(&conn, account.id, device_id, "laptop", &[2; 32], 200).expect("second");
        assert!(
            device_by_token(&conn, &[1; 32])
                .expect("old token")
                .is_none()
        );
        let (_, device) = device_by_token(&conn, &[2; 32])
            .expect("by token")
            .expect("found");
        assert_eq!(device.created_at, 100, "created_at survives a re-register");
        assert_eq!(device.last_seen_at, 200);
        assert_eq!(list_devices(&conn, account.id).expect("devices").len(), 1);
        assert!(delete_device(&conn, account.id, device_id).expect("delete"));
        assert!(!delete_device(&conn, account.id, device_id).expect("again"));
    }

    #[test]
    fn vaults_are_owned_once() {
        let conn = memory_db();
        let ada = create_account(&conn, "ada@example.com", &SALT, &HASH, 100).expect("create");
        let bob = create_account(&conn, "bob@example.com", &SALT, &HASH, 100).expect("create");
        let vault_id = Uuid::new_v4();
        assert!(create_vault(&conn, ada.id, vault_id, 100).expect("create"));
        assert!(!create_vault(&conn, bob.id, vault_id, 200).expect("collision"));
        assert_eq!(
            owner_of_vault(&conn, vault_id).expect("owner"),
            Some(ada.id)
        );
        assert_eq!(vaults_for(&conn, ada.id).expect("list").len(), 1);
        assert!(vaults_for(&conn, bob.id).expect("list").is_empty());
        assert_eq!(
            owner_of_vault(&conn, Uuid::new_v4()).expect("missing"),
            None
        );
    }
}
