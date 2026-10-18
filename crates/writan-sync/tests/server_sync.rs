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
    Connected, FolderVault, ServerConnect, ServerConnection, SessionError, SyncKind, SyncReport,
    SyncSession, SyncTarget, connect_server, run_pass,
};

const EMAIL: &str = "ada@example.com";
const ACCOUNT_PASSWORD: &str = "account secret";
const SYNC_PASSWORD: &str = "shared secret";

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
    );
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

fn connect(
    url: &str,
    end: &End,
    device_name: &str,
    create_account: bool,
    sync_password: &str,
    account_password: &str,
) -> Result<Connected, SessionError> {
    connect_server(
        &ServerConnect {
            url: url.to_string(),
            email: EMAIL.to_string(),
            account_password: account_password.to_string(),
            device_name: device_name.to_string(),
            create_account,
        },
        sync_password,
        Some(&end.root),
        &end.state_path,
    )
}

/// Sign the connected device in again to get its token (the connect flow
/// keeps it in the keychain, which tests do not have).
fn token(url: &str, connected: &Connected, device_name: &str) -> String {
    writan_sync::server::login(&ServerLogin {
        url: url.to_string(),
        email: EMAIL.to_string(),
        password: ACCOUNT_PASSWORD.to_string(),
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
    let mut a_connected =
        connect(&url, &a, "laptop", true, SYNC_PASSWORD, ACCOUNT_PASSWORD).expect("connect a");
    assert_eq!(a_connected.config.kind, SyncKind::Server);
    assert!(a_connected.recovery_code.is_some());
    let vault_id = a_connected.session.vault_id;

    let target_a = target(&url, &token(&url, &a_connected, "laptop"), Some(vault_id));
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

    // Device B joins with the account password, discovers the vault from the
    // server (`vault_id: None` in its target) and adopts the note.
    let mut b_connected =
        connect(&url, &b, "phone", false, SYNC_PASSWORD, ACCOUNT_PASSWORD).expect("join b");
    assert!(b_connected.recovery_code.is_none(), "the keyring exists");
    assert_eq!(b_connected.session.vault_id, vault_id);
    let target_b = target(&url, &token(&url, &b_connected, "phone"), None);
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
fn joining_needs_the_right_passwords() {
    let root = tempfile::tempdir().expect("tempdir");
    let url = spawn_server(&root.path().join("data"));
    let a = End::new(root.path(), "a");
    let b = End::new(root.path(), "b");
    connect(&url, &a, "laptop", true, SYNC_PASSWORD, ACCOUNT_PASSWORD).expect("connect a");

    // The account password gates the sign-in.
    let wrong_account = connect(&url, &b, "phone", false, SYNC_PASSWORD, "nope");
    assert!(matches!(wrong_account, Err(SessionError::Server(_))));

    // The sync password gates the keyring: the account signs in, the join
    // still fails, and nothing is created for the half-joined device.
    let wrong_sync = connect(&url, &b, "phone", false, "wrong secret", ACCOUNT_PASSWORD);
    assert!(wrong_sync.is_err(), "a wrong sync password must not join");
}
