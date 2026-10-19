//! Turning text shared into the app into a note draft.
//!
//! Android's share sheet hands the app an `ACTION_SEND` intent; this module is
//! the platform-independent half — derive a title and a body from the shared
//! text (and optional subject) so the workspace can create the note.

/// A note draft derived from a share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareDraft {
    pub title: String,
    pub body: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShareError {
    #[error("nothing to save: the shared text was empty")]
    Empty,
}

/// Longest title a share produces; longer ones are cut at a char boundary.
const MAX_TITLE_CHARS: usize = 80;
const FALLBACK_TITLE: &str = "Shared note";

/// Derive a title and body from shared text, using `subject` as the title when
/// the sharing app provided one.
///
/// - CRLF and lone CR line endings are normalised to LF; outer blanks trimmed.
/// - A non-empty `subject` wins as the title.
/// - Otherwise the first non-empty line becomes the title, with Markdown
///   heading/list markers stripped; a single URL becomes its host.
/// - The title is capped at [`MAX_TITLE_CHARS`] characters.
/// - Whitespace-only text is rejected with [`ShareError::Empty`].
pub fn draft_from_share(text: &str, subject: Option<&str>) -> Result<ShareDraft, ShareError> {
    let body = normalize(text).trim().to_string();
    if body.is_empty() {
        return Err(ShareError::Empty);
    }
    let title = subject
        .and_then(|subject| first_non_empty_line(&normalize(subject)).map(strip_markers))
        .filter(|title| !title.is_empty())
        .or_else(|| title_from_body(&body))
        .unwrap_or_else(|| FALLBACK_TITLE.to_string());
    Ok(ShareDraft {
        title: truncate_title(&title),
        body,
    })
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Drop a leading heading or list marker so titles do not start with `##` or `-`.
fn strip_markers(line: &str) -> String {
    let mut rest = line.trim();
    if let Some(after) = rest.strip_prefix('#') {
        rest = after.trim_start_matches('#').trim_start();
    }
    while let Some(after) = rest.strip_prefix('>') {
        rest = after.trim_start();
    }
    if let Some(after) = rest.strip_prefix(['-', '*', '+'])
        && (after.is_empty() || after.starts_with(char::is_whitespace))
    {
        rest = after.trim_start();
    }
    rest.trim().to_string()
}

fn title_from_body(body: &str) -> Option<String> {
    if let Some(host) = single_url_host(body) {
        return Some(host.unwrap_or_else(|| "Shared link".to_string()));
    }
    let line = first_non_empty_line(body)?;
    let stripped = strip_markers(line);
    (!stripped.is_empty()).then_some(stripped)
}

/// The host of `text` when it is exactly one http(s) URL, `None` otherwise.
/// A URL without a host (`https://`) yields `Some(None)`.
fn single_url_host(text: &str) -> Option<Option<String>> {
    if text.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    Some((!host.is_empty()).then(|| host.to_string()))
}

fn truncate_title(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title.to_string();
    }
    let mut cut: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
    while cut.ends_with(char::is_whitespace) {
        cut.pop();
    }
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_wins_and_crlf_is_normalized() {
        let draft = draft_from_share("hello\r\nworld", Some("From Chrome")).unwrap();
        assert_eq!(draft.title, "From Chrome");
        assert_eq!(draft.body, "hello\nworld");
    }

    #[test]
    fn first_line_becomes_the_title() {
        let draft = draft_from_share("Shopping list\nmilk\neggs", None).unwrap();
        assert_eq!(draft.title, "Shopping list");
        assert_eq!(draft.body, "Shopping list\nmilk\neggs");
    }

    #[test]
    fn markdown_markers_are_stripped_from_the_title() {
        assert_eq!(
            draft_from_share("## Heading\n\nbody", None).unwrap().title,
            "Heading"
        );
        assert_eq!(
            draft_from_share("- milk\n- eggs", None).unwrap().title,
            "milk"
        );
        assert_eq!(
            draft_from_share("> quoted\nmore", None).unwrap().title,
            "quoted"
        );
    }

    #[test]
    fn a_single_url_becomes_its_host() {
        assert_eq!(
            draft_from_share("https://www.example.com/page?q=1#top", None)
                .unwrap()
                .title,
            "example.com"
        );
        assert_eq!(
            draft_from_share("http://user@notes.example.org:8443/x", None)
                .unwrap()
                .title,
            "notes.example.org"
        );
    }

    #[test]
    fn a_url_without_a_host_falls_back() {
        assert_eq!(
            draft_from_share("https://", None).unwrap().title,
            "Shared link"
        );
    }

    #[test]
    fn a_url_among_other_text_keeps_the_first_line() {
        let draft = draft_from_share("Read this https://example.com now", None).unwrap();
        assert_eq!(draft.title, "Read this https://example.com now");
    }

    #[test]
    fn long_titles_are_capped_at_a_char_boundary() {
        let text = "a".repeat(120);
        let title = draft_from_share(&text, None).unwrap().title;
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
        // Multi-byte characters are cut cleanly too.
        let cjk = "汉".repeat(120);
        let title = draft_from_share(&cjk, None).unwrap().title;
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.starts_with("汉汉"));
    }

    #[test]
    fn empty_and_whitespace_only_text_is_rejected() {
        assert_eq!(draft_from_share("", None), Err(ShareError::Empty));
        assert_eq!(draft_from_share("  \n \r\n", None), Err(ShareError::Empty));
        assert_eq!(
            draft_from_share("\r\n\r\n", Some("  ")),
            Err(ShareError::Empty)
        );
    }

    #[test]
    fn a_blank_subject_falls_back_to_the_body() {
        let draft = draft_from_share("Some text", Some("   \n  ")).unwrap();
        assert_eq!(draft.title, "Some text");
    }

    #[test]
    fn marker_only_text_still_gets_a_title() {
        let draft = draft_from_share("#", None).unwrap();
        assert_eq!(draft.title, FALLBACK_TITLE);
        assert_eq!(draft.body, "#");
    }

    #[test]
    fn multi_line_bodies_are_kept_whole() {
        let body = "Line one\n\nLine three\n";
        let draft = draft_from_share(body, None).unwrap();
        assert_eq!(draft.body, "Line one\n\nLine three");
        assert_eq!(draft.title, "Line one");
    }
}
