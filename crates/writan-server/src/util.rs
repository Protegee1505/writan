//! Small shared helpers: timestamps and blob-path validation.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::ApiError;

/// Seconds since the Unix epoch (UTC).
pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// RFC 3339 rendering (ADR-0007 §12); a bogus stored timestamp falls back to
/// the epoch rather than failing a response.
pub fn rfc3339(unix_secs: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix_secs)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// Longest accepted blob path, in bytes (ADR-0007 §6).
pub const BLOB_PATH_MAX_BYTES: usize = 512;

/// Validate a stored blob path (ADR-0007 §6): relative, at most 512 bytes, no
/// empty, `.` or `..` segment and no control characters. Anything else is
/// accepted as opaque bytes — the same forward-compatibility rule as the
/// folder target (ADR-0005 §12).
pub fn validate_blob_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty() {
        return Err(ApiError::BadRequest("the blob path is empty".to_string()));
    }
    if path.len() > BLOB_PATH_MAX_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the blob path is longer than {BLOB_PATH_MAX_BYTES} bytes"
        )));
    }
    if path.starts_with('/') {
        return Err(ApiError::BadRequest(
            "the blob path must be relative".to_string(),
        ));
    }
    if path.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(
            "the blob path must not contain control characters".to_string(),
        ));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(ApiError::BadRequest(
                "the blob path must not contain an empty segment".to_string(),
            ));
        }
        if segment == "." || segment == ".." {
            return Err(ApiError::BadRequest(format!(
                "the blob path must not contain a `{segment}` segment"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_renders_utc_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn rfc3339_survives_out_of_range_timestamps() {
        assert_eq!(rfc3339(i64::MAX), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn blob_paths_follow_the_contract() {
        for good in [
            "keys/1.json",
            "tombstones/00000000-0000-0000-0000-000000000000",
            "items/6f1d/9c2e/17.enc",
            "weird name/with spaces.enc",
        ] {
            assert!(validate_blob_path(good).is_ok(), "{good} should pass");
        }
        for bad in [
            "",
            "/absolute",
            "a//b",
            "a/./b",
            "a/../b",
            "../escape",
            "new\nline",
        ] {
            assert!(validate_blob_path(bad).is_err(), "{bad} should fail");
        }
        assert!(validate_blob_path(&"a".repeat(BLOB_PATH_MAX_BYTES)).is_ok());
        assert!(validate_blob_path(&"a".repeat(BLOB_PATH_MAX_BYTES + 1)).is_err());
    }
}
