//! The vault: Markdown files on disk are the source of truth.

use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
use uuid::Uuid;

use crate::{NoteId, NoteMeta, content_hash, now_rfc3339, split_frontmatter};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("frontmatter in {path} is not valid: {message}")]
    Frontmatter { path: PathBuf, message: String },
    #[error("note at {0} has no frontmatter id")]
    MissingId(String),
}

fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> VaultError {
    VaultError::Io {
        path: path.into(),
        source,
    }
}

/// A note loaded from its Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub meta: NoteMeta,
    pub body: String,
    /// Path relative to the vault root, always `/`-separated.
    pub rel_path: String,
    /// Unknown frontmatter keys, preserved verbatim across writes.
    pub extra: Mapping,
}

/// `stat` data used for change detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub mtime_ns: i64,
    pub size: i64,
}

/// A note file that was parsed leniently: it may not carry a usable id yet
/// (which is how foreign Markdown files enter the vault).
#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// Whole file text (including frontmatter).
    pub text: String,
    pub body: String,
    /// Full metadata when the frontmatter has an `id`.
    pub meta: Option<NoteMeta>,
    /// Lenient fields used when adopting a file without an id.
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub extra: Mapping,
    pub hash: String,
}

pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Open (or create) a vault rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = normalize_root(root.as_ref());
        fs::create_dir_all(&root).map_err(|e| io_err(&root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Convert an absolute path into a vault-relative `/`-separated path.
    pub fn rel_path(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut out = String::new();
        for component in rel.components() {
            match component {
                Component::Normal(part) => {
                    if !out.is_empty() {
                        out.push('/');
                    }
                    out.push_str(&part.to_string_lossy());
                }
                _ => return None,
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    pub fn abs_path(&self, rel: &str) -> PathBuf {
        let mut path = self.root.clone();
        for part in rel.split('/') {
            path.push(part);
        }
        path
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.abs_path(rel).is_file()
    }

    pub fn stat(&self, rel: &str) -> Result<FileStat, VaultError> {
        let path = self.abs_path(rel);
        let md = fs::metadata(&path).map_err(|e| io_err(&path, e))?;
        Ok(FileStat {
            mtime_ns: mtime_ns(&md),
            size: md.len() as i64,
        })
    }

    /// All note files in the vault, `/`-separated and sorted.
    ///
    /// Hidden files and directories (leading `.`) and non-`.md` files are
    /// skipped; those are never notes (temp files, `.git`, editors' droppings).
    pub fn walk_notes(&self) -> Result<Vec<String>, VaultError> {
        let mut out = Vec::new();
        self.walk_dir(&self.root, &mut out)?;
        out.sort();
        Ok(out)
    }

    fn walk_dir(&self, dir: &Path, out: &mut Vec<String>) -> Result<(), VaultError> {
        let entries = fs::read_dir(dir).map_err(|e| io_err(dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_err(dir, e))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type().map_err(|e| io_err(&path, e))?;
            if file_type.is_dir() {
                self.walk_dir(&path, out)?;
            } else if file_type.is_file() && is_markdown(name) {
                if let Some(rel) = self.rel_path(&path) {
                    out.push(rel);
                }
            }
        }
        Ok(())
    }

    pub fn parse_file(&self, rel: &str) -> Result<ParsedFile, VaultError> {
        let path = self.abs_path(rel);
        let text = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        let hash = content_hash(text.as_bytes());
        let (frontmatter, body) = split_frontmatter(&text);

        let mut parsed = ParsedFile {
            body: body.to_string(),
            text: text.clone(),
            meta: None,
            title: None,
            tags: Vec::new(),
            pinned: false,
            extra: Mapping::new(),
            hash,
        };

        let Some(yaml) = frontmatter else {
            return Ok(parsed);
        };
        let value: Value = serde_yaml_ng::from_str(yaml).map_err(|e| VaultError::Frontmatter {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let Value::Mapping(mapping) = value else {
            if matches!(value, Value::Null) {
                return Ok(parsed);
            }
            return Err(VaultError::Frontmatter {
                path,
                message: format!("expected a YAML mapping, found {}", kind_of(&value)),
            });
        };

        let lenient: LenientMeta = serde_yaml_ng::from_value(Value::Mapping(mapping.clone()))
            .map_err(|e| VaultError::Frontmatter {
                path: path.clone(),
                message: e.to_string(),
            })?;

        parsed.title = lenient.title.clone();
        parsed.tags = lenient.tags.clone();
        parsed.pinned = lenient.pinned;
        if let Some(id) = lenient.id {
            parsed.meta = Some(NoteMeta {
                id,
                title: lenient
                    .title
                    .unwrap_or_else(|| derive_title(&parsed.body, rel)),
                tags: lenient.tags,
                pinned: lenient.pinned,
                created: lenient.created,
                updated: lenient.updated,
            });
        }
        parsed.extra = mapping;
        Ok(parsed)
    }

    /// Strict read: the file must carry a frontmatter `id`.
    pub fn read(&self, rel: &str) -> Result<Note, VaultError> {
        let parsed = self.parse_file(rel)?;
        let Some(meta) = parsed.meta else {
            return Err(VaultError::MissingId(rel.to_string()));
        };
        Ok(Note {
            meta,
            body: parsed.body,
            rel_path: rel.to_string(),
            extra: parsed.extra,
        })
    }

    /// Write a note atomically (temp file + rename) to `note.rel_path`.
    pub fn write(&self, note: &Note) -> Result<(), VaultError> {
        let path = self.abs_path(&note.rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let document = compose_document(note)?;
        atomic_write(&path, &document).map_err(|e| io_err(&path, e))
    }

    /// Create a new empty note with a fresh id and a `slug-<id8>.md` file name.
    pub fn create(&self, title: &str, tags: &[String]) -> Result<Note, VaultError> {
        let id = Uuid::new_v4();
        let now = now_rfc3339();
        let rel_path = self.unique_rel_path(&slugify(title), id, None);
        let note = Note {
            meta: NoteMeta {
                id,
                title: title.trim().to_string(),
                tags: tags.to_vec(),
                pinned: false,
                created: Some(now.clone()),
                updated: Some(now),
            },
            body: String::new(),
            rel_path,
            extra: Mapping::new(),
        };
        self.write(&note)?;
        Ok(note)
    }

    /// Change a note's title and move its file to the matching new name.
    pub fn rename(&self, note: &Note, new_title: &str) -> Result<Note, VaultError> {
        let mut renamed = note.clone();
        renamed.meta.title = new_title.trim().to_string();
        let rel_path =
            self.unique_rel_path(&slugify(new_title), note.meta.id, Some(&note.rel_path));
        if rel_path == note.rel_path {
            self.write(&renamed)?;
            return Ok(renamed);
        }
        renamed.rel_path = rel_path;
        self.write(&renamed)?;
        let old_path = self.abs_path(&note.rel_path);
        fs::remove_file(&old_path).map_err(|e| io_err(&old_path, e))?;
        Ok(renamed)
    }

    pub fn remove(&self, rel: &str) -> Result<(), VaultError> {
        let path = self.abs_path(rel);
        fs::remove_file(&path).map_err(|e| io_err(&path, e))
    }

    /// First free `slug-<id8>.md` (or `slug-<id8>-N.md` on collisions).
    pub fn unique_rel_path(&self, slug: &str, id: NoteId, avoid: Option<&str>) -> String {
        let short = id.simple().to_string();
        let short = &short[..8];
        let base = format!("{slug}-{short}");
        let mut candidate = format!("{base}.md");
        if self.candidate_free(&candidate, avoid) {
            return candidate;
        }
        for n in 2..=100 {
            candidate = format!("{base}-{n}.md");
            if self.candidate_free(&candidate, avoid) {
                return candidate;
            }
        }
        format!("{base}-{}.md", id.simple())
    }

    fn candidate_free(&self, candidate: &str, avoid: Option<&str>) -> bool {
        // A note may keep its own current path when its slug does not change.
        if avoid == Some(candidate) {
            return true;
        }
        !self.exists(candidate)
    }
}

#[derive(Debug, Deserialize, Default)]
struct LenientMeta {
    id: Option<Uuid>,
    title: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    pinned: bool,
    created: Option<String>,
    updated: Option<String>,
}

/// Title for a file that has none: first leading `# heading`, else file stem.
pub(crate) fn derive_title(body: &str, rel: &str) -> String {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                return heading.to_string();
            }
        }
        break;
    }
    Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string()
}

/// Turn a title into a filesystem-safe slug (Unicode letters kept).
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() && !is_forbidden(ch) {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else if ch.is_whitespace() || ch == '-' || ch == '_' || ch == '.' {
            pending_dash = true;
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(60).collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        "note".to_string()
    } else {
        out
    }
}

fn is_forbidden(ch: char) -> bool {
    ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

fn is_markdown(name: &str) -> bool {
    name.len() > 3 && name.to_ascii_lowercase().ends_with(".md")
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a sequence",
        Value::Mapping(_) => "a mapping",
        _ => "an unsupported value",
    }
}

/// Insert `key` into a frontmatter mapping.
fn set(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

/// Remove `key` from a frontmatter mapping.
fn unset(mapping: &mut Mapping, key: &str) {
    mapping.remove(Value::String(key.to_string()));
}

fn compose_document(note: &Note) -> Result<String, VaultError> {
    let mut mapping = note.extra.clone();
    set(&mut mapping, "id", Value::String(note.meta.id.to_string()));
    set(
        &mut mapping,
        "title",
        Value::String(note.meta.title.clone()),
    );
    if note.meta.tags.is_empty() {
        unset(&mut mapping, "tags");
    } else {
        set(
            &mut mapping,
            "tags",
            Value::Sequence(
                note.meta
                    .tags
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
    }
    if note.meta.pinned {
        set(&mut mapping, "pinned", Value::Bool(true));
    } else {
        unset(&mut mapping, "pinned");
    }
    match &note.meta.created {
        Some(created) => set(&mut mapping, "created", Value::String(created.clone())),
        None => unset(&mut mapping, "created"),
    }
    match &note.meta.updated {
        Some(updated) => set(&mut mapping, "updated", Value::String(updated.clone())),
        None => unset(&mut mapping, "updated"),
    }

    let mut yaml = serde_yaml_ng::to_string(&mapping).map_err(|e| VaultError::Frontmatter {
        path: PathBuf::from(&note.rel_path),
        message: e.to_string(),
    })?;
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    Ok(format!("---\n{yaml}---\n{}", note.body))
}

fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(".writan-tmp-{}", Uuid::new_v4().simple()));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn mtime_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn normalize_root(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    #[test]
    fn create_write_read_roundtrip() {
        let (_dir, vault) = vault();
        let note = vault
            .create("My First Note!", &["a".into(), "b".into()])
            .expect("create");
        assert!(note.rel_path.starts_with("my-first-note-"));
        assert!(note.rel_path.ends_with(".md"));

        let mut loaded = vault.read(&note.rel_path).expect("read");
        assert_eq!(loaded.meta.id, note.meta.id);
        assert_eq!(loaded.meta.title, "My First Note!");
        assert_eq!(loaded.meta.tags, vec!["a".to_string(), "b".to_string()]);
        assert!(loaded.meta.created.is_some());

        loaded.body = "# Hello\n\nBody text.\n".to_string();
        vault.write(&loaded).expect("write");
        let again = vault.read(&note.rel_path).expect("re-read");
        assert_eq!(again.body, "# Hello\n\nBody text.\n");
    }

    #[test]
    fn unknown_frontmatter_keys_survive_writes() {
        let (_dir, vault) = vault();
        let raw = "---\nid: 11111111-1111-1111-1111-111111111111\ntitle: Old\naliases: [x, y]\ncssclass: fancy\n---\nbody\n";
        std::fs::write(vault.abs_path("note.md"), raw).expect("write raw");

        let mut note = vault.read("note.md").expect("read");
        assert_eq!(note.meta.title, "Old");
        note.meta.pinned = true;
        vault.write(&note).expect("write");

        let text = std::fs::read_to_string(vault.abs_path("note.md")).expect("read raw");
        assert!(text.contains("aliases:"), "{text}");
        assert!(text.contains("cssclass: fancy"), "{text}");
        assert!(text.contains("pinned: true"), "{text}");
        assert!(text.contains("title: Old"), "{text}");
    }

    #[test]
    fn rename_moves_file_and_keeps_id() {
        let (_dir, vault) = vault();
        let note = vault.create("First Title", &[]).expect("create");
        let renamed = vault.rename(&note, "Second Title").expect("rename");
        assert!(renamed.rel_path.starts_with("second-title-"));
        assert_ne!(renamed.rel_path, note.rel_path);
        assert!(!vault.exists(&note.rel_path));
        assert!(vault.exists(&renamed.rel_path));
        let loaded = vault.read(&renamed.rel_path).expect("read");
        assert_eq!(loaded.meta.id, note.meta.id);
        assert_eq!(loaded.meta.title, "Second Title");
    }

    #[test]
    fn rename_with_same_slug_keeps_file() {
        let (_dir, vault) = vault();
        let note = vault.create("Same Slug", &[]).expect("create");
        let renamed = vault.rename(&note, "same slug").expect("rename");
        assert_eq!(renamed.rel_path, note.rel_path);
    }

    #[test]
    fn walk_skips_hidden_and_non_markdown() {
        let (_dir, vault) = vault();
        vault.create("Visible", &[]).expect("create");
        std::fs::create_dir_all(vault.abs_path(".obsidian")).expect("mkdir");
        std::fs::write(vault.abs_path(".obsidian/workspace.json"), "{}").expect("write");
        std::fs::write(vault.abs_path(".hidden.md"), "---\nid: x\n---\n").expect("write");
        std::fs::write(vault.abs_path("notes.txt"), "hi").expect("write");

        let found = vault.walk_notes().expect("walk");
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("visible-"));
    }

    #[test]
    fn parse_file_without_frontmatter_has_no_meta() {
        let (_dir, vault) = vault();
        std::fs::write(vault.abs_path("plain.md"), "# Just a heading\n\ntext\n").expect("write");
        let parsed = vault.parse_file("plain.md").expect("parse");
        assert!(parsed.meta.is_none());
        assert_eq!(parsed.body, "# Just a heading\n\ntext\n");
        assert_eq!(parsed.title, None);
    }

    #[test]
    fn parse_file_with_partial_frontmatter_keeps_fields_for_adoption() {
        let (_dir, vault) = vault();
        std::fs::write(
            vault.abs_path("partial.md"),
            "---\ntitle: Kept Tag\ntags: [one, two]\npinned: true\n---\nbody\n",
        )
        .expect("write");
        let parsed = vault.parse_file("partial.md").expect("parse");
        assert!(parsed.meta.is_none());
        assert_eq!(parsed.title.as_deref(), Some("Kept Tag"));
        assert_eq!(parsed.tags, vec!["one".to_string(), "two".to_string()]);
        assert!(parsed.pinned);
    }

    #[test]
    fn malformed_frontmatter_is_an_error() {
        let (_dir, vault) = vault();
        std::fs::write(
            vault.abs_path("bad.md"),
            "---\n- just\n- a list\n---\nbody\n",
        )
        .expect("write");
        assert!(matches!(
            vault.parse_file("bad.md"),
            Err(VaultError::Frontmatter { .. })
        ));
    }

    #[test]
    fn crlf_note_reads_back() {
        let (_dir, vault) = vault();
        let raw =
            "---\r\nid: 22222222-2222-2222-2222-222222222222\r\ntitle: CRLF\r\n---\r\nbody\r\n";
        std::fs::write(vault.abs_path("crlf.md"), raw).expect("write");
        let note = vault.read("crlf.md").expect("read");
        assert_eq!(note.meta.title, "CRLF");
        assert_eq!(note.body, "body\r\n");
    }

    #[test]
    fn no_temp_files_left_behind() {
        let (_dir, vault) = vault();
        let note = vault.create("Temp Check", &[]).expect("create");
        vault.write(&note).expect("write");
        let entries: Vec<_> = std::fs::read_dir(vault.root())
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 1, "{entries:?}");
    }

    #[test]
    fn slugify_rules() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("a/b:c*d?"), "abcd");
        assert_eq!(slugify("  ...  "), "note");
        assert_eq!(slugify("日本語 ノート"), "日本語-ノート");
        assert_eq!(slugify("Ünïcode Text"), "ünïcode-text");
    }

    #[test]
    fn derive_title_prefers_heading() {
        assert_eq!(derive_title("# My Title\n\nbody", "x.md"), "My Title");
        assert_eq!(derive_title("## Sub\n\nbody", "x.md"), "Sub");
        assert_eq!(derive_title("plain text", "file-name.md"), "file-name");
        assert_eq!(derive_title("", "other.md"), "other");
    }
}
