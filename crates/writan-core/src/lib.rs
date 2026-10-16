//! Core domain types for the Writan note vault.
//!
//! Markdown files with YAML frontmatter are the source of truth; everything
//! in this crate must be rebuildable from the vault directory alone.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type NoteId = Uuid;

/// Metadata carried in a note's frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
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
}
