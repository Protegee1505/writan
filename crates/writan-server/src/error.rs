//! The error type every handler returns (ADR-0007 §7 of the DTO rules: JSON
//! bodies, camelCase fields where structured).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// A refused request. `Display` is the exact text the client shows; internal
/// details are logged, never sent.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("missing or invalid credentials")]
    Unauthorized,
    #[error("registration is closed")]
    RegistrationClosed,
    #[error(
        "this account predates this server version; the server owner must run `writan-server reset-password EMAIL`"
    )]
    LegacyAccount,
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error("too many failed attempts; try again later")]
    Locked,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Quota(String),
    #[error("request body is too large")]
    TooLarge,
    #[error("internal error")]
    Internal(String),
}

impl ApiError {
    /// The HTTP status for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::RegistrationClosed => StatusCode::FORBIDDEN,
            Self::LegacyAccount => StatusCode::FORBIDDEN,
            Self::EmailTaken => StatusCode::CONFLICT,
            Self::Locked => StatusCode::TOO_MANY_REQUESTS,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Quota(_) => StatusCode::INSUFFICIENT_STORAGE,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(format!("sqlite: {error}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = match &self {
            // Log the cause server-side; the client gets the generic text.
            Self::Internal(detail) => {
                eprintln!("writan-server: internal error: {detail}");
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
