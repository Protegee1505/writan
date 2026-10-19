//! The writan-server sync target (ADR-0007 §§6, 11; ADR-0008 §§2–6).
//!
//! A server is the same dumb blob store as a shared folder or an S3 bucket:
//! [`ServerTransport`] is the HTTP seam (one method per REST call the target
//! needs), [`ServerHttpTransport`] is the live `ureq` implementation carrying
//! the device token, and [`ServerTarget`] maps the replies onto the engine's
//! create-only rules (ADR-0005 §12). Around them, [`login`], [`register`],
//! [`change_verifier`], [`logout`], [`discover_vault`], [`ensure_vault`] and
//! the pairing trio ([`create_pairing`], [`redeem_pairing`],
//! [`pair_payload`]) are the account calls a shell needs to set sync up.
//!
//! Since ADR-0008 the account password never crosses the wire: the client
//! derives an Argon2id *verifier* from it against a salt the server hands out
//! and sends only that.

use std::io;
use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use writan_crypto::{CryptoError, SALT_LEN, derive_kek, random_salt};

use crate::target::{SyncTarget, TargetError};

/// What a create-only `PUT` did (ADR-0007 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutStatus {
    /// `201`: the blob was stored.
    Created,
    /// `204`: the blob already held exactly these bytes (a retried write).
    Unchanged,
    /// `409`: the blob exists with different bytes.
    Diverged,
}

/// The REST seam of the server target: list a vault's blobs, read one and
/// store one create-only. Implementations are deliberately dumb, so the
/// target's rules stay testable without a network.
pub trait ServerTransport: Send + Sync {
    /// Every blob path in the vault, in any order.
    fn list(&self) -> Result<Vec<String>, io::Error>;
    /// The blob at `path`, or `None` when the server has none.
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error>;
    /// Store `bytes` at `path` if absent; identical bytes are a retried write.
    fn put(&self, path: &str, bytes: &[u8]) -> Result<PutStatus, io::Error>;
}

/// Where and as whom a transport talks to a server (ADR-0007 §§3–5).
#[derive(Debug, Clone)]
pub struct ServerConnection {
    /// Server root, e.g. `https://notes.example.com` (a trailing `/` is fine).
    pub base_url: String,
    /// The device token from a sign-in or a pairing; never the vault key.
    pub token: String,
    /// The vault to work on. `None` asks the server for the account's single
    /// vault on first use — the join flow on a device that has none yet.
    pub vault_id: Option<Uuid>,
}

/// [`SyncTarget`] over the server's blob API.
pub struct ServerTarget<T: ServerTransport> {
    transport: T,
}

impl<T: ServerTransport> ServerTarget<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: ServerTransport> SyncTarget for ServerTarget<T> {
    fn list(&self) -> Result<Vec<String>, TargetError> {
        self.transport.list().map_err(TargetError::Io)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError> {
        match self.transport.get(path)? {
            Some(bytes) => Ok(bytes),
            None => Err(TargetError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the server has no blob at {path}"),
            ))),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError> {
        match self.transport.put(path, bytes)? {
            PutStatus::Created | PutStatus::Unchanged => Ok(()),
            PutStatus::Diverged => Err(TargetError::Diverged(path.to_string())),
        }
    }
}

/// A refused or failed server call, with the wording the UI shows.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("could not reach the server: {0}")]
    Transport(String),
    #[error("the server refused the email or password")]
    Credentials,
    #[error("the server refused the current password — sign in again, then try once more")]
    CurrentPasswordRefused,
    #[error("that email already has an account on this server — sign in instead")]
    EmailTaken,
    #[error("the device token is not valid any more — sign in again")]
    Token,
    #[error("the server refused the request: {0}")]
    Refused(String),
    #[error("this account has no vault on the server yet")]
    NoVault,
    #[error("this account has {0} vaults on the server; this version supports one")]
    SeveralVaults(usize),
    #[error("the server's vault does not match the one this device syncs")]
    VaultMismatch,
    #[error("could not derive the sign-in verifier: {0}")]
    Derive(CryptoError),
}

/// The details a sign-in needs. The password is the *one* password
/// (ADR-0008 §1): it becomes the account verifier here and unlocks the
/// keyring on the client.
#[derive(Debug, Clone)]
pub struct ServerLogin {
    pub url: String,
    pub email: String,
    pub password: String,
    /// This device's sync id; re-registering under the same id replaces the
    /// old token (ADR-0007 §3).
    pub device_id: Uuid,
    pub device_name: String,
}

/// Create the account — registering this device — and return its token
/// (`POST /api/v1/accounts`). The password never leaves this process: a fresh
/// auth salt goes up with its Argon2id verifier (ADR-0008 §2). Fails with
/// [`ServerError::EmailTaken`] when the email is already registered.
pub fn register(details: &ServerLogin) -> Result<String, ServerError> {
    register_at(&Api::new(&details.url, ""), details)
}

/// Sign in — registering this device if it is new — and return its token
/// (`POST /api/v1/sessions`). Asks for this email's auth salt, then sends the
/// verifier derived from the password (ADR-0008 §§2–3); the token belongs in
/// the OS keychain.
pub fn login(details: &ServerLogin) -> Result<String, ServerError> {
    login_at(&Api::new(&details.url, ""), details)
}

/// Replace the account's verifier (`POST /api/v1/account/verifier`) after
/// proving `old_password`, so every other device of the account is revoked
/// and the caller keeps its token (ADR-0008 §6). Both passwords are derived
/// here; only verifiers travel.
pub fn change_verifier(
    base_url: &str,
    token: &str,
    old_password: &str,
    new_password: &str,
) -> Result<(), ServerError> {
    change_verifier_at(&Api::new(base_url, token), old_password, new_password)
}

/// Sign this device out; its token dies (`DELETE /api/v1/sessions`).
pub fn logout(base_url: &str, token: &str) -> Result<(), ServerError> {
    logout_at(&Api::new(base_url, token))
}

/// The account's vault on the server: `None` when there is none, refused when
/// there is more than one (v1 has one vault per account, ADR-0007 §5).
pub fn discover_vault(base_url: &str, token: &str) -> Result<Option<Uuid>, ServerError> {
    discover_at(&Api::new(base_url, token))
}

/// Make sure `vault_id` exists for this account; idempotent (`201` created,
/// `200` already owned).
pub fn ensure_vault(base_url: &str, token: &str, vault_id: Uuid) -> Result<(), ServerError> {
    ensure_at(&Api::new(base_url, token), vault_id)
}

/// A fresh pairing code and the instant it stops working, RFC 3339
/// (ADR-0007 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub code: String,
    pub expires_at: String,
}

/// Mint a single-use pairing code for this account
/// (`POST /api/v1/pairings`); render it as a QR with [`pair_payload`].
pub fn create_pairing(base_url: &str, token: &str) -> Result<Pairing, ServerError> {
    create_pairing_at(&Api::new(base_url, token))
}

/// Trade a pairing code for this device's own token
/// (`POST /api/v1/pairings/redeem`). Blob access only — the vault key never
/// travels, and the device still needs the one password (or a recovery code)
/// to open the keyring (ADR-0007 §4, ADR-0008 §1).
pub fn redeem_pairing(
    base_url: &str,
    code: &str,
    device_id: Uuid,
    device_name: &str,
) -> Result<String, ServerError> {
    redeem_pairing_at(&Api::new(base_url, ""), code, device_id, device_name)
}

/// The format marker of a pairing QR payload (ADR-0007 §4).
pub const PAIRING_FORMAT: &str = "writan-pair/1";

/// What a pairing QR carries: compact JSON, versioned by `format`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairPayload {
    pub format: String,
    pub url: String,
    pub code: String,
    pub email: String,
}

/// The QR text for a fresh pairing code.
pub fn pair_payload(url: &str, code: &str, email: &str) -> String {
    serde_json::json!({
        "format": PAIRING_FORMAT,
        "url": url.trim().trim_end_matches('/'),
        "code": code,
        "email": email.trim().to_lowercase(),
    })
    .to_string()
}

/// Read a scanned pairing QR; anything that is not a usable `writan-pair/1`
/// payload is refused with the reason (ADR-0007 §4).
pub fn parse_pair_payload(text: &str) -> Result<PairPayload, ServerError> {
    let payload: PairPayload = serde_json::from_str(text)
        .map_err(|_| ServerError::Refused("this is not a writan pairing code".to_string()))?;
    if payload.format != PAIRING_FORMAT {
        return Err(ServerError::Refused(format!(
            "this pairing code was made by another version (format `{}`)",
            payload.format
        )));
    }
    if !(payload.url.starts_with("http://") || payload.url.starts_with("https://")) {
        return Err(ServerError::Refused(
            "this pairing code carries no server address".to_string(),
        ));
    }
    if payload.code.trim().is_empty() {
        return Err(ServerError::Refused(
            "this pairing code carries no code".to_string(),
        ));
    }
    Ok(payload)
}

fn register_at(api: &Api, details: &ServerLogin) -> Result<String, ServerError> {
    let salt = random_salt();
    let verifier = verifier_for(&details.password, &salt)?;
    let body = serde_json::json!({
        "email": details.email,
        "salt": STANDARD.encode(salt),
        "verifier": STANDARD.encode(verifier),
        "deviceId": details.device_id,
        "deviceName": details.device_name,
    });
    let (status, body) = api.post("/api/v1/accounts", &body)?;
    match status {
        200 | 201 => token_from(&body),
        409 => Err(ServerError::EmailTaken),
        _ => Err(refuse(status, &body)),
    }
}

fn login_at(api: &Api, details: &ServerLogin) -> Result<String, ServerError> {
    let salt = salt_for(api, &details.email)?;
    let verifier = verifier_for(&details.password, &salt)?;
    let body = serde_json::json!({
        "email": details.email,
        "verifier": STANDARD.encode(verifier),
        "deviceId": details.device_id,
        "deviceName": details.device_name,
    });
    let (status, body) = api.post("/api/v1/sessions", &body)?;
    match status {
        200 | 201 => token_from(&body),
        401 => Err(ServerError::Credentials),
        429 => Err(ServerError::Refused(
            "too many failed sign-ins — try again in a moment".to_string(),
        )),
        _ => Err(refuse(status, &body)),
    }
}

fn change_verifier_at(
    api: &Api,
    old_password: &str,
    new_password: &str,
) -> Result<(), ServerError> {
    // The old verifier must be derived against the *stored* salt, so the
    // account's email (and through it the salt) is fetched first.
    let email = account_email(api)?;
    let old_salt = salt_for(api, &email)?;
    let old_verifier = verifier_for(old_password, &old_salt)?;
    let salt = random_salt();
    let verifier = verifier_for(new_password, &salt)?;
    let body = serde_json::json!({
        "oldVerifier": STANDARD.encode(old_verifier),
        "salt": STANDARD.encode(salt),
        "verifier": STANDARD.encode(verifier),
    });
    let (status, body) = api.post("/api/v1/account/verifier", &body)?;
    match status {
        200 | 204 => Ok(()),
        401 => Err(ServerError::CurrentPasswordRefused),
        429 => Err(ServerError::Refused(
            "too many failed sign-ins — try again in a moment".to_string(),
        )),
        _ => Err(refuse(status, &body)),
    }
}

/// The account's email as the server knows it (`GET /api/v1/account`).
fn account_email(api: &Api) -> Result<String, ServerError> {
    let (status, body) = api.get("/api/v1/account")?;
    if status != 200 {
        return Err(refuse(status, &body));
    }
    json_body(&body)
        .and_then(|answer| answer.get("email")?.as_str().map(str::to_string))
        .ok_or_else(|| {
            ServerError::Transport("the server's account answer carried no email".to_string())
        })
}

/// The auth salt a sign-in verifier is derived against
/// (`GET /api/v1/accounts/salt`). Unknown emails answer a dummy salt, so the
/// call itself reveals nothing (ADR-0008 §3).
fn salt_for(api: &Api, email: &str) -> Result<[u8; SALT_LEN], ServerError> {
    let path = format!("/api/v1/accounts/salt?email={}", encode_query(email));
    let (status, body) = api.get(&path)?;
    if status != 200 {
        return Err(refuse(status, &body));
    }
    let text = json_body(&body)
        .and_then(|answer| answer.get("salt")?.as_str().map(str::to_string))
        .ok_or_else(|| {
            ServerError::Transport("the server's salt answer was not JSON".to_string())
        })?;
    let salt = STANDARD
        .decode(text.as_bytes())
        .map_err(|_| ServerError::Transport("the server's salt is not base64".to_string()))?;
    salt.try_into()
        .map_err(|_| ServerError::Transport("the server's salt has the wrong length".to_string()))
}

/// The verifier for a password: the pinned Argon2id KDF against the account's
/// auth salt (ADR-0008 §2). The server stores only its SHA-256.
fn verifier_for(password: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; 32], ServerError> {
    derive_kek(password, salt).map_err(ServerError::Derive)
}

/// The JSON of an answer, when it is one.
fn json_body(body: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(body).ok()
}

/// The token out of a `Session` answer.
fn token_from(body: &[u8]) -> Result<String, ServerError> {
    json_body(body)
        .and_then(|answer| answer.get("token")?.as_str().map(str::to_string))
        .ok_or_else(|| ServerError::Transport("the server's answer carried no token".to_string()))
}

fn create_pairing_at(api: &Api) -> Result<Pairing, ServerError> {
    let (status, body) = api.post("/api/v1/pairings", &serde_json::json!({}))?;
    if !(200..300).contains(&status) {
        return Err(refuse(status, &body));
    }
    let answer = json_body(&body).ok_or_else(|| {
        ServerError::Transport("the server's pairing answer was not JSON".to_string())
    })?;
    let code = answer
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if code.is_empty() {
        return Err(ServerError::Transport(
            "the server's pairing answer carried no code".to_string(),
        ));
    }
    let expires_at = answer
        .get("expiresAt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(Pairing { code, expires_at })
}

fn redeem_pairing_at(
    api: &Api,
    code: &str,
    device_id: Uuid,
    device_name: &str,
) -> Result<String, ServerError> {
    let body = serde_json::json!({
        "code": code,
        "deviceId": device_id,
        "deviceName": device_name,
    });
    let (status, body) = api.post("/api/v1/pairings/redeem", &body)?;
    match status {
        200 | 201 => token_from(&body),
        401 => Err(ServerError::Refused(
            "this pairing code is not valid any more".to_string(),
        )),
        _ => Err(refuse(status, &body)),
    }
}

fn logout_at(api: &Api) -> Result<(), ServerError> {
    let (status, body) = api.delete("/api/v1/sessions")?;
    match status {
        200 | 204 => Ok(()),
        _ => Err(refuse(status, &body)),
    }
}

fn discover_at(api: &Api) -> Result<Option<Uuid>, ServerError> {
    let (status, body) = api.get("/api/v1/vaults")?;
    if status != 200 {
        return Err(refuse(status, &body));
    }
    let ids: Vec<Uuid> = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|answer| {
            answer.get("vaults")?.as_array().map(|vaults| {
                vaults
                    .iter()
                    .filter_map(|vault| vault.get("id")?.as_str()?.parse().ok())
                    .collect()
            })
        })
        .unwrap_or_default();
    match ids.len() {
        0 => Ok(None),
        1 => Ok(Some(ids[0])),
        count => Err(ServerError::SeveralVaults(count)),
    }
}

fn ensure_at(api: &Api, vault_id: Uuid) -> Result<(), ServerError> {
    let (status, body) = api.put(&format!("/api/v1/vaults/{vault_id}"), &[])?;
    match status {
        200 | 201 => Ok(()),
        _ => Err(refuse(status, &body)),
    }
}

/// The live transport: blocking `ureq` calls with the device token.
pub struct ServerHttpTransport {
    api: Api,
    /// The vault from the connection details, if the caller had one.
    requested: Option<Uuid>,
    /// Resolved once: the vault this transport works on.
    resolved: OnceLock<Uuid>,
}

impl ServerHttpTransport {
    pub fn new(connection: ServerConnection) -> Self {
        Self {
            api: Api::new(&connection.base_url, &connection.token),
            requested: connection.vault_id,
            resolved: OnceLock::new(),
        }
    }

    /// The vault to work on: resolved once, checked against the server, so a
    /// device whose local state points at another vault fails loudly instead
    /// of mixing two note trees.
    fn vault(&self) -> Result<Uuid, io::Error> {
        if let Some(vault) = self.resolved.get() {
            return Ok(*vault);
        }
        let stored = discover_at(&self.api).map_err(describe)?;
        let vault = match (self.requested, stored) {
            (Some(wanted), Some(found)) if found == wanted => wanted,
            (Some(_), Some(_)) => return Err(describe(ServerError::VaultMismatch)),
            (Some(_), None) | (None, None) => return Err(describe(ServerError::NoVault)),
            (None, Some(found)) => found,
        };
        let _ = self.resolved.set(vault);
        Ok(vault)
    }

    fn blobs_url(&self, vault: Uuid, path: Option<&str>) -> String {
        match path {
            Some(path) => format!("/api/v1/vaults/{vault}/blobs/{}", encode_path(path)),
            None => format!("/api/v1/vaults/{vault}/blobs"),
        }
    }
}

impl ServerTransport for ServerHttpTransport {
    fn list(&self) -> Result<Vec<String>, io::Error> {
        let vault = self.vault()?;
        let (status, body) = self
            .api
            .get(&self.blobs_url(vault, None))
            .map_err(describe)?;
        if status != 200 {
            return Err(describe(refuse(status, &body)));
        }
        let paths = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|answer| {
                answer.get("paths")?.as_array().map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default();
        Ok(paths)
    }

    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error> {
        let vault = self.vault()?;
        let (status, body) = self
            .api
            .get(&self.blobs_url(vault, Some(path)))
            .map_err(describe)?;
        match status {
            200 => Ok(Some(body)),
            404 => Ok(None),
            _ => Err(describe(refuse(status, &body))),
        }
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<PutStatus, io::Error> {
        let vault = self.vault()?;
        let (status, body) = self
            .api
            .put(&self.blobs_url(vault, Some(path)), bytes)
            .map_err(describe)?;
        match status {
            201 => Ok(PutStatus::Created),
            204 => Ok(PutStatus::Unchanged),
            409 => Ok(PutStatus::Diverged),
            _ => Err(describe(refuse(status, &body))),
        }
    }
}

/// The HTTP plumbing the connect helpers and the live transport share: the
/// base URL, the token and the wording of failures.
struct Api {
    agent: ureq::Agent,
    base_url: String,
    token: String,
}

impl Api {
    fn new(base_url: &str, token: &str) -> Self {
        Self {
            agent: plain_agent(),
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// The authorization header, when this client has a token.
    fn auth(&self) -> Option<(&'static str, String)> {
        (!self.token.is_empty()).then(|| ("authorization", format!("Bearer {}", self.token)))
    }

    fn get(&self, path: &str) -> Result<(u16, Vec<u8>), ServerError> {
        let mut request = self.agent.get(self.url(path));
        if let Some((name, value)) = self.auth() {
            request = request.header(name, value);
        }
        read(request.call().map_err(reach)?)
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(u16, Vec<u8>), ServerError> {
        let mut request = self.agent.put(self.url(path));
        if let Some((name, value)) = self.auth() {
            request = request.header(name, value);
        }
        read(request.send(bytes).map_err(reach)?)
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<(u16, Vec<u8>), ServerError> {
        let payload =
            serde_json::to_vec(body).map_err(|error| ServerError::Transport(error.to_string()))?;
        let mut request = self
            .agent
            .post(self.url(path))
            .header("content-type", "application/json");
        if let Some((name, value)) = self.auth() {
            request = request.header(name, value);
        }
        read(request.send(payload.as_slice()).map_err(reach)?)
    }

    fn delete(&self, path: &str) -> Result<(u16, Vec<u8>), ServerError> {
        let mut request = self.agent.delete(self.url(path));
        if let Some((name, value)) = self.auth() {
            request = request.header(name, value);
        }
        read(request.call().map_err(reach)?)
    }
}

/// The agent talking to the server: non-2xx answers come back as responses,
/// so their error text can be shown instead of a bare status.
fn plain_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// The status and body of any answer.
fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), ServerError> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| ServerError::Transport(error.to_string()))?;
    Ok((status, body))
}

/// An unreachable server.
fn reach(error: ureq::Error) -> ServerError {
    ServerError::Transport(error.to_string())
}

/// A refused authenticated call: `401` is the token, everything else carries
/// the server's own message.
fn refuse(status: u16, body: &[u8]) -> ServerError {
    match status {
        401 => ServerError::Token,
        _ => {
            let reported = error_text(body);
            if reported.is_empty() {
                ServerError::Refused(format!("the server answered HTTP {status}"))
            } else {
                ServerError::Refused(reported)
            }
        }
    }
}

/// The server's own error text (`{"error": "…"}`), or the raw body.
fn error_text(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|answer| answer.get("error")?.as_str().map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string())
}

/// A [`ServerError`] as the I/O error the target layer carries.
fn describe(error: ServerError) -> io::Error {
    let kind = match error {
        ServerError::Token => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error.to_string())
}

/// Percent-encode everything outside the unreserved set, keeping `/` as the
/// segment separator (blob paths are opaque to the server, ADR-0007 §6).
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Percent-encode a query-string value (an email carries `@` and maybe `+`).
fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    /// An in-memory stand-in for the server: the same create-only rules,
    /// without a network.
    #[derive(Default)]
    struct Canned {
        blobs: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl ServerTransport for Canned {
        fn list(&self) -> Result<Vec<String>, io::Error> {
            Ok(self.blobs.lock().unwrap().keys().cloned().collect())
        }

        fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error> {
            Ok(self.blobs.lock().unwrap().get(path).cloned())
        }

        fn put(&self, path: &str, bytes: &[u8]) -> Result<PutStatus, io::Error> {
            let mut blobs = self.blobs.lock().unwrap();
            match blobs.get(path) {
                Some(existing) if existing == bytes => Ok(PutStatus::Unchanged),
                Some(_) => Ok(PutStatus::Diverged),
                None => {
                    blobs.insert(path.to_string(), bytes.to_vec());
                    Ok(PutStatus::Created)
                }
            }
        }
    }

    #[test]
    fn writes_are_create_only_and_retries_pass() {
        let target = ServerTarget::new(Canned::default());
        target.write("keys/1.json", b"one").expect("create");
        target
            .write("keys/1.json", b"one")
            .expect("identical retry");

        let error = target.write("keys/1.json", b"two").expect_err("diverged");
        match error {
            TargetError::Diverged(path) => assert_eq!(path, "keys/1.json"),
            other => panic!("expected a divergence, got {other:?}"),
        }

        assert_eq!(target.read("keys/1.json").expect("read"), b"one");
        assert_eq!(target.list().expect("list"), ["keys/1.json"]);
    }

    #[test]
    fn missing_blobs_read_as_not_found() {
        let target = ServerTarget::new(Canned::default());
        let error = target.read("items/nope.enc").expect_err("missing");
        match error {
            TargetError::Io(error) => assert_eq!(error.kind(), io::ErrorKind::NotFound),
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }

    #[test]
    fn blob_paths_keep_their_shape() {
        assert_eq!(encode_path("items/2a/9b/0007.enc"), "items/2a/9b/0007.enc");
        assert_eq!(encode_path("odd name+1.enc"), "odd%20name%2B1.enc");
        assert_eq!(encode_path("keys/1.json"), "keys/1.json");
    }

    #[test]
    fn pairing_payloads_roundtrip_and_refuse_foreign_shapes() {
        let text = pair_payload("https://notes.example.com/", "abc123", " Ada@Example.COM ");
        let payload = parse_pair_payload(&text).expect("roundtrip");
        assert_eq!(payload.format, "writan-pair/1");
        assert_eq!(payload.url, "https://notes.example.com");
        assert_eq!(payload.code, "abc123");
        assert_eq!(payload.email, "ada@example.com");
        assert!(parse_pair_payload("not json at all").is_err());
        let older = r#"{"format":"writan-pair/0","url":"https://x","code":"c","email":"a@b"}"#;
        assert!(parse_pair_payload(older).is_err());
        let no_url = r#"{"format":"writan-pair/1","url":"ftp://x","code":"c","email":"a@b"}"#;
        assert!(parse_pair_payload(no_url).is_err());
        let no_code = r#"{"format":"writan-pair/1","url":"https://x","code":" ","email":"a@b"}"#;
        assert!(parse_pair_payload(no_code).is_err());
    }
}
