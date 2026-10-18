//! Optional self-hosted Writan sync server (ADR-0007).
//!
//! The server is a dumb, untrusted blob store with accounts: it persists the
//! encrypted blob tree from ADR-0005 and a change feed, and never sees keys
//! or plaintext. Accounts and bearer tokens guard *access*; the E2EE key
//! hierarchy stays client-side. Configuration is environment-only; the
//! binary adds `serve` and an admin `reset-password` command.
//!
//! Milestone M4 lands in chunks (ADR-0007 §12): accounts, device sessions,
//! vaults, the blob API, the change feed and its SSE stream, and pairing are
//! in; the Docker image and deploy doc follow.

#![forbid(unsafe_code)]

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod routes;
pub mod util;

#[cfg(test)]
pub(crate) mod test_support;

pub use config::ServerConfig;
pub use error::ApiError;
pub use routes::{AppState, app};

/// Serve `state` on an already-bound `listener` until the process ends.
///
/// The caller binds the listener (so a test can pick an ephemeral port);
/// this function switches it to non-blocking mode, builds the runtime and
/// waits forever on the answer.
pub fn serve_blocking(
    listener: std::net::TcpListener,
    state: AppState,
) -> Result<(), std::io::Error> {
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, app(state)).await
    })
}
