//! The writan-server sync target (ADR-0007 §§6, 11).
//!
//! A server is the same dumb blob store as a shared folder or an S3 bucket:
//! [`ServerTransport`] is the HTTP seam (one method per REST call the target
//! needs), [`ServerHttpTransport`] is the live `ureq` implementation carrying
//! the device token, and [`ServerTarget`] maps the replies onto the engine's
//! create-only rules (ADR-0005 §12). Around them, [`login`], [`logout`],
//! [`discover_vault`] and [`ensure_vault`] are the account calls a shell needs
//! to set sync up.

use std::io;
use std::sync::OnceLock;

use uuid::Uuid;

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
}

/// The details a sign-in needs. The password is the *account* password
/// (ADR-0007 §2), not the sync password that unlocks the keyring.
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
/// (`POST /api/v1/accounts`). Fails with [`ServerError::EmailTaken`] when the
/// email is already registered.
pub fn register(details: &ServerLogin) -> Result<String, ServerError> {
    register_at(&Api::new(&details.url, ""), details)
}

/// Sign in — registering this device if it is new — and return its token
/// (`POST /api/v1/sessions`). The token belongs in the OS keychain.
pub fn login(details: &ServerLogin) -> Result<String, ServerError> {
    login_at(&Api::new(&details.url, ""), details)
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

fn credentials_body(details: &ServerLogin) -> serde_json::Value {
    serde_json::json!({
        "email": details.email,
        "password": details.password,
        "deviceId": details.device_id,
        "deviceName": details.device_name,
    })
}

fn register_at(api: &Api, details: &ServerLogin) -> Result<String, ServerError> {
    let (status, body) = api.post("/api/v1/accounts", &credentials_body(details))?;
    match status {
        200 | 201 => token_from(&body),
        409 => Err(ServerError::EmailTaken),
        _ => Err(refuse(status, &body)),
    }
}

fn login_at(api: &Api, details: &ServerLogin) -> Result<String, ServerError> {
    let (status, body) = api.post("/api/v1/sessions", &credentials_body(details))?;
    match status {
        200 | 201 => token_from(&body),
        401 => Err(ServerError::Credentials),
        429 => Err(ServerError::Refused(
            "too many failed sign-ins — try again in a moment".to_string(),
        )),
        _ => Err(refuse(status, &body)),
    }
}

/// The token out of a `Session` answer.
fn token_from(body: &[u8]) -> Result<String, ServerError> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|answer| answer.get("token")?.as_str().map(str::to_string))
        .ok_or_else(|| ServerError::Transport("the server's answer carried no token".to_string()))
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
}
