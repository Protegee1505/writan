//! The HTTP surface (ADR-0007 §§2–4). Chunk A: health, accounts, sessions.
//!
//! Handlers take the single writer connection behind a mutex — cheap at one
//! user per instance (ADR-0007 §8); a connection pool is a later change with
//! no format or API impact.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::routing::{get, post};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth;
use crate::config::ServerConfig;
use crate::db::{self, Account, Device};
use crate::error::ApiError;
use crate::util::{now_unix, rfc3339};

/// Longest stored device name (characters).
const DEVICE_NAME_MAX_CHARS: usize = 64;

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
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/accounts", post(create_account))
        .route("/api/v1/sessions", post(login).delete(logout))
        .route("/api/v1/account", get(account_info))
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
