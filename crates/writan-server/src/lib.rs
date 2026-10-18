//! Optional self-hosted Writan sync server (ADR-0007).
//!
//! The server is a dumb, untrusted blob store with accounts: it persists the
//! encrypted blob tree from ADR-0005 and a change feed, and never sees keys
//! or plaintext. Accounts and bearer tokens guard *access*; the E2EE key
//! hierarchy stays client-side. Configuration is environment-only; the
//! binary adds `serve` and an admin `reset-password` command.
//!
//! Milestone M4 lands in chunks (ADR-0007 §12): this chunk carries accounts,
//! device sessions and the storage layer; the blob API, change feed, SSE and
//! pairing follow.

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
