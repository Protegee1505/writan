//! Small shared helpers.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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
}
