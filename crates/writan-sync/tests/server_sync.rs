//! Two devices, one writan-server: the engine's storage rules over HTTP.
//!
//! The real server runs in a thread (the same `writan_server::serve_blocking`
//! the `serve` command calls), and the clients go through the same
//! `connect_server` flow the shells use: register or join, discover the
//! vault, exchange the keyring, then plain sync passes. The ciphertext blobs
//! travel the ADR-0007 API; the vault key never does.

use std::path::{Path, PathBuf};

use writan_core::vault::{Note, Vault};
use writan_sync::server::{ServerHttpTransport, ServerLogin, ServerTarget};
use writan_sync::{
    Connected, FolderVault, KeyringPurpose, ServerConnect, ServerConnection, ServerPairing,
    SessionError, SyncKind, SyncReport, SyncSession, SyncTarget, connect_server, create_pairing,
    join_with_code, keyring_generations, pair_payload, parse_pair_payload, push_generation,
    rewrap_generation, run_pass, unlock_keyring,
};

const EMAIL: &str = "ada@example.com";
/// One password signs the device in and opens the keyring (ADR-0008).
const PASSWORD: &str = "shared secret";
const NEW_PASSWORD: &str = "brand new secret";

/// Start the real server on an ephemeral port; the thread lives until the
/// test process ends.
fn spawn_server(data_dir: &Path) -> String {
    std::fs::create_dir_all(data_dir).expect("data dir");
    let conn = writan_server::db::open(&data_dir.join("writan-server.sqlite")).expect("db");
    let state = writan_server::AppState::new(
        conn,
        writan_server::ServerConfig {
            data_dir: data_dir.to_path_buf(),
            ..writan_server::ServerConfig::default()
        },
    )
    .expect("state");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        if let Err(error) = writan_server::serve_blocking(listener, state) {
            eprintln!("test server stopped: {error}");
        }
    });
    format!("http://{addr}")
}

/// One device: a vault plus the sync-state file `connect_server` writes.
struct End {
    vault: Vault,
    root: PathBuf,
    state_path: PathBuf,
}

impl End {
    fn new(dir: &Path, name: &str) -> Self {
        let root = dir.join(name);
        let vault = Vault::open(&root).expect("vault");
        End {
            vault,
            root,
            state_path: dir.join(format!("{name}.state.json")),
        }
    }

    fn sync(
        &self,
        session: &mut SyncSession,
        target: &ServerTarget<ServerHttpTransport>,
    ) -> SyncReport {
        let io = FolderVault::new(&self.vault);
        run_pass(&self.state_path, session, &io, target).expect("sync pass")
    }

    fn notes(&self) -> Vec<Note> {
        self.vault
            .walk_notes()
            .expect("walk")
            .iter()
            .map(|rel| self.vault.read(rel).expect("read"))
            .collect()
    }

    fn note(&self, title: &str) -> Option<Note> {
        self.notes().into_iter().find(|n| n.meta.title == title)
    }

    fn body_of(&self, title: &str) -> Option<String> {
        self.note(title).map(|n| n.body)
    }

    fn edit(&self, title: &str, body: &str, updated: &str) {
        let mut note = self.note(title).expect("note to edit");
        note.body = body.to_string();
        note.meta.updated = Some(updated.to_string());
        self.vault.write(&note).expect("write");
    }
}

/// One device connecting with the one password; no fallback credential (the
/// fresh-device flow ADR-0008 is about).
fn connect(
    url: &str,
    end: &End,
    device_name: &str,
    create_account: bool,
    password: &str,
) -> Result<Connected, SessionError> {
    connect_server(
        &ServerConnect {
            url: url.to_string(),
            email: EMAIL.to_string(),
            device_name: device_name.to_string(),
            create_account,
        },
        password,
        None,
        Some(&end.root),
        &end.state_path,
    )
}

/// Sign the connected device in again to get its token (the connect flow
/// keeps it in the keychain, which tests do not have).
fn token(url: &str, connected: &Connected, device_name: &str, password: &str) -> String {
    writan_sync::server::login(&ServerLogin {
        url: url.to_string(),
        email: EMAIL.to_string(),
        password: password.to_string(),
        device_id: connected.session.device,
        device_name: device_name.to_string(),
    })
    .expect("login")
}

fn target(
    url: &str,
    token: &str,
    vault_id: Option<uuid::Uuid>,
) -> ServerTarget<ServerHttpTransport> {
    ServerTarget::new(ServerHttpTransport::new(ServerConnection {
        base_url: url.to_string(),
        token: token.to_string(),
        vault_id,
    }))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn two_devices_converge_through_a_writan_server() {
    let root = tempfile::tempdir().expect("tempdir");
    let url = spawn_server(&root.path().join("data"));
    let a = End::new(root.path(), "a");
    let b = End::new(root.path(), "b");

    // Device A registers the account and owns a fresh vault: the recovery
    // code comes back exactly once.
    let mut a_connected = connect(&url, &a, "laptop", true, PASSWORD).expect("connect a");
    assert_eq!(a_connected.config.kind, SyncKind::Server);
    assert!(a_connected.recovery_code.is_some());
    let vault_id = a_connected.session.vault_id;

    let target_a = target(
        &url,
        &token(&url, &a_connected, "laptop", PASSWORD),
        Some(vault_id),
    );
    a.vault.create("Alpha", &[]).expect("create");
    a.edit("Alpha", "from a\n", "2026-03-01T10:00:00Z");
    let report = a.sync(&mut a_connected.session, &target_a);
    assert_eq!(report.published, 1);
    assert_eq!(
        report.unreadable, 0,
        "the keyring blob is ignored, not read"
    );

    // What the server holds: the keyring and one sealed item — no plaintext.
    let paths = SyncTarget::list(&target_a).expect("list");
    assert!(paths.iter().any(|path| path.starts_with("keys/")));
    let item = paths
        .iter()
        .find(|path| path.starts_with("items/"))
        .expect("item blob")
        .clone();
    let bytes = SyncTarget::read(&target_a, &item).expect("read blob");
    assert!(
        !contains(&bytes, b"Alpha") && !contains(&bytes, b"from a"),
        "note text never reaches the server in the clear"
    );

    // Device B — a fresh machine — joins with the address, the email and the
    // same password: one credential signs in and opens the keyring. It
    // discovers the vault from the server (`vault_id: None` in its target)
    // and adopts the note.
    let mut b_connected = connect(&url, &b, "phone", false, PASSWORD).expect("join b");
    assert!(b_connected.recovery_code.is_none(), "the keyring exists");
    assert_eq!(b_connected.session.vault_id, vault_id);
    let target_b = target(&url, &token(&url, &b_connected, "phone", PASSWORD), None);
    assert_eq!(b.sync(&mut b_connected.session, &target_b).adopted, 1);
    assert_eq!(b.body_of("Alpha").as_deref(), Some("from a\n"));

    // B writes its own note; A adopts it on its next pass.
    b.vault.create("Beta", &[]).expect("create");
    b.edit("Beta", "from b\n", "2026-03-02T10:00:00Z");
    assert_eq!(b.sync(&mut b_connected.session, &target_b).published, 1);
    assert_eq!(a.sync(&mut a_connected.session, &target_a).adopted, 1);
    assert_eq!(a.body_of("Beta").as_deref(), Some("from b\n"));

    // Converged: neither side has anything left to do.
    assert_eq!(a.sync(&mut a_connected.session, &target_a).unchanged, 2);
    assert_eq!(b.sync(&mut b_connected.session, &target_b).unchanged, 2);
}

#[test]
fn joining_needs_the_right_password() {
    let root = tempfile::tempdir().expect("tempdir");
    let url = spawn_server(&root.path().join("data"));
    let a = End::new(root.path(), "a");
    let b = End::new(root.path(), "b");
    connect(&url, &a, "laptop", true, PASSWORD).expect("connect a");

    // The wrong password does not sign in — and so never reaches the keyring.
    let wrong = connect(&url, &b, "phone", false, "not the password");
    assert!(matches!(wrong, Err(SessionError::Server(_))));

    // The right one joins.
    let joined = connect(&url, &b, "phone", false, PASSWORD).expect("join b");
    assert!(joined.recovery_code.is_none());
}

#[test]
fn joining_by_pairing_code_skips_the_sign_in() {
    let root = tempfile::tempdir().expect("tempdir");
    let url = spawn_server(&root.path().join("data"));
    let a = End::new(root.path(), "a");
    let b = End::new(root.path(), "b");

    // A registers, publishes a note, and mints a pairing code.
    let mut a_connected = connect(&url, &a, "laptop", true, PASSWORD).expect("connect a");
    let vault_id = a_connected.session.vault_id;
    let a_token = token(&url, &a_connected, "laptop", PASSWORD);
    let target_a = target(&url, &a_token, Some(vault_id));
    a.vault.create("Alpha", &[]).expect("create");
    a.edit("Alpha", "from a\n", "2026-03-01T10:00:00Z");
    assert_eq!(a.sync(&mut a_connected.session, &target_a).published, 1);

    // The QR payload carries everything the joining device needs.
    let pairing = create_pairing(&url, &a_token).expect("pairing");
    let payload = pair_payload(&url, &pairing.code, EMAIL);
    let parsed = parse_pair_payload(&payload).expect("payload");
    let details = |device_name: &str| ServerPairing {
        url: parsed.url.clone(),
        code: parsed.code.clone(),
        email: parsed.email.clone(),
        device_name: device_name.to_string(),
    };

    // B joins with the code alone — no sign-in — and adopts A's note. (The
    // test signs B in again below only because it has no OS keychain to
    // recall the paired token from.)
    let mut b_connected = join_with_code(
        &details("phone"),
        PASSWORD,
        None,
        Some(&b.root),
        &b.state_path,
    )
    .expect("join by code");
    assert_eq!(b_connected.session.vault_id, vault_id);
    assert!(b_connected.recovery_code.is_none(), "the keyring exists");
    let target_b = target(&url, &token(&url, &b_connected, "phone", PASSWORD), None);
    assert_eq!(b.sync(&mut b_connected.session, &target_b).adopted, 1);
    assert_eq!(b.body_of("Alpha").as_deref(), Some("from a\n"));

    // The code is single use: a third device trades it in vain.
    let c = End::new(root.path(), "c");
    let consumed = join_with_code(
        &details("tablet"),
        PASSWORD,
        None,
        Some(&c.root),
        &c.state_path,
    );
    assert!(consumed.is_err(), "a used code must not join again");

    // And a code grants access, not the key: with a fresh code, the wrong
    // password still refuses the join.
    let fresh = create_pairing(&url, &a_token).expect("pairing");
    let parsed = parse_pair_payload(&pair_payload(&url, &fresh.code, EMAIL)).expect("payload");
    let details = ServerPairing {
        url: parsed.url.clone(),
        code: parsed.code.clone(),
        email: parsed.email.clone(),
        device_name: "tablet".to_string(),
    };
    let wrong_key = join_with_code(&details, "wrong secret", None, Some(&c.root), &c.state_path);
    assert!(
        wrong_key.is_err(),
        "the vault key never travels with a code"
    );
}

#[test]
fn changing_the_password_keeps_the_vault_and_retires_the_old_one() {
    let root = tempfile::tempdir().expect("tempdir");
    let url = spawn_server(&root.path().join("data"));
    let a = End::new(root.path(), "a");
    let b = End::new(root.path(), "b");
    let c = End::new(root.path(), "c");

    // A registers, publishes a note, and B joins with the one password.
    let mut a_connected = connect(&url, &a, "laptop", true, PASSWORD).expect("connect a");
    let vault_id = a_connected.session.vault_id;
    let a_token = token(&url, &a_connected, "laptop", PASSWORD);
    let target_a = target(&url, &a_token, Some(vault_id));
    a.vault.create("Alpha", &[]).expect("create");
    a.edit("Alpha", "from a\n", "2026-03-01T10:00:00Z");
    assert_eq!(a.sync(&mut a_connected.session, &target_a).published, 1);
    let mut b_connected = connect(&url, &b, "phone", false, PASSWORD).expect("join b");
    let target_b = target(&url, &token(&url, &b_connected, "phone", PASSWORD), None);
    assert_eq!(b.sync(&mut b_connected.session, &target_b).adopted, 1);

    // Change the one password. The keyring first (ADR-0008 §6): a new
    // generation the new password opens, wrapping the same vault key — the
    // two steps `writan_sync::change_password` performs, minus the OS
    // keychain this test does not have.
    let generations = keyring_generations(&target_a).expect("generations");
    let unlocked = unlock_keyring(
        &generations,
        KeyringPurpose::Password,
        PASSWORD,
        Some(vault_id),
    )
    .expect("the current password opens the keyring");
    let generation = generations
        .iter()
        .map(|(number, _)| *number)
        .max()
        .unwrap_or(0)
        + 1;
    let file = rewrap_generation(
        vault_id,
        &unlocked.vault_key,
        generation,
        KeyringPurpose::Password,
        NEW_PASSWORD,
    )
    .expect("rewrap");
    push_generation(&target_a, &file).expect("push");
    // Then the account: the server learns the new verifier.
    writan_sync::server::change_verifier(&url, &a_token, PASSWORD, NEW_PASSWORD).expect("change");

    // Changing the password signs the other devices out (ADR-0008 §6): B's
    // token is refused…
    let io = FolderVault::new(&b.vault);
    let resigned = run_pass(&b.state_path, &mut b_connected.session, &io, &target_b);
    assert!(resigned.is_err(), "the other devices must sign in again");

    // …and the new password brings B back, keyring and all, while A keeps
    // syncing on the token it changed the password with.
    let mut b_again = connect(&url, &b, "phone", false, NEW_PASSWORD).expect("sign in again");
    let target_b = target(&url, &token(&url, &b_again, "phone", NEW_PASSWORD), None);
    b.vault.create("Beta", &[]).expect("create");
    b.edit("Beta", "from b\n", "2026-03-02T10:00:00Z");
    assert_eq!(b.sync(&mut b_again.session, &target_b).published, 1);
    assert_eq!(a.sync(&mut a_connected.session, &target_a).adopted, 1);
    assert_eq!(a.body_of("Beta").as_deref(), Some("from b\n"));

    // A third device signs in with the new password and gets both notes.
    let mut c_connected = connect(&url, &c, "tablet", false, NEW_PASSWORD).expect("join c");
    assert_eq!(c_connected.session.vault_id, vault_id);
    let target_c = target(
        &url,
        &token(&url, &c_connected, "tablet", NEW_PASSWORD),
        None,
    );
    assert_eq!(c.sync(&mut c_connected.session, &target_c).adopted, 2);

    // The old password neither signs in nor opens the keyring.
    let stale = connect(&url, &End::new(root.path(), "d"), "spare", false, PASSWORD);
    assert!(matches!(stale, Err(SessionError::Server(_))));
}
