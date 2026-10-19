//! The writan-server binary: `serve` (default) and the admin commands
//! (ADR-0007 §§2, 10).

use std::process::ExitCode;

use writan_server::config::ServerConfig;
use writan_server::routes::AppState;
use writan_server::{auth, db};

const USAGE: &str = "\
writan-server — optional Writan sync server (ADR-0007)

Usage:
  writan-server [serve]              start the HTTP server (default)
  writan-server reset-password EMAIL set a new password for EMAIL and revoke
                                     every device of that account; also the
                                     migration for an account that predates
                                     schema v2. The new password comes from
                                     WRITAN_NEW_PASSWORD or, when unset, from
                                     stdin
  writan-server --version | -V       print the version
  writan-server --help | -h          print this help

Environment:
  WRITAN_BIND              address to listen on (default 0.0.0.0; set
                           127.0.0.1 when a reverse proxy terminates TLS)
  WRITAN_PORT              port to listen on (default 8787)
  WRITAN_DATA_DIR          directory holding writan-server.sqlite
                           (default `writan-data`)
  WRITAN_REGISTRATION      `open` (default) or `closed`
  WRITAN_MAX_BLOB_KIB      largest accepted blob (default 4096)
  WRITAN_VAULT_QUOTA_MIB   per-vault quota, 0 = unlimited (default 512)
  WRITAN_PAIRING_TTL_SECS  pairing-code lifetime (default 600)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => report(serve()),
        Some("serve") => {
            if args.len() > 1 {
                eprintln!("writan-server: ignoring extra arguments {:?}", &args[1..]);
            }
            report(serve())
        }
        Some("reset-password") => match args.get(1) {
            Some(email) if args.len() == 2 => report(reset_password(email)),
            _ => fail("usage: writan-server reset-password EMAIL"),
        },
        Some("--version" | "-V") => {
            println!("writan-server {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(&format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

fn report(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("writan-server: {message}");
    ExitCode::FAILURE
}

/// Any library error, as a startup/admin failure message.
fn reason(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Read the configuration, open the database and serve until interrupted.
fn serve() -> Result<(), String> {
    let config = ServerConfig::from_env()?;
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|error| format!("cannot create {}: {error}", config.data_dir.display()))?;
    let conn = db::open(&database_path(&config)).map_err(reason)?;
    let state = AppState::new(conn, config.clone()).map_err(reason)?;
    let listener = std::net::TcpListener::bind((config.bind, config.port))
        .map_err(|error| format!("cannot bind {}:{}: {error}", config.bind, config.port))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("cannot read the local address: {error}"))?;
    eprintln!(
        "writan-server {} listening on http://{address} (data: {})",
        env!("CARGO_PKG_VERSION"),
        config.data_dir.display()
    );
    writan_server::serve_blocking(listener, state).map_err(|error| format!("server: {error}"))
}

/// Replace an account's verifier material and revoke its devices
/// (ADR-0007 §2, ADR-0008 §4/§6). The new password is derived into a verifier
/// exactly as a client would, so a legacy row is migrated by the same run.
fn reset_password(email: &str) -> Result<(), String> {
    let config = ServerConfig::from_env()?;
    let email = auth::normalize_email(email).map_err(reason)?;
    let password = new_password()?;
    auth::validate_password(&password).map_err(reason)?;
    let salt = writan_crypto::random_salt();
    let verifier = writan_crypto::derive_kek(&password, &salt).map_err(reason)?;
    let mut conn = db::open(&database_path(&config)).map_err(reason)?;
    let Some(found) = db::account_by_email(&conn, &email).map_err(reason)? else {
        return Err(format!("no account for {email}"));
    };
    if !db::replace_verifier(
        &mut conn,
        found.account.id,
        &salt,
        &auth::hash_verifier(&verifier),
        None,
    )
    .map_err(reason)?
    {
        return Err(format!("no account for {email}"));
    }
    eprintln!("password updated for {email}; all its devices were revoked");
    Ok(())
}

fn new_password() -> Result<String, String> {
    if let Ok(password) = std::env::var("WRITAN_NEW_PASSWORD") {
        return Ok(password);
    }
    eprintln!(
        "writan-server: reading the new password from stdin \
         (it will be visible in the terminal)"
    );
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("cannot read the password: {error}"))?;
    let password = line.trim_end_matches(['\n', '\r']).to_string();
    if password.is_empty() {
        return Err("empty password".to_string());
    }
    Ok(password)
}

fn database_path(config: &ServerConfig) -> std::path::PathBuf {
    config.data_dir.join("writan-server.sqlite")
}
