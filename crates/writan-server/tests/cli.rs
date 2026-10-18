//! The binary itself (ADR-0007 §10): command line, the admin
//! `reset-password`, and a real listen-and-answer smoke of `serve`.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use writan_server::{auth, db};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_writan-server"))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn run_in(dir: &Path, args: &[&str], password: Option<&str>) -> Output {
    let mut command = binary();
    command.args(args).env("WRITAN_DATA_DIR", dir);
    if let Some(password) = password {
        command.env("WRITAN_NEW_PASSWORD", password);
    }
    command.output().expect("run the binary")
}

/// Kill the child even when an assertion below fails.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn version_and_help_answer() {
    let output = binary().arg("--version").output().expect("run");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("writan-server {}", env!("CARGO_PKG_VERSION"))
    );

    let output = binary().arg("--help").output().expect("run");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("reset-password"), "{help}");
    assert!(help.contains("WRITAN_DATA_DIR"), "{help}");

    let output = binary().arg("frobnicate").output().expect("run");
    assert!(!output.status.success());
    assert!(stderr_of(&output).contains("unknown command `frobnicate`"));
}

#[test]
fn reset_password_swaps_the_hash_and_revokes_devices() {
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("writan-server.sqlite");
    let conn = db::open(&database).expect("database");
    let (salt, hash) = auth::hash_password("old password").expect("hash");
    let account = db::create_account(&conn, "ada@example.com", &salt, &hash, 100).expect("create");
    db::upsert_device(
        &conn,
        account.id,
        uuid::Uuid::new_v4(),
        "laptop",
        &[1; 32],
        100,
    )
    .expect("device");
    drop(conn);

    let missing = run_in(
        dir.path(),
        &["reset-password", "nobody@example.com"],
        Some("new password"),
    );
    assert!(!missing.status.success());
    assert!(stderr_of(&missing).contains("no account for nobody@example.com"));

    let output = run_in(
        dir.path(),
        &["reset-password", "  Ada@Example.COM "],
        Some("new password"),
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert!(stderr_of(&output).contains("password updated for ada@example.com"));

    let conn = db::open(&database).expect("reopen");
    let found = db::account_by_email(&conn, "ada@example.com")
        .expect("lookup")
        .expect("found");
    assert!(auth::verify_password(
        "new password",
        &found.salt,
        &found.hash
    ));
    assert!(!auth::verify_password(
        "old password",
        &found.salt,
        &found.hash
    ));
    assert!(
        db::list_devices(&conn, account.id)
            .expect("devices")
            .is_empty(),
        "the reset revokes every device"
    );
}

#[test]
fn serve_listens_and_answers_health() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A free port: bind, read it, release it, then hand it to the server.
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("probe")
        .local_addr()
        .expect("address")
        .port();

    let child = binary()
        .env("WRITAN_DATA_DIR", dir.path())
        .env("WRITAN_PORT", port.to_string())
        .spawn()
        .expect("spawn the server");
    let _guard = Guard(child);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the server never accepted a connection: {error}"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("\"status\":\"ok\""),
        "health body: {response}"
    );
}
