//! SQLite + FTS5 index over the vault. Fully rebuildable from the files.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, types::Value as SqlValue};
use uuid::Uuid;

use crate::NoteId;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("corrupt index row: {0}")]
    Corrupt(String),
}

/// A note as stored in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedNote {
    pub id: NoteId,
    pub rel_path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub mtime_ns: i64,
    pub size: i64,
    pub hash: String,
}

/// Search parameters. `text` is a plain user query — never raw FTS syntax.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: String,
    /// All of these tags must be present.
    pub tags: Vec<String>,
    /// Restrict to a vault subfolder (vault-relative, `/`-separated).
    pub folder: Option<String>,
    pub pinned_only: bool,
    /// Defaults to 50.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: NoteId,
    pub rel_path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub snippet: String,
}

const SCHEMA: &str = r"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS notes (
    id       TEXT PRIMARY KEY NOT NULL,
    rel_path TEXT NOT NULL UNIQUE,
    title    TEXT NOT NULL,
    tags     TEXT NOT NULL DEFAULT '[]',
    pinned   INTEGER NOT NULL DEFAULT 0,
    created  TEXT,
    updated  TEXT,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    size     INTEGER NOT NULL DEFAULT 0,
    hash     TEXT NOT NULL DEFAULT ''
);
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    id UNINDEXED, title, body, tokenize = 'trigram'
);
";

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open(path)?;
        let index = Self { conn };
        index.init()?;
        Ok(index)
    }

    pub fn open_in_memory() -> Result<Self, IndexError> {
        let conn = Connection::open_in_memory()?;
        let index = Self { conn };
        index.init()?;
        Ok(index)
    }

    fn init(&self) -> Result<(), IndexError> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    pub fn count(&self) -> Result<i64, IndexError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?)
    }

    pub fn all_paths(&self) -> Result<Vec<String>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path FROM notes ORDER BY rel_path")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn tag_counts(&self) -> Result<Vec<(String, i64)>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT value AS tag, COUNT(*) AS n
             FROM notes, json_each(notes.tags)
             GROUP BY value
             ORDER BY n DESC, tag ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get(&self, id: &NoteId) -> Result<Option<IndexedNote>, IndexError> {
        let row = self
            .conn
            .query_row(SELECT_NOTE_BY_ID, [id.to_string()], row_to_note)
            .optional()?;
        row.map(note_from_row).transpose()
    }

    pub fn get_by_path(&self, rel_path: &str) -> Result<Option<IndexedNote>, IndexError> {
        let row = self
            .conn
            .query_row(SELECT_NOTE_BY_PATH, [rel_path], row_to_note)
            .optional()?;
        row.map(note_from_row).transpose()
    }

    pub fn upsert(&mut self, note: &IndexedNote, body: &str) -> Result<(), IndexError> {
        let tags = serde_json::to_string(&note.tags)?;
        let id = note.id.to_string();
        let tx = self.conn.transaction()?;

        // If another note (different id) owned this path, it was replaced.
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM notes WHERE rel_path = ?1",
                [&note.rel_path],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(other) = existing
            && other != id
        {
            tx.execute("DELETE FROM notes WHERE id = ?1", [&other])?;
            tx.execute("DELETE FROM notes_fts WHERE id = ?1", [&other])?;
        }

        tx.execute(
            "INSERT INTO notes (id, rel_path, title, tags, pinned, created, updated, mtime_ns, size, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                 rel_path = excluded.rel_path,
                 title    = excluded.title,
                 tags     = excluded.tags,
                 pinned   = excluded.pinned,
                 created  = excluded.created,
                 updated  = excluded.updated,
                 mtime_ns = excluded.mtime_ns,
                 size     = excluded.size,
                 hash     = excluded.hash",
            rusqlite::params![
                id,
                note.rel_path,
                note.title,
                tags,
                i64::from(note.pinned),
                note.created,
                note.updated,
                note.mtime_ns,
                note.size,
                note.hash,
            ],
        )?;

        tx.execute("DELETE FROM notes_fts WHERE id = ?1", [&id])?;
        tx.execute(
            "INSERT INTO notes_fts (id, title, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, note.title, body],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&mut self, id: &NoteId) -> Result<(), IndexError> {
        let id = id.to_string();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM notes WHERE id = ?1", [&id])?;
        tx.execute("DELETE FROM notes_fts WHERE id = ?1", [&id])?;
        tx.commit()?;
        Ok(())
    }

    /// Forget the note at `rel_path`; returns the id it had, if any.
    pub fn remove_by_path(&mut self, rel_path: &str) -> Result<Option<NoteId>, IndexError> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM notes WHERE rel_path = ?1",
                [rel_path],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = existing else {
            return Ok(None);
        };
        let parsed = Uuid::parse_str(&id).map_err(|e| IndexError::Corrupt(e.to_string()))?;
        self.remove(&parsed)?;
        Ok(Some(parsed))
    }

    pub fn clear(&mut self) -> Result<(), IndexError> {
        self.conn
            .execute_batch("DELETE FROM notes; DELETE FROM notes_fts;")?;
        Ok(())
    }

    /// Notes ordered pinned-first, then most recently updated.
    pub fn list(&self, limit: usize) -> Result<Vec<IndexedNote>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, title, tags, pinned, created, updated, mtime_ns, size, hash
             FROM notes
             ORDER BY pinned DESC, COALESCE(updated, created) DESC, title COLLATE NOCASE ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], row_to_note)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(note_from_row(row?)?);
        }
        Ok(out)
    }

    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, IndexError> {
        let limit = query.limit.unwrap_or(50).clamp(1, 500);
        let trimmed = query.text.trim();

        let mut conditions: Vec<String> = Vec::new();
        let mut values: Vec<SqlValue> = Vec::new();
        let mut order = String::from("ORDER BY n.pinned DESC, COALESCE(n.updated, n.created) DESC");

        if !trimmed.is_empty() {
            let terms: Vec<&str> = trimmed.split_whitespace().collect();
            let all_long_enough = terms.iter().all(|t| t.chars().count() >= 3);
            if all_long_enough {
                // Trigram FTS: every term is quoted, so user input can never
                // break the MATCH syntax.
                let expr = terms
                    .iter()
                    .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" ");
                conditions.push("notes_fts MATCH ?".to_string());
                values.push(SqlValue::Text(expr));
                order = String::from(
                    "ORDER BY n.pinned DESC, bm25(notes_fts) ASC, COALESCE(n.updated, n.created) DESC",
                );
            } else {
                // Short queries (< 3 chars, e.g. 2-char CJK words) can never
                // match trigram tokens; fall back to substring search.
                let like = format!("%{}%", escape_like(trimmed));
                conditions.push(
                    "(n.title LIKE ? ESCAPE '\\' OR notes_fts.body LIKE ? ESCAPE '\\')".to_string(),
                );
                values.push(SqlValue::Text(like.clone()));
                values.push(SqlValue::Text(like));
            }
        }

        for tag in &query.tags {
            conditions.push("EXISTS (SELECT 1 FROM json_each(n.tags) WHERE value = ?)".to_string());
            values.push(SqlValue::Text(tag.clone()));
        }

        if let Some(folder) = &query.folder {
            let folder = folder.trim_matches('/');
            if !folder.is_empty() {
                conditions.push("n.rel_path LIKE ? ESCAPE '\\'".to_string());
                values.push(SqlValue::Text(format!(
                    "{}%",
                    escape_like(&format!("{folder}/"))
                )));
            }
        }

        if query.pinned_only {
            conditions.push("n.pinned = 1".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT n.id, n.rel_path, n.title, n.tags, n.pinned, notes_fts.body
             FROM notes_fts JOIN notes n ON n.id = notes_fts.id
             {where_clause}
             {order}
             LIMIT ?"
        );
        values.push(SqlValue::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut hits = Vec::new();
        for row in rows {
            let (id, rel_path, title, tags, pinned, body) = row?;
            let id = Uuid::parse_str(&id).map_err(|e| IndexError::Corrupt(e.to_string()))?;
            let tags = serde_json::from_str(&tags).unwrap_or_default();
            let snippet = make_snippet(&body, trimmed);
            hits.push(SearchHit {
                id,
                rel_path,
                title,
                tags,
                pinned: pinned != 0,
                snippet,
            });
        }
        Ok(hits)
    }
}

const SELECT_NOTE_COLUMNS: &str =
    "id, rel_path, title, tags, pinned, created, updated, mtime_ns, size, hash";
const SELECT_NOTE_BY_ID: &str = "SELECT id, rel_path, title, tags, pinned, created, updated, mtime_ns, size, hash FROM notes WHERE id = ?1";
const SELECT_NOTE_BY_PATH: &str = "SELECT id, rel_path, title, tags, pinned, created, updated, mtime_ns, size, hash FROM notes WHERE rel_path = ?1";

#[allow(dead_code)]
fn _columns_doc() -> &'static str {
    SELECT_NOTE_COLUMNS
}

type NoteRow = (
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
);

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn note_from_row(row: NoteRow) -> Result<IndexedNote, IndexError> {
    let id = Uuid::parse_str(&row.0).map_err(|e| IndexError::Corrupt(e.to_string()))?;
    let tags = serde_json::from_str(&row.3).unwrap_or_default();
    Ok(IndexedNote {
        id,
        rel_path: row.1,
        title: row.2,
        tags,
        pinned: row.4 != 0,
        created: row.5,
        updated: row.6,
        mtime_ns: row.7,
        size: row.8,
        hash: row.9,
    })
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A short single-line excerpt around the first occurrence of `needle`.
fn make_snippet(body: &str, needle: &str) -> String {
    let clean: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let total = clean.chars().count();
    if total <= 160 {
        return clean;
    }
    let lower = clean.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let Some(byte_pos) = (!lower_needle.is_empty())
        .then(|| lower.find(&lower_needle))
        .flatten()
    else {
        return format!("{}…", clean.chars().take(160).collect::<String>());
    };
    let char_pos = lower[..byte_pos].chars().count();
    let start = char_pos.saturating_sub(60);
    let end = (start + 160).min(total);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(clean.chars().skip(start).take(end - start));
    if end < total {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, rel: &str, title: &str, tags: &[&str], pinned: bool) -> IndexedNote {
        IndexedNote {
            id: Uuid::parse_str(id).expect("uuid"),
            rel_path: rel.to_string(),
            title: title.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            pinned,
            created: Some("2026-01-01T00:00:00Z".into()),
            updated: Some("2026-01-02T00:00:00Z".into()),
            mtime_ns: 1,
            size: 1,
            hash: "h".into(),
        }
    }

    const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    fn seed(index: &mut Index) {
        index
            .upsert(
                &note(A, "alpha.md", "Alpha", &["work"], false),
                "the quick brown fox jumps over the lazy dog",
            )
            .expect("a");
        index
            .upsert(
                &note(B, "beta.md", "Beta", &["work", "fun"], true),
                "笔记应用要快，搜索要好用。",
            )
            .expect("b");
        index
            .upsert(
                &note(C, "sub/gamma.md", "Gamma", &[], false),
                "unrelated text about nothing",
            )
            .expect("c");
    }

    #[test]
    fn upsert_get_and_count() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        assert_eq!(index.count().expect("count"), 3);
        let got = index
            .get(&Uuid::parse_str(A).expect("uuid"))
            .expect("get")
            .expect("some");
        assert_eq!(got.title, "Alpha");
        assert_eq!(got.tags, vec!["work".to_string()]);
        assert!(
            index
                .get_by_path("beta.md")
                .expect("get")
                .expect("some")
                .pinned
        );
    }

    #[test]
    fn search_words() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        let hits = index
            .search(&SearchQuery {
                text: "quick fox".into(),
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Alpha");
        assert!(hits[0].snippet.contains("quick"), "{}", hits[0].snippet);
    }

    #[test]
    fn search_cjk_trigram_and_short_query() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        // 3 chars: trigram FTS path.
        let hits = index
            .search(&SearchQuery {
                text: "笔记应".into(),
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Beta");
        // 2 chars: LIKE fallback path.
        let hits = index
            .search(&SearchQuery {
                text: "搜索".into(),
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Beta");
    }

    #[test]
    fn search_filters() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        let hits = index
            .search(&SearchQuery {
                tags: vec!["work".into()],
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 2);

        let hits = index
            .search(&SearchQuery {
                pinned_only: true,
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Beta");

        let hits = index
            .search(&SearchQuery {
                folder: Some("sub".into()),
                ..Default::default()
            })
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Gamma");
    }

    #[test]
    fn search_never_breaks_on_fts_syntax() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        for weird in [
            "\"unbalanced",
            "AND OR NOT",
            "a*b(c)",
            "quick OR fox",
            "\"quick fox\"",
            "100%",
        ] {
            let hits = index
                .search(&SearchQuery {
                    text: weird.into(),
                    ..Default::default()
                })
                .expect(weird);
            let _ = hits;
        }
    }

    #[test]
    fn upsert_takes_over_rel_path() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        index
            .upsert(
                &note(B, "alpha.md", "Beta Moved", &[], false),
                "new content",
            )
            .expect("upsert");
        assert!(
            index
                .get_by_path("alpha.md")
                .expect("get")
                .expect("some")
                .id
                == Uuid::parse_str(B).expect("uuid")
        );
        assert_eq!(index.count().expect("count"), 2);
        let hits = index
            .search(&SearchQuery {
                text: "quick fox".into(),
                ..Default::default()
            })
            .expect("search");
        assert!(hits.is_empty(), "stale FTS rows: {hits:?}");
    }

    #[test]
    fn remove_and_clear() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        let id = Uuid::parse_str(A).expect("uuid");
        index.remove(&id).expect("remove");
        assert_eq!(index.count().expect("count"), 2);
        assert!(index.get(&id).expect("get").is_none());
        index.clear().expect("clear");
        assert_eq!(index.count().expect("count"), 0);
        let hits = index
            .search(&SearchQuery {
                text: "quick".into(),
                ..Default::default()
            })
            .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn list_orders_pinned_first() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        let listed = index.list(10).expect("list");
        assert_eq!(listed[0].title, "Beta");
    }

    #[test]
    fn tag_counts_aggregate() {
        let mut index = Index::open_in_memory().expect("open");
        seed(&mut index);
        let counts = index.tag_counts().expect("counts");
        assert_eq!(
            counts.iter().find(|(t, _)| t == "work").map(|(_, n)| *n),
            Some(2)
        );
        assert_eq!(
            counts.iter().find(|(t, _)| t == "fun").map(|(_, n)| *n),
            Some(1)
        );
    }

    #[test]
    fn snippet_marks_truncation() {
        let body = "x".repeat(400);
        let s = make_snippet(&body, "x");
        assert!(s.starts_with('…') || s.ends_with('…'));
        assert!(s.chars().count() <= 162);
        assert_eq!(make_snippet("short body", "body"), "short body");
    }
}
