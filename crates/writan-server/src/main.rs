//! Writan sync server: stores encrypted note blobs and pushes change events.
//!
//! M0 skeleton: `/health` only. Blob storage, accounts and SSE push arrive in
//! milestone M4; the server never sees plaintext.

#![forbid(unsafe_code)]

use axum::{Router, routing::get};

async fn health() -> &'static str {
    "ok"
}

fn app() -> Router {
    Router::new().route("/health", get(health))
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("WRITAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");
    println!("writan-server listening on {addr}");
    axum::serve(listener, app()).await.expect("server error");
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn health_returns_ok() {
        assert_eq!(super::health().await, "ok");
    }
}
