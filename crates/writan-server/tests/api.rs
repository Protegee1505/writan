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

async fn call_raw(state: &AppState, request: Request<Body>) -> (StatusCode, Vec<u8>) {
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
    (status, bytes)
}

async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let (status, bytes) = call_raw(state, request).await;
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

/// Like [`request`] but with an opaque body and the blob content type.
fn blob_request(
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
    token: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/octet-stream");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(body.map_or_else(Body::empty, Body::from))
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

/// A signed-in account with one registered vault.
async fn sign_up_and_register_vault(state: &AppState) -> (Uuid, String) {
    let (status, account) = create_account(
        state,
        "ada@example.com",
        "correct horse",
        Uuid::new_v4(),
        "laptop",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{account}");
    let token = token_of(&account);
    let vault_id = Uuid::new_v4();
    let (status, body) = call(
        state,
        request(
            "PUT",
            &format!("/api/v1/vaults/{vault_id}"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (vault_id, token)
}

#[tokio::test]
async fn vault_registration_is_idempotent_and_private() {
    let state = state();
    let (vault_id, token) = sign_up_and_register_vault(&state).await;

    let (status, body) = call(
        &state,
        request(
            "PUT",
            &format!("/api/v1/vaults/{vault_id}"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "re-registering is idempotent: {body}"
    );
    assert_eq!(body["id"], vault_id.to_string());
    assert!(
        body["createdAt"]
            .as_str()
            .expect("createdAt")
            .ends_with('Z')
    );

    let (status, body) = call(&state, request("GET", "/api/v1/vaults", None, Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["vaults"].as_array().expect("vaults").len(), 1);
    assert_eq!(body["vaults"][0]["id"], vault_id.to_string());

    // Another account can neither claim nor see it.
    let (_, other) = create_account(
        &state,
        "bob@example.com",
        "correct horse",
        Uuid::new_v4(),
        "desktop",
    )
    .await;
    let bob = token_of(&other);
    let (status, _) = call(
        &state,
        request(
            "PUT",
            &format!("/api/v1/vaults/{vault_id}"),
            None,
            Some(&bob),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(&state, request("GET", "/api/v1/vaults", None, Some(&bob))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["vaults"].as_array().expect("vaults").is_empty());
    let (status, _) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/changes"),
            None,
            Some(&bob),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no probing foreign vaults");
}

#[tokio::test]
async fn blob_writes_are_create_only() {
    let state = state();
    let (vault_id, token) = sign_up_and_register_vault(&state).await;
    let blob_path = format!("items/{}/1.enc", Uuid::new_v4());
    let blob = format!("/api/v1/vaults/{vault_id}/blobs/{blob_path}");

    let (status, _) = call_raw(
        &state,
        blob_request("PUT", &blob, Some(b"ciphertext".to_vec()), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call_raw(
        &state,
        blob_request("PUT", &blob, Some(b"ciphertext".to_vec()), Some(&token)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "an identical retry is accepted"
    );
    let (status, body) = call(
        &state,
        blob_request("PUT", &blob, Some(b"different".to_vec()), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let response = app(state.clone())
        .oneshot(blob_request("GET", &blob, None, Some(&token)))
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(&bytes[..], b"ciphertext");

    let (status, body) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/blobs"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["paths"].as_array().expect("paths").len(), 1);
    assert_eq!(body["paths"][0].as_str().expect("path"), blob_path);

    let (status, _) = call_raw(
        &state,
        blob_request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/blobs/keys/1.json"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_raw(&state, blob_request("GET", &blob, None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unsafe_blob_paths_are_refused() {
    let state = state();
    let (vault_id, token) = sign_up_and_register_vault(&state).await;
    for bad in ["a/../b", "a//b", "a/./b", &"x".repeat(600)] {
        let path = format!("/api/v1/vaults/{vault_id}/blobs/{bad}");
        let (status, body) = call(
            &state,
            blob_request("PUT", &path, Some(b"x".to_vec()), Some(&token)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    // A signal-clean path still works after the refusals.
    let (status, _) = call_raw(
        &state,
        blob_request(
            "PUT",
            &format!("/api/v1/vaults/{vault_id}/blobs/keys/1.json"),
            Some(b"{}".to_vec()),
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn blob_size_and_quota_limits_are_enforced() {
    let state = state_with(ServerConfig {
        max_blob_bytes: 16,
        vault_quota_bytes: 24,
        ..ServerConfig::default()
    });
    let (vault_id, token) = sign_up_and_register_vault(&state).await;
    let blob = |name: &str| format!("/api/v1/vaults/{vault_id}/blobs/{name}");

    let (status, body) = call(
        &state,
        blob_request("PUT", &blob("too-big"), Some(vec![b'x'; 17]), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");

    let (status, _) = call_raw(
        &state,
        blob_request("PUT", &blob("a"), Some(vec![b'x'; 16]), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &state,
        blob_request("PUT", &blob("b"), Some(vec![b'y'; 16]), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{body}");
    let (status, _) = call_raw(
        &state,
        blob_request("PUT", &blob("a"), Some(vec![b'x'; 16]), Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "retries ignore the quota");
}

#[tokio::test]
async fn change_feed_pages_by_sequence() {
    let state = state();
    let (vault_id, token) = sign_up_and_register_vault(&state).await;
    for (index, name) in ["keys/1.json", "items/a", "items/b"].iter().enumerate() {
        let path = format!("/api/v1/vaults/{vault_id}/blobs/{name}");
        let (status, _) = call_raw(
            &state,
            blob_request(
                "PUT",
                &path,
                Some(format!("body {index}").into_bytes()),
                Some(&token),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/changes"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[0]["path"], "keys/1.json");
    assert_eq!(events[0]["size"], 6);
    assert_eq!(events[1]["seq"], 2);
    assert_eq!(events[2]["path"], "items/b");
    assert!(
        events[0]["at"].as_str().expect("at").ends_with('Z'),
        "RFC 3339: {body}"
    );
    assert_eq!(body["next"], 3);

    let (status, body) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/changes?since=3"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["events"].as_array().expect("events").is_empty());
    assert_eq!(body["next"], 3, "the cursor holds when nothing is new");

    let (status, body) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/changes?since=0&limit=2"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().expect("events").len(), 2);
    assert_eq!(body["next"], 2);

    let (status, body) = call(
        &state,
        request(
            "GET",
            &format!("/api/v1/vaults/{vault_id}/changes?since=2"),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().expect("events").len(), 1);
}
