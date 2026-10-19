//! End-to-end HTTP tests for pairing (ADR-0007 §4), through the real router
//! with an in-memory database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;
use writan_server::{AppState, ServerConfig, app, db};

fn state_with(config: ServerConfig) -> AppState {
    let conn = db::open(std::path::Path::new(":memory:")).expect("in-memory database");
    AppState::new(conn, config).expect("state")
}

async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = app(state.clone())
        .oneshot(request)
        .await
        .expect("infallible");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn request(method: &str, path: &str, body: Value, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

/// Register an account (which signs the device in) and give back its token.
/// The helper derives the verifier from the password against the server's
/// salt, as the shells do (ADR-0008 §2).
async fn sign_up(state: &AppState, email: &str, device: Uuid) -> String {
    let salt = salt_for(state, email).await;
    let verifier = verifier_for("correct horse", &salt);
    let (status, answer) = call(
        state,
        request(
            "POST",
            "/api/v1/accounts",
            json!({"email": email, "salt": salt, "verifier": verifier,
                   "deviceId": device, "deviceName": "laptop"}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    answer["token"].as_str().expect("token").to_string()
}

/// The salt a client derives its verifier against (ADR-0008 §2).
async fn salt_for(state: &AppState, email: &str) -> String {
    let (status, body) = call(
        state,
        request(
            "GET",
            &format!("/api/v1/accounts/salt?email={email}"),
            json!({}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["salt"].as_str().expect("salt").to_string()
}

fn verifier_for(password: &str, salt: &str) -> String {
    let salt = writan_server::auth::parse_salt(salt).expect("salt is valid");
    let verifier = writan_crypto::derive_kek(password, &salt).expect("kdf");
    base64::engine::general_purpose::STANDARD.encode(verifier)
}

async fn mint(state: &AppState, token: &str) -> (StatusCode, Value) {
    call(
        state,
        request("POST", "/api/v1/pairings", json!({}), Some(token)),
    )
    .await
}

async fn redeem(state: &AppState, code: &str, device: Uuid) -> (StatusCode, Value) {
    call(
        state,
        request(
            "POST",
            "/api/v1/pairings/redeem",
            json!({"code": code, "deviceId": device, "deviceName": "phone"}),
            None,
        ),
    )
    .await
}

#[tokio::test]
async fn minting_needs_a_token() {
    let state = state_with(ServerConfig::default());
    let (status, _) = mint(&state, "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_code_is_redeemed_once_and_registers_the_device() {
    let state = state_with(ServerConfig::default());
    let laptop = Uuid::new_v4();
    let token = sign_up(&state, "ada@example.com", laptop).await;
    let (status, pairing) = mint(&state, &token).await;
    assert_eq!(status, StatusCode::OK, "{pairing}");
    let code = pairing["code"].as_str().expect("code").to_string();
    assert_eq!(code.len(), 43, "32 random bytes, base64url");
    let expires = pairing["expiresAt"].as_str().expect("expiresAt");
    assert!(expires.ends_with('Z'), "{expires}");

    let phone = Uuid::new_v4();
    let (status, session) = redeem(&state, &code, phone).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["email"], "ada@example.com");
    assert_eq!(session["deviceId"], json!(phone));
    let phone_token = session["token"].as_str().expect("token").to_string();
    assert_ne!(phone_token, token, "every device gets its own token");

    // The issued token really works, and the account lists both devices.
    let (status, account) = call(
        &state,
        Request::builder()
            .method("GET")
            .uri("/api/v1/account")
            .header("authorization", format!("Bearer {phone_token}"))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{account}");
    assert_eq!(account["devices"].as_array().expect("devices").len(), 2);

    // Single use: the same code is refused now.
    let (status, _) = redeem(&state, &code, Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_empty_and_expired_codes_are_refused() {
    let (status, _) = redeem(
        &state_with(ServerConfig::default()),
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        Uuid::new_v4(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "unknown code");

    let (status, _) = redeem(&state_with(ServerConfig::default()), "   ", Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty code");

    // A zero TTL makes the code expire before it can be used.
    let state = state_with(ServerConfig {
        pairing_ttl_secs: 0,
        ..ServerConfig::default()
    });
    let token = sign_up(&state, "ada@example.com", Uuid::new_v4()).await;
    let (status, pairing) = mint(&state, &token).await;
    assert_eq!(status, StatusCode::OK, "{pairing}");
    let code = pairing["code"].as_str().expect("code");
    let (status, _) = redeem(&state, code, Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "expired code");
}
