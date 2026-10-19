//! The HTTP surface (ADR-0007 §§2–7, ADR-0008 §§2–3,6). Health, the salt
//! lookup, accounts, sessions, the verifier change, vaults, the blob API, the
//! change feed and its SSE stream, and pairing.
//!
//! Handlers take the single writer connection behind a mutex — cheap at one
//! user per instance (ADR-0007 §8); a connection pool is a later change with
//! no format or API impact.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post, put};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::stream::unfold;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth;
use crate::config::ServerConfig;
use crate::db::{self, Account, BlobRecord, Device};
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

/// Live fan-out buffer for the SSE stream. A subscriber that falls further
/// behind than this is told it lagged and re-reads the REST feed — which is
/// authoritative (ADR-0007 §7).
const FEED_CAPACITY: usize = 256;

/// What a blob write publishes to SSE subscribers.
type FeedEvent = (Uuid, BlobRecord);

/// Everything the handlers need.
#[derive(Clone)]
pub struct AppState {
    /// The one writer connection, shared across handler tasks.
    pub db: Arc<Mutex<Connection>>,
    /// Startup configuration (limits, registration switch, lockout).
    pub config: Arc<ServerConfig>,
    /// Live change events for the SSE streams.
    pub events: broadcast::Sender<FeedEvent>,
    /// The HMAC key behind dummy salts for unknown emails (ADR-0008 §3).
    dummy_secret: [u8; 32],
}

impl AppState {
    /// Wrap an open connection, loading (or minting) the dummy-salt secret.
    pub fn new(conn: Connection, config: ServerConfig) -> Result<Self, ApiError> {
        let dummy_secret = db::dummy_salt_secret(&conn)?;
        let (events, _) = broadcast::channel(FEED_CAPACITY);
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            config: Arc::new(config),
            events,
            dummy_secret,
        })
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
        .route("/api/v1/accounts/salt", get(account_salt))
        .route("/api/v1/sessions", post(login).delete(logout))
        .route("/api/v1/account", get(account_info))
        .route("/api/v1/account/verifier", post(change_verifier))
        .route("/api/v1/vaults", get(list_vaults))
        .route("/api/v1/vaults/{vault_id}", put(create_vault))
        .route("/api/v1/vaults/{vault_id}/blobs", get(list_blobs))
        .route(
            "/api/v1/vaults/{vault_id}/blobs/{*path}",
            get(read_blob).put(write_blob),
        )
        .route("/api/v1/vaults/{vault_id}/changes", get(changes))
        .route("/api/v1/vaults/{vault_id}/events", get(events))
        .route("/api/v1/pairings", post(create_pairing))
        .route("/api/v1/pairings/redeem", post(redeem_pairing))
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
struct SaltQuery {
    email: String,
}

#[derive(Debug, Serialize)]
struct SaltResponse {
    salt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    email: String,
    salt: String,
    verifier: String,
    device_id: Uuid,
    device_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignInRequest {
    email: String,
    verifier: String,
    device_id: Uuid,
    device_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeVerifierRequest {
    old_verifier: String,
    salt: String,
    verifier: String,
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
struct PairingInfo {
    code: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemRequest {
    code: String,
    device_id: Uuid,
    device_name: String,
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

/// `GET /api/v1/accounts/salt?email=` — the account's `salt_auth`, or a dummy
/// salt for an unknown email (ADR-0008 §§2–3). The shape is identical either
/// way, so the endpoint cannot be used to enumerate accounts.
async fn account_salt(
    State(state): State<AppState>,
    Query(query): Query<SaltQuery>,
) -> Result<Json<SaltResponse>, ApiError> {
    let email = auth::normalize_email(&query.email)?;
    let conn = state.conn();
    let salt = db::account_by_email(&conn, &email)?
        .and_then(|material| material.salt)
        .unwrap_or_else(|| auth::dummy_salt(&state.dummy_secret, &email));
    Ok(Json(SaltResponse {
        salt: STANDARD.encode(salt),
    }))
}

/// `POST /api/v1/accounts` — register with a verifier and sign the device in.
async fn create_account(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    if !state.config.registration_open {
        return Err(ApiError::RegistrationClosed);
    }
    let email = auth::normalize_email(&request.email)?;
    let salt = auth::parse_salt(&request.salt)?;
    let verifier = auth::parse_verifier(&request.verifier)?;
    let device_name = clean_device_name(&request.device_name);
    let now = now_unix();
    let conn = state.conn();
    let account = db::create_account(&conn, &email, &salt, &auth::hash_verifier(&verifier), now)?;
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

/// `POST /api/v1/sessions` — email + verifier, answered with a fresh device
/// token (the sign-in of both shells).
async fn login(
    State(state): State<AppState>,
    Json(request): Json<SignInRequest>,
) -> Result<Json<Session>, ApiError> {
    let email = auth::normalize_email(&request.email)?;
    let verifier = auth::parse_verifier(&request.verifier)?;
    let device_name = clean_device_name(&request.device_name);
    let now = now_unix();
    let conn = state.conn();
    let Some(material) = db::account_by_email(&conn, &email)? else {
        return Err(ApiError::Unauthorized);
    };
    // A row that predates schema v2 has no verifier material; the 403 (before
    // the lockout check) is what sends its owner to `reset-password`
    // (ADR-0008 §4).
    let Some(stored) = material.verifier_hash else {
        return Err(ApiError::LegacyAccount);
    };
    if material.account.locked_until > now {
        return Err(ApiError::Locked);
    }
    if !auth::verifier_matches(&verifier, &stored) {
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

/// `POST /api/v1/account/verifier` — change the password: replace the salt and
/// verifier hash after checking the old verifier, and revoke every other
/// device (ADR-0008 §6). The caller's own token stays valid.
async fn change_verifier(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChangeVerifierRequest>,
) -> Result<StatusCode, ApiError> {
    let now = now_unix();
    let mut conn = state.conn();
    // Authentication first: an unauthenticated caller learns nothing about
    // the body, and the material is only parsed for the account that sent it.
    let (account, device) = require_auth(&conn, &headers)?;
    let old_verifier = auth::parse_verifier(&request.old_verifier)?;
    let salt = auth::parse_salt(&request.salt)?;
    let verifier = auth::parse_verifier(&request.verifier)?;
    let Some(material) = db::account_by_email(&conn, &account.email)? else {
        return Err(ApiError::Unauthorized);
    };
    let Some(stored) = material.verifier_hash else {
        return Err(ApiError::LegacyAccount);
    };
    if material.account.locked_until > now {
        return Err(ApiError::Locked);
    }
    if !auth::verifier_matches(&old_verifier, &stored) {
        let locked_until = db::record_login_failure(&conn, account.id, &state.config.lockout, now)?;
        return Err(if locked_until > now {
            ApiError::Locked
        } else {
            ApiError::Unauthorized
        });
    }
    if !db::replace_verifier(
        &mut conn,
        account.id,
        &salt,
        &auth::hash_verifier(&verifier),
        Some(device.id),
    )? {
        return Err(ApiError::Unauthorized);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/pairings` — mint a single-use code another device can trade
/// for its own token (ADR-0007 §4). The code lives ten minutes by default
/// (`WRITAN_PAIRING_TTL_SECS`) and is stored hashed.
async fn create_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PairingInfo>, ApiError> {
    let conn = state.conn();
    let (account, _) = require_auth(&conn, &headers)?;
    let code = auth::generate_token();
    let expires_at = db::create_pairing(
        &conn,
        account.id,
        &auth::hash_token(&code),
        state.config.pairing_ttl_secs,
        now_unix(),
    )?;
    Ok(Json(PairingInfo {
        code,
        expires_at: rfc3339(expires_at),
    }))
}

/// `POST /api/v1/pairings/redeem` — the joining device trades a code for its
/// own token; no account password is involved (ADR-0007 §4). Blob access
/// only: the vault key never travels.
async fn redeem_pairing(
    State(state): State<AppState>,
    Json(request): Json<RedeemRequest>,
) -> Result<Json<Session>, ApiError> {
    let code = request.code.trim();
    if code.is_empty() {
        return Err(ApiError::BadRequest("enter the pairing code".to_string()));
    }
    let device_name = clean_device_name(&request.device_name);
    let token = auth::generate_token();
    let mut conn = state.conn();
    let account = db::redeem_pairing(
        &mut conn,
        &auth::hash_token(code),
        request.device_id,
        &device_name,
        &auth::hash_token(&token),
        now_unix(),
    )?
    .ok_or(ApiError::Unauthorized)?;
    Ok(Json(session(
        &account.email,
        account.id,
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
        db::BlobWrite::Created(record) => {
            // Advisory fan-out to blocking SSE readers; a missed event only
            // costs a later poll (ADR-0007 §7).
            let _ = state.events.send((vault_id, record));
            Ok(StatusCode::CREATED)
        }
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
        events: records.iter().map(change_event).collect(),
        next,
    }))
}

/// `GET /api/v1/vaults/{vault_id}/events` — the same feed as SSE
/// (ADR-0007 §7): `id:` is the sequence, so a reconnecting client resumes
/// with `Last-Event-ID` (or `?since=`). Each connection first replays
/// everything it missed, then follows live writes.
async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(vault_id): Path<Uuid>,
    Query(query): Query<ChangesQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let cursor = last_event_id(&headers)?.or(query.since).unwrap_or(0).max(0);

    // Subscribe before reading the backlog: anything written in between is
    // seen twice and dropped by the sequence filter, never missed.
    let receiver = state.events.subscribe();
    let mut queue = VecDeque::new();
    let mut last = cursor;
    {
        let conn = state.conn();
        let (account, _) = require_auth(&conn, &headers)?;
        own_vault(&conn, account.id, vault_id)?;
        loop {
            let page = db::changes_since(&conn, vault_id, last, CHANGES_LIMIT_MAX as usize)?;
            let Some(max) = page.last().map(|record| record.seq) else {
                break;
            };
            last = max;
            let exhausted = page.len() < CHANGES_LIMIT_MAX as usize;
            queue.extend(page.iter().map(sse_event));
            if exhausted {
                break;
            }
        }
    }

    let stream = unfold(
        (queue, receiver, last, vault_id, state),
        |(mut queue, mut receiver, mut last, vault_id, state)| async move {
            loop {
                if let Some(event) = queue.pop_front() {
                    return Some((Ok(event), (queue, receiver, last, vault_id, state)));
                }
                match receiver.recv().await {
                    Ok((written, record)) if written == vault_id && record.seq > last => {
                        last = record.seq;
                        let event = sse_event(&record);
                        return Some((Ok(event), (queue, receiver, last, vault_id, state)));
                    }
                    // Other vaults and events the backlog already carried.
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Too far behind: re-read from the authoritative feed.
                        let conn = state.conn();
                        if let Ok(page) =
                            db::changes_since(&conn, vault_id, last, CHANGES_LIMIT_MAX as usize)
                        {
                            if let Some(max) = page.last().map(|record| record.seq) {
                                last = max;
                            }
                            queue.extend(page.iter().map(sse_event));
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn change_event(record: &BlobRecord) -> ChangeEvent {
    ChangeEvent {
        seq: record.seq,
        path: record.path.clone(),
        device_id: record.device_id,
        size: record.size,
        at: rfc3339(record.created_at),
    }
}

fn sse_event(record: &BlobRecord) -> Event {
    let data = serde_json::to_string(&change_event(record)).unwrap_or_else(|_| "{}".to_string());
    Event::default().id(record.seq.to_string()).data(data)
}

/// The `Last-Event-ID` request header (ADR-0007 §7); a malformed value is a
/// client bug, not something to guess at.
fn last_event_id(headers: &HeaderMap) -> Result<Option<i64>, ApiError> {
    let Some(value) = headers.get("last-event-id") else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| ApiError::BadRequest("last-event-id is not a sequence number".to_string()))?;
    raw.trim()
        .parse::<i64>()
        .map(Some)
        .map_err(|_| ApiError::BadRequest("last-event-id is not a sequence number".to_string()))
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

    #[test]
    fn last_event_id_parses_but_does_not_guess() {
        let mut headers = HeaderMap::new();
        assert_eq!(last_event_id(&headers).expect("absent"), None);
        headers.insert("last-event-id", HeaderValue::from_static(" 42 "));
        assert_eq!(last_event_id(&headers).expect("number"), Some(42));
        headers.insert("last-event-id", HeaderValue::from_static("nope"));
        assert!(matches!(
            last_event_id(&headers),
            Err(ApiError::BadRequest(_))
        ));
    }
}
