//! The SSE change stream over a real socket (ADR-0007 §7): backlog replay
//! from a cursor, live fan-out while connected, `Last-Event-ID` resume, and
//! the same 401 as the REST feed.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use uuid::Uuid;

/// `Argon2id(password, salt)`, base64 — what a client sends (ADR-0008 §2).
fn verifier_for(password: &str, salt: &str) -> String {
    let salt = writan_server::auth::parse_salt(salt).expect("salt is valid");
    let verifier = writan_crypto::derive_kek(password, &salt).expect("kdf");
    base64::engine::general_purpose::STANDARD.encode(verifier)
}

/// Kill the child even when an assertion below fails.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One request on its own connection, read to the end (`Connection: close`).
fn call(port: u16, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> String {
    let mut raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        raw.push_str("Content-Type: application/json\r\n");
    }
    if let Some(token) = token {
        raw.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    raw.push_str("\r\n");
    let mut bytes = raw.into_bytes();
    bytes.extend_from_slice(body);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("timeout");
    stream.write_all(&bytes).expect("request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response");
    response
}

fn body_of(response: &str) -> Value {
    let (_, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no headers in: {response}"));
    serde_json::from_str(body).unwrap_or_else(|error| panic!("no JSON body ({error}): {response}"))
}

/// Open the event stream and keep the connection (no `Connection: close`).
fn open_stream(port: u16, vault: Uuid, token: &str, last_event_id: Option<i64>) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .expect("timeout");
    let mut request = format!(
        "GET /api/v1/vaults/{vault}/events HTTP/1.1\r\nHost: localhost\r\n\
         Accept: text/event-stream\r\nAuthorization: Bearer {token}\r\n"
    );
    if let Some(id) = last_event_id {
        request.push_str(&format!("Last-Event-ID: {id}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("subscribe");
    stream
}

/// Read until `needle` appears in the text, or fail after 15 s. The chunked
/// transfer framing stays in the text — the payload is still readable.
fn read_until(stream: &mut TcpStream, needle: &str) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("read: {error}"),
        }
        let text = String::from_utf8_lossy(&buffer).into_owned();
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "never saw {needle:?} in:\n{text}"
        );
    }
}

#[test]
fn the_event_stream_replays_and_follows_live_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A free port: bind, read it, release it, then hand it to the server.
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("probe")
        .local_addr()
        .expect("address")
        .port();
    let child = Command::new(env!("CARGO_BIN_EXE_writan-server"))
        .env("WRITAN_DATA_DIR", dir.path())
        .env("WRITAN_PORT", port.to_string())
        .spawn()
        .expect("spawn the server");
    let _guard = Guard(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => break,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the server never accepted a connection: {error}"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    let device = Uuid::new_v4();
    let email = "ada@example.com";
    let salt_response = call(
        port,
        "GET",
        &format!("/api/v1/accounts/salt?email={email}"),
        None,
        b"",
    );
    assert!(salt_response.starts_with("HTTP/1.1 200"), "{salt_response}");
    let salt = body_of(&salt_response)["salt"]
        .as_str()
        .expect("salt")
        .to_string();
    let account = call(
        port,
        "POST",
        "/api/v1/accounts",
        None,
        json!({
            "email": email,
            "salt": salt,
            "verifier": verifier_for("correct horse", &salt),
            "deviceId": device,
            "deviceName": "laptop",
        })
        .to_string()
        .as_bytes(),
    );
    assert!(account.starts_with("HTTP/1.1 201"), "{account}");
    let token = body_of(&account)["token"]
        .as_str()
        .expect("token")
        .to_string();

    let vault = Uuid::new_v4();
    let created = call(
        port,
        "PUT",
        &format!("/api/v1/vaults/{vault}"),
        Some(&token),
        b"",
    );
    assert!(created.starts_with("HTTP/1.1 201"), "{created}");
    let first = call(
        port,
        "PUT",
        &format!("/api/v1/vaults/{vault}/blobs/keys/1.json"),
        Some(&token),
        br#"{"gen":1}"#,
    );
    assert!(first.starts_with("HTTP/1.1 201"), "{first}");

    // Backlog: a fresh subscription replays what happened before it dialed in.
    let mut stream = open_stream(port, vault, &token, None);
    let text = read_until(&mut stream, "\"seq\":1");
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(text.contains("text/event-stream"), "{text}");
    assert!(text.contains("id: 1"), "{text}");
    assert!(text.contains("\"path\":\"keys/1.json\""), "{text}");

    // Live: a write on another connection arrives without polling.
    let second = call(
        port,
        "PUT",
        &format!("/api/v1/vaults/{vault}/blobs/items/{device}/1.enc"),
        Some(&token),
        b"note one",
    );
    assert!(second.starts_with("HTTP/1.1 201"), "{second}");
    let text = read_until(&mut stream, "id: 2");
    assert!(text.contains("\"seq\":2"), "{text}");
    drop(stream);

    // Resume: Last-Event-ID picks the feed up exactly after the cursor.
    let mut stream = open_stream(port, vault, &token, Some(1));
    let text = read_until(&mut stream, "id: 2");
    assert!(
        !text.contains("id: 1"),
        "nothing before the cursor is replayed:\n{text}"
    );
    let third = call(
        port,
        "PUT",
        &format!("/api/v1/vaults/{vault}/blobs/items/{device}/2.enc"),
        Some(&token),
        b"note two",
    );
    assert!(third.starts_with("HTTP/1.1 201"), "{third}");
    let text = read_until(&mut stream, "id: 3");
    assert!(text.contains("\"seq\":3"), "{text}");
    drop(stream);

    // The same auth and path discipline as the REST surface.
    let denied = call(
        port,
        "GET",
        &format!("/api/v1/vaults/{vault}/events"),
        None,
        b"",
    );
    assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
    let bad_cursor = format!(
        "GET /api/v1/vaults/{vault}/events HTTP/1.1\r\nHost: localhost\r\n\
         Authorization: Bearer {token}\r\nLast-Event-ID: soon\r\nConnection: close\r\n\r\n"
    );
    let mut bad = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    bad.set_read_timeout(Some(Duration::from_secs(15)))
        .expect("timeout");
    bad.write_all(bad_cursor.as_bytes()).expect("request");
    let mut response = String::new();
    bad.read_to_string(&mut response).expect("response");
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
}
