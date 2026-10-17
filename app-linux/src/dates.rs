//! Timestamp formatting for the Linux shell: the Tauri `fmtDate` from
//! `app/src/lib/format.ts`, so both shells read the same `deleted_at` and
//! `updated` strings the same way. GTK-free so it is testable on any target.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// `settings.view.dateFormat` (ADR-0003): the Tauri registry defaults to the
/// relative shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    Relative,
    Absolute,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Format an RFC 3339 timestamp the way `fmtDate` does: relative ("just now",
/// "5m ago", "3h ago", "2d ago", then a plain date), or absolute
/// ("Oct 17, 12:00"). Unparseable input reads as an empty string.
pub fn fmt_date(raw: &str, format: DateFormat) -> String {
    match OffsetDateTime::parse(raw, &Rfc3339) {
        Ok(at) => fmt_date_at(at, format, OffsetDateTime::now_utc()),
        Err(_) => String::new(),
    }
}

/// The testable core of [`fmt_date`]: `now` is passed in.
pub fn fmt_date_at(at: OffsetDateTime, format: DateFormat, now: OffsetDateTime) -> String {
    let seconds = (now - at).whole_seconds().max(0);
    if format == DateFormat::Absolute {
        return absolute(at, now, true);
    }
    let minutes = (seconds + 30) / 60;
    if minutes < 1 {
        return "just now".to_string();
    }
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = (minutes + 30) / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = (hours + 12) / 24;
    if days < 7 {
        return format!("{days}d ago");
    }
    absolute(at, now, false)
}

/// "Oct 17, 12:00" (with the year when it is not the current one). The Tauri
/// side follows the locale's 12/24-hour choice; this shell always uses 24-hour.
fn absolute(at: OffsetDateTime, now: OffsetDateTime, with_time: bool) -> String {
    let month = MONTHS[at.month() as usize - 1];
    let day = at.day();
    let mut out = format!("{month} {day}");
    if at.year() != now.year() {
        out.push_str(&format!(", {}", at.year()));
    }
    if with_time {
        out.push_str(&format!(", {:02}:{:02}", at.hour(), at.minute()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> OffsetDateTime {
        OffsetDateTime::parse(raw, &Rfc3339).expect("fixture")
    }

    #[test]
    fn relative_shapes_match_the_tauri_formatter() {
        let now = at("2026-10-17T12:00:00Z");
        let fmt = |raw: &str| fmt_date_at(at(raw), DateFormat::Relative, now);
        assert_eq!(fmt("2026-10-17T11:59:50Z"), "just now");
        assert_eq!(fmt("2026-10-17T11:55:00Z"), "5m ago");
        assert_eq!(fmt("2026-10-17T09:00:00Z"), "3h ago");
        assert_eq!(fmt("2026-10-15T12:00:00Z"), "2d ago");
        // A week or more reads as a date, without the time.
        assert_eq!(fmt("2026-10-07T12:00:00Z"), "Oct 7");
        assert_eq!(fmt("2025-10-17T12:00:00Z"), "Oct 17, 2025");
    }

    #[test]
    fn absolute_shapes_and_malformed_input() {
        let now = at("2026-10-17T12:00:00Z");
        let fmt = |raw: &str| fmt_date_at(at(raw), DateFormat::Absolute, now);
        assert_eq!(fmt("2026-10-17T14:32:00Z"), "Oct 17, 14:32");
        assert_eq!(fmt("2025-12-31T23:59:00Z"), "Dec 31, 2025, 23:59");
        assert_eq!(fmt_date("not a date", DateFormat::Relative), "");
        assert_eq!(fmt_date("", DateFormat::Absolute), "");
    }
}
