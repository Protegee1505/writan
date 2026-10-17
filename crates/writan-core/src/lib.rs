//! Core domain types for the Writan note vault.
//!
//! Markdown files with YAML frontmatter are the source of truth; everything
//! else (the SQLite index, trash state) must be rebuildable from the vault
//! directory alone.

#![forbid(unsafe_code)]

pub mod index;
pub mod trash;
pub mod vault;
pub mod watch;
pub mod workspace;

pub use index::{Index, IndexError, IndexedNote, SearchHit, SearchQuery};
pub use trash::{Trash, TrashEntry, TrashError};
pub use vault::{FileStat, Note, ParsedFile, Vault, VaultError};
pub use watch::{VaultEvent, VaultWatcher, WatchError};
pub use workspace::{ReconcileStats, Workspace, WorkspaceError};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type NoteId = Uuid;

/// Metadata carried in a note's frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: Uuid,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// RFC 3339 timestamp in UTC for the current moment.
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

/// Content hash used for change detection and, later, sync.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Split a document into its frontmatter block and its body.
///
/// Frontmatter is recognised only when the document starts with a `---` fence
/// on its own line and a closing `---` line follows. Returns
/// `(Some(frontmatter), body)` or `(None, whole_document)`.
pub fn split_frontmatter(doc: &str) -> (Option<&str>, &str) {
    let Some(rest) = strip_line(doc, "---") else {
        return (None, doc);
    };
    let mut offset = doc.len() - rest.len();
    for line in rest.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
        offset += line.len();
        if trimmed == "---" {
            let fm_start = doc.len() - rest.len();
            let fm_end = offset - line.len();
            let body = &doc[offset..];
            return (Some(&doc[fm_start..fm_end]), body);
        }
    }
    (None, doc)
}

/// Strip `line + newline` from the start of `doc` if it is present.
fn strip_line<'a>(doc: &'a str, line: &str) -> Option<&'a str> {
    let rest = doc.strip_prefix(line)?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frontmatter() {
        let doc = "# Hello\n\nbody";
        assert_eq!(split_frontmatter(doc), (None, doc));
    }

    #[test]
    fn simple_frontmatter() {
        let doc = "---\nid: abc\ntags: [a]\n---\n# Title\n\nbody\n";
        let (fm, body) = split_frontmatter(doc);
        assert_eq!(fm, Some("id: abc\ntags: [a]\n"));
        assert_eq!(body, "# Title\n\nbody\n");
    }

    #[test]
    fn crlf_frontmatter() {
        let doc = "---\r\nid: abc\r\n---\r\nbody";
        let (fm, body) = split_frontmatter(doc);
        assert_eq!(fm, Some("id: abc\r\n"));
        assert_eq!(body, "body");
    }

    #[test]
    fn unterminated_fence_is_not_frontmatter() {
        let doc = "---\nid: abc\nno closing fence";
        assert_eq!(split_frontmatter(doc), (None, doc));
    }

    #[test]
    fn hr_at_start_is_not_frontmatter() {
        let doc = "text before\n---\nmore";
        assert_eq!(split_frontmatter(doc), (None, doc));
    }

    #[test]
    fn empty_document() {
        assert_eq!(split_frontmatter(""), (None, ""));
    }

    #[test]
    fn meta_serialization_skips_defaults() {
        let meta = NoteMeta {
            id: Uuid::nil(),
            title: "Hi".into(),
            tags: Vec::new(),
            pinned: false,
            created: None,
            updated: None,
        };
        let yaml = serde_saphyr::to_string(&meta).expect("serialize");
        assert_eq!(
            yaml,
            "id: 00000000-0000-0000-0000-000000000000\ntitle: Hi\n"
        );
    }
}
