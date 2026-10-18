//! The HTTP surface (ADR-0007 §§2–7). Chunks A–B: health, accounts,
//! sessions, vaults, the blob API and the change feed.
//!
//! Handlers take the single writer connection behind a mutex — cheap at one
//! user per instance (ADR-0007 §8); a connection pool is a later change with
//! no format or API impact.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth;
use crate::config::ServerConfig;
use crate::db::{self, Account, Device};
use crate::error::ApiError;
use crate::util::{now_unix, rfc3339, validate_blob_path};

/// Longest stored device name (characters).
const DEVICE_NAME_MAX_CHARS: usize = 64;

/// Change-feed page size when the request does not ask (ADR-0007 §7).
const CHANGES_LIMIT_DEFAULT: u32 = 500;

/// Largest page a client may request.
const CHANGES_LIMIT_MAX: u32 = 2_000;

/// Slack above the blob limit so an oversized body still gets this server's
/// JSON error; anything larger is cut off by the extractor's own limit
/// (ADR-0007 §9).
const BODY_LIMIT_SLACK: usize = 64 * 1024;

/// Everything the handlers need.
#[derive(Clone)]
pub struct AppState {
    /// The one writer connection, shared across handler tasks.
    pub db: Arc<Mutex<Connection>>,
    /// Startup configuration (limits, registration switch, lockout).
    pub config: Arc<ServerConfig>,
}

impl AppState {
    /// Wrap an open connection.
    pub fn new(conn: Connection, config: ServerConfig) -> Self {
        Self {
            db: Arc::new(Mutex::new(conn)),
            config: Arc::new(config),
        }
    }

    /// The shared connection; a panicked handler cannot poison the server.
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The router (ADR-0007 §12: DTOs are camelCase, everything under
/// `/api/v1`).
pub fn app(state: AppState) -> Router {
    let body_limit = state.config.max_blob_bytes.saturating_add(BODY_LIMIT_SLACK);
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/accounts", post(create_account))
        .route("/api/v1/sessions", post(login).delete(logout))
        .route("/api/v1/account", get(account_info))
        .route("/api/v1/vaults", get(list_vaults))
        .route("/api/v1/vaults/{vault_id}", put(create_vault))
        .route("/api/v1/vaults/{vault_id}/blobs", get(list_blobs))
        .route(
            "/api/v1/vaults/{vault_id}/blobs/{*path}",
            get(read_blob).put(write_blob),
        )
        .route("/api/v1/vaults/{vault_id}/changes", get(changes))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    schema: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    email: String,
    password: String,
    device_id: Uuid,
    device_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    account_id: Uuid,
    email: String,
    device_id: Uuid,
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    id: Uuid,
    name: String,
    created_at: String,
    last_seen_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    account_id: Uuid,
    email: String,
    created_at: String,
    devices: Vec<DeviceInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultInfo {
    id: Uuid,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct VaultList {
    vaults: Vec<VaultInfo>,
}

#[derive(Debug, Serialize)]
struct BlobList {
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    since: Option<i64>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeEvent {
    seq: i64,
    path: String,
    device_id: Uuid,
    size: i64,
    at: String,
}

#[derive(Debug, Serialize)]
struct ChangePage {
    events: Vec<ChangeEvent>,
    next: i64,
}

/// `GET /health` — monitoring shape (ADR-0007 §10).
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        schema: db::SCHEMA_VERSION,
    })
}

/// `POST /api/v1/accounts` — register and sign the device in.
async fn create_account(
    State(state): State<AppState>,
    Json(request): Json<Credentials>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    if !state.config.registration_open {
        return Err(ApiError::RegistrationClosed);
    }
    let email = auth::normalize_email(&request.email)?;
    auth::validate_password(&request.password)?;
    let device_name = clean_device_name(&request.device_name);
    let (salt, hash) = auth::hash_password(&request.password)?;
    let now = now_unix();
    let conn = state.conn();
    let account = db::create_account(&conn, &email, &salt, &hash, now)?;
    let token = auth::generate_token();
    db::upsert_device(
        &conn,
        account.id,
        request.device_id,
        &device_name,
        &auth::hash_token(&token),
        now,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(session(
            &account.email,
            account.id,
            request.device_id,
            token,
        )),
    ))
}

/// `POST /api/v1/sessions` — email + password, answered with a fresh device
/// token (the sign-in of both shells).
async fn login(
    State(state): State<AppState>,
    Json(request): Json<Credentials>,
) -> Result<Json<Session>, ApiError> {
    let email = auth::normalize_email(&request.email)?;
    let device_name = clean_device_name(&request.device_name);
    let now = now_unix();
    let conn = state.conn();
    let Some(material) = db::account_by_email(&conn, &email)? else {
        auth::verify_dummy(&request.password);
        return Err(ApiError::Unauthorized);
    };
    if material.account.locked_until > now {
        return Err(ApiError::Locked);
    }
    if !auth::verify_password(&request.password, &material.salt, &material.hash) {
        let locked_until =
            db::record_login_failure(&conn, material.account.id, &state.config.lockout, now)?;
        return Err(if locked_until > now {
            ApiError::Locked
        } else {
            ApiError::Unauthorized
        });
    }
    db::clear_login_failures(&conn, material.account.id)?;
    let token = auth::generate_token();
    db::upsert_device(
        &conn,
        material.account.id,
        request.device_id,
        &device_name,
        &auth::hash_token(&token),
        now,
    )?;
    Ok(Json(session(
        &material.account.email,
        material.account.id,
        request.device_id,
        token,
    )))
}

/// `DELETE /api/v1/sessions` — forget the calling device (logout).
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let conn = state.conn();
    let (account, device) = require_auth(&conn, &headers)?;
    db::delete_device(&conn, account.id, device.id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/account` — who the token belongs to, and its sibling
/// devices.
async fn account_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountInfo>, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    let devices = db::list_devices(&conn, account.id)?;
    Ok(Json(AccountInfo {
        account_id: account.id,
        email: account.email,
        created_at: rfc3339(account.created_at),
        devices: devices.iter().map(device_info).collect(),
    }))
}

/// `PUT /api/v1/vaults/{vault_id}` — the owning device registers its keyring
/// vault id; idempotent: 201 on first registration, 200 afterwards, 404 for a
/// vault someone else owns (ADR-0007 §5).
async fn create_vault(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(vault_id): Path<Uuid>,
) -> Result<(StatusCode, Json<VaultInfo>), ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    if let Some(vault) = db::owned_vault(&conn, account.id, vault_id)? {
        return Ok((StatusCode::OK, Json(vault_info(&vault))));
    }
    if db::owner_of_vault(&conn, vault_id)?.is_some() {
        // A foreign vault answers exactly like a missing one.
        return Err(vault_not_found());
    }
    let now = now_unix();
    db::create_vault(&conn, account.id, vault_id, now)?;
    Ok((
        StatusCode::CREATED,
        Json(VaultInfo {
            id: vault_id,
            created_at: rfc3339(now),
        }),
    ))
}

/// `GET /api/v1/vaults` — the account's vaults, oldest first.
async fn list_vaults(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<VaultList>, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    let vaults = db::vaults_for(&conn, account.id)?;
    Ok(Json(VaultList {
        vaults: vaults.iter().map(vault_info).collect(),
    }))
}

/// `GET /api/v1/vaults/{vault_id}/blobs` — every stored path, sorted
/// (`SyncTarget::list`).
async fn list_blobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(vault_id): Path<Uuid>,
) -> Result<Json<BlobList>, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    own_vault(&conn, account.id, vault_id)?;
    Ok(Json(BlobList {
        paths: db::blob_paths(&conn, vault_id)?,
    }))
}

/// `GET /api/v1/vaults/{vault_id}/blobs/{*path}` — the stored bytes
/// (`SyncTarget::read`).
async fn read_blob(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((vault_id, path)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    own_vault(&conn, account.id, vault_id)?;
    validate_blob_path(&path)?;
    let Some((_record, bytes)) = db::blob_by_path(&conn, vault_id, &path)? else {
        return Err(ApiError::NotFound("no such blob".to_string()));
    };
    Ok((
        [(CONTENT_TYPE, "application/octet-stream")],
        Bytes::from(bytes),
    ))
}

/// `PUT /api/v1/vaults/{vault_id}/blobs/{*path}` — create-only
/// (`SyncTarget::write`): 201 stored, 204 identical retry, 409 different
/// bytes (ADR-0007 §6).
async fn write_blob(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((vault_id, path)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.conn();
    let (account, device) = require_auth(&conn, &headers)?;
    own_vault(&conn, account.id, vault_id)?;
    validate_blob_path(&path)?;
    if body.len() > state.config.max_blob_bytes {
        return Err(ApiError::TooLarge);
    }
    let now = now_unix();
    match db::write_blob(
        &mut conn,
        vault_id,
        &path,
        &body,
        device.id,
        now,
        state.config.vault_quota_bytes,
    )? {
        db::BlobWrite::Created(_) => Ok(StatusCode::CREATED),
        db::BlobWrite::Unchanged(_) => Ok(StatusCode::NO_CONTENT),
        db::BlobWrite::Diverged => Err(ApiError::Conflict(format!(
            "`{path}` already exists with different bytes"
        ))),
    }
}

/// `GET /api/v1/vaults/{vault_id}/changes?since=&limit=` — the append-only
/// feed; `next` is the cursor for the following call (ADR-0007 §7).
async fn changes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(vault_id): Path<Uuid>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<ChangePage>, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    own_vault(&conn, account.id, vault_id)?;
    let since = query.since.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(CHANGES_LIMIT_DEFAULT)
        .clamp(1, CHANGES_LIMIT_MAX);
    let records = db::changes_since(&conn, vault_id, since, limit as usize)?;
    let next = records.last().map_or(since, |record| record.seq);
    Ok(Json(ChangePage {
        events: records
            .iter()
            .map(|record| ChangeEvent {
                seq: record.seq,
                path: record.path.clone(),
                device_id: record.device_id,
                size: record.size,
                at: rfc3339(record.created_at),
            })
            .collect(),
        next,
    }))
}

fn vault_info(vault: &db::Vault) -> VaultInfo {
    VaultInfo {
        id: vault.id,
        created_at: rfc3339(vault.created_at),
    }
}

/// The vault must exist and belong to the account; anything else is the same
/// 404, so accounts cannot probe each other's ids (ADR-0007 §5).
fn own_vault(conn: &Connection, account_id: Uuid, vault_id: Uuid) -> Result<db::Vault, ApiError> {
    db::owned_vault(conn, account_id, vault_id)?.ok_or_else(vault_not_found)
}

fn vault_not_found() -> ApiError {
    ApiError::NotFound("no such vault".to_string())
}

fn session(email: &str, account_id: Uuid, device_id: Uuid, token: String) -> Session {
    Session {
        account_id,
        email: email.to_string(),
        device_id,
        token,
    }
}

fn device_info(device: &Device) -> DeviceInfo {
    DeviceInfo {
        id: device.id,
        name: device.name.clone(),
        created_at: rfc3339(device.created_at),
        last_seen_at: rfc3339(device.last_seen_at),
    }
}

/// Resolve the `Authorization: Bearer` header, refreshing `last_seen_at`
/// when it is stale (ADR-0007 §3).
fn require_auth(conn: &Connection, headers: &HeaderMap) -> Result<(Account, Device), ApiError> {
    let token = bearer_token(headers).ok_or(ApiError::Unauthorized)?;
    let (account, device) =
        db::device_by_token(conn, &auth::hash_token(token))?.ok_or(ApiError::Unauthorized)?;
    let now = now_unix();
    if now - device.last_seen_at >= auth::LAST_SEEN_STALE_SECS {
        db::touch_device(conn, account.id, device.id, now)?;
    }
    Ok((account, device))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn clean_device_name(raw: &str) -> String {
    let clipped: String = raw.trim().chars().take(DEVICE_NAME_MAX_CHARS).collect();
    if clipped.is_empty() {
        "unnamed device".to_string()
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_parsing_is_strict_but_case_insensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer abc"));
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer    abc  "));
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer"));
        assert_eq!(bearer_token(&headers), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn device_names_are_clipped_and_defaulted() {
        assert_eq!(clean_device_name("  Pixel 9  "), "Pixel 9");
        assert_eq!(clean_device_name("   "), "unnamed device");
        assert_eq!(clean_device_name(&"n".repeat(100)).chars().count(), 64);
    }
}
