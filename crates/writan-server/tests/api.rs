//! End-to-end HTTP tests for the chunk A surface (ADR-0007 §§2–4), driven
//! through the real router with an in-memory database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;
use writan_server::auth::LockoutPolicy;
use writan_server::{AppState, ServerConfig, app, db};

fn state() -> AppState {
    state_with(ServerConfig::default())
}

fn state_with(config: ServerConfig) -> AppState {
    let conn = db::open(std::path::Path::new(":memory:")).expect("in-memory database");
    AppState::new(conn, config)
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
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn request(method: &str, path: &str, body: Option<Value>, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("request")
}

fn credentials(email: &str, password: &str, device_id: Uuid, device_name: &str) -> Value {
    json!({
        "email": email,
        "password": password,
        "deviceId": device_id,
        "deviceName": device_name,
    })
}

async fn create_account(
    state: &AppState,
    email: &str,
    password: &str,
    device_id: Uuid,
    device_name: &str,
) -> (StatusCode, Value) {
    call(
        state,
        request(
            "POST",
            "/api/v1/accounts",
            Some(credentials(email, password, device_id, device_name)),
            None,
        ),
    )
    .await
}

async fn sign_in(
    state: &AppState,
    email: &str,
    password: &str,
    device_id: Uuid,
    device_name: &str,
) -> (StatusCode, Value) {
    call(
        state,
        request(
            "POST",
            "/api/v1/sessions",
            Some(credentials(email, password, device_id, device_name)),
            None,
        ),
    )
    .await
}

fn token_of(body: &Value) -> String {
    body["token"].as_str().expect("token").to_string()
}

#[tokio::test]
async fn health_reports_status_and_schema() {
    let (status, body) = call(&state(), request("GET", "/health", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["schema"], db::SCHEMA_VERSION);
}

#[tokio::test]
async fn account_creation_signs_the_device_in() {
    let state = state();
    let device_id = Uuid::new_v4();
    let (status, body) = create_account(
        &state,
        "  Ada@Example.COM ",
        "correct horse",
        device_id,
        "laptop",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["email"], "ada@example.com", "the email is normalized");
    assert_eq!(body["deviceId"], device_id.to_string());
    let token = token_of(&body);

    let (status, body) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "ada@example.com");
    assert_eq!(body["devices"].as_array().expect("devices").len(), 1);
    assert_eq!(body["devices"][0]["name"], "laptop");
    assert_eq!(body["devices"][0]["id"], device_id.to_string());
    assert!(
        body["createdAt"]
            .as_str()
            .expect("createdAt")
            .ends_with('Z'),
        "RFC 3339: {body}"
    );
}

#[tokio::test]
async fn duplicate_email_is_refused_case_insensitively() {
    let state = state();
    let (first, _) = create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    assert_eq!(first, StatusCode::CREATED);
    let (second, _) = create_account(
        &state,
        "ADA@Example.COM",
        "another password",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(second, StatusCode::CONFLICT);
}

#[tokio::test]
async fn bad_email_and_short_password_are_refused() {
    let state = state();
    let (status, _) = create_account(
        &state,
        "not-an-email",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) =
        create_account(&state, "ada@example.com", "short", Uuid::new_v4(), "laptop").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_gives_each_device_its_own_token() {
    let state = state();
    let (_, first) = create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    let laptop_token = token_of(&first);

    let phone_id = Uuid::new_v4();
    let (status, phone) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        phone_id,
        "phone",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{phone}");
    let phone_token = token_of(&phone);
    assert_ne!(laptop_token, phone_token);

    let (status, body) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&phone_token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["devices"].as_array().expect("devices").len(), 2);

    // Unknown token and no token at all are both unauthorized.
    let (status, _) = call(
        &state,
        request("GET", "/api/v1/account", None, Some("nonsense")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&state, request("GET", "/api/v1/account", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_password_is_unauthorized() {
    let state = state();
    create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    let (status, body) = sign_in(
        &state,
        "ada@example.com",
        "wrong password",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    // An unknown email answers the same way, after the same kind of work.
    let (status, _) = sign_in(
        &state,
        "nobody@example.com",
        "wrong password",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn repeated_failures_lock_the_account() {
    let state = state_with(ServerConfig {
        lockout: LockoutPolicy {
            after: 2,
            base_secs: 2,
            cap_secs: 2,
        },
        ..ServerConfig::default()
    });
    create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;

    for expected in [StatusCode::UNAUTHORIZED, StatusCode::TOO_MANY_REQUESTS] {
        let (status, body) = sign_in(
            &state,
            "ada@example.com",
            "wrong password",
            Uuid::new_v4(),
            "phone",
        )
        .await;
        assert_eq!(status, expected, "{body}");
    }
    let (status, _) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "locked even with the right password"
    );

    std::thread::sleep(std::time::Duration::from_millis(2_100));
    let (status, body) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the lock expires: {body}");
}

#[tokio::test]
async fn logout_revokes_only_the_calling_device() {
    let state = state();
    let (_, laptop) = create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    let laptop_token = token_of(&laptop);
    let (_, phone) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    let phone_token = token_of(&phone);

    let (status, _) = call(
        &state,
        request("DELETE", "/api/v1/sessions", None, Some(&phone_token)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&phone_token)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&laptop_token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["devices"].as_array().expect("devices").len(), 1);
}

#[tokio::test]
async fn closed_registration_refuses_new_accounts() {
    let state = state_with(ServerConfig {
        registration_open: false,
        ..ServerConfig::default()
    });
    let (status, _) = create_account(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "phone",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no account exists");
}

#[tokio::test]
async fn re_registering_a_device_replaces_its_token() {
    let state = state();
    let device_id = Uuid::new_v4();
    let (_, first) = create_account(
        &state,
        "ada@example.com",
        "correct horse",
        device_id,
        "laptop",
    )
    .await;
    let old_token = token_of(&first);

    let (status, second) = sign_in(
        &state,
        "ada@example.com",
        "correct horse",
        device_id,
        "laptop",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let new_token = token_of(&second);
    assert_ne!(old_token, new_token);

    let (status, body) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&new_token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["devices"].as_array().expect("devices").len(),
        1,
        "the device is replaced, not duplicated"
    );
    let (status, _) = call(
        &state,
        request("GET", "/api/v1/account", None, Some(&old_token)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old token is dead");
}
