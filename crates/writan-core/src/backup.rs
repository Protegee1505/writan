//! Backup archives: the vault as one zip (notes plus everything they link
//! to), and the import side that only ever adds files.
//!
//! The archive is a plain copy of what is already on disk — no index, no app
//! state, no encryption — so any zip tool can read it. Import accepts a Writan
//! archive, any folder of Markdown files, or a single `.md` file.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::vault::{Vault, VaultError, derive_title, is_markdown, slugify};
use crate::{Note, NoteMeta, now_rfc3339};

/// Manifest at the archive root. It is informational: import never trusts it.
pub const BACKUP_MANIFEST: &str = "writan-backup.json";
pub const BACKUP_FORMAT: &str = "writan-backup/1";
/// Refuse archives with more entries than this (zip bombs, runaway folders).
pub const MAX_ARCHIVE_ENTRIES: usize = 100_000;
/// Refuse archives/folders whose contents expand beyond this.
pub const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// One oversized file is skipped (and reported), not imported.
pub const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

const ZIP_MODE: u32 = 0o644;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("vault error: {0}")]
    Vault(#[from] VaultError),
    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("archive is encrypted; Writan backups are not encrypted yet")]
    Encrypted,
    #[error("archive has too many entries")]
    TooManyEntries,
    #[error("contents expand beyond the {} MB backup limit", MAX_ARCHIVE_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("destination {0} is inside the vault")]
    DestinationInsideVault(PathBuf),
    #[error("source {0} is inside the vault")]
    SourceInsideVault(PathBuf),
    #[error("destination {0} is not empty; pick an empty folder")]
    DestinationNotEmpty(PathBuf),
    #[error("destination {0} is not a folder")]
    NotAFolder(PathBuf),
    #[error("note not found: {0}")]
    NoteNotFound(String),
}

fn io_err(path: impl Into<PathBuf>, source: io::Error) -> BackupError {
    BackupError::Io {
        path: path.into(),
        source,
    }
}

/// What an export wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub notes: usize,
    pub attachments: usize,
    pub skipped: usize,
    pub bytes: u64,
}

/// What an import added. `renamed` counts notes that got a fresh id because
/// their id was already taken (they keep their words, not their identity).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub notes: usize,
    pub attachments: usize,
    pub renamed: usize,
    pub skipped: usize,
    pub bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    created: String,
    app_version: String,
    notes: usize,
    attachments: usize,
    bytes: u64,
}

struct VaultFile {
    rel: String,
    abs: PathBuf,
    size: u64,
}

/// Export the whole vault as a zip at `dest`.
pub fn export_archive(
    vault_root: impl AsRef<Path>,
    dest: impl AsRef<Path>,
) -> Result<ExportReport, BackupError> {
    let vault = Vault::open(vault_root)?;
    let dest = absolute(dest.as_ref())?;
    ensure_outside(vault.root(), &dest)?;
    let (files, skipped) = collect_files(vault.root())?;
    let report = export_report(&files, skipped);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    let tmp = temp_sibling(&dest, "backup");
    let result = write_zip(&tmp, &files, &report);
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    replace_file(&tmp, &dest)?;
    Ok(report)
}

/// Export the whole vault as a folder at `dest`. A non-empty destination is
/// only reused when it already holds a Writan backup manifest.
pub fn export_folder(
    vault_root: impl AsRef<Path>,
    dest: impl AsRef<Path>,
) -> Result<ExportReport, BackupError> {
    let vault = Vault::open(vault_root)?;
    let dest = absolute(dest.as_ref())?;
    ensure_outside(vault.root(), &dest)?;
    if dest.exists() {
        if !dest.is_dir() {
            return Err(BackupError::NotAFolder(dest));
        }
        let has_manifest = dest.join(BACKUP_MANIFEST).is_file();
        let mut entries = fs::read_dir(&dest).map_err(|e| io_err(&dest, e))?;
        if entries.next().is_some() && !has_manifest {
            return Err(BackupError::DestinationNotEmpty(dest));
        }
    }
    let (files, skipped) = collect_files(vault.root())?;
    let report = export_report(&files, skipped);
    fs::create_dir_all(&dest).map_err(|e| io_err(&dest, e))?;
    for file in &files {
        let target = dest.join(rel_to_path(&file.rel));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        fs::copy(&file.abs, &target).map_err(|e| io_err(&target, e))?;
    }
    let manifest = manifest_for(&report);
    write_json_file(&dest.join(BACKUP_MANIFEST), &manifest)?;
    Ok(report)
}

/// Copy one note's Markdown file to `dest`, frontmatter included.
pub fn export_note(
    vault_root: impl AsRef<Path>,
    rel_path: &str,
    dest: impl AsRef<Path>,
) -> Result<u64, BackupError> {
    let vault = Vault::open(vault_root)?;
    let dest = absolute(dest.as_ref())?;
    ensure_outside(vault.root(), &dest)?;
    let source = vault.abs_path(rel_path);
    if !source.is_file() {
        return Err(BackupError::NoteNotFound(rel_path.to_string()));
    }
    let bytes = fs::read(&source).map_err(|e| io_err(&source, e))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    atomic_write(&dest, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Import a zip archive, a folder of Markdown files, or one `.md` file.
pub fn import_path(
    vault_root: impl AsRef<Path>,
    source: impl AsRef<Path>,
) -> Result<ImportReport, BackupError> {
    let source = absolute(source.as_ref())?;
    if source.is_dir() {
        return import_folder(vault_root, &source);
    }
    let is_markdown_file = source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if is_markdown_file {
        return import_markdown_file(vault_root, &source);
    }
    import_archive(vault_root, &source)
}

/// Import every file from a zip. Existing files are never overwritten.
pub fn import_archive(
    vault_root: impl AsRef<Path>,
    archive: impl AsRef<Path>,
) -> Result<ImportReport, BackupError> {
    let archive = absolute(archive.as_ref())?;
    let file = File::open(&archive).map_err(|e| io_err(&archive, e))?;
    let mut zip = ZipArchive::new(file)?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(BackupError::TooManyEntries);
    }
    // Inspect every entry before writing anything, so a hostile archive is
    // rejected without leaving half an import behind.
    let mut declared = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        if entry.encrypted() {
            return Err(BackupError::Encrypted);
        }
        declared = declared.saturating_add(entry.size());
        if declared > MAX_ARCHIVE_BYTES {
            return Err(BackupError::TooLarge);
        }
    }

    let mut state = ImportState::new(vault_root)?;
    let mut report = ImportReport::default();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            report.skipped += 1;
            continue;
        }
        let Ok(name) = entry.name() else {
            report.skipped += 1;
            continue;
        };
        let Some(rel) = safe_rel_path(&name) else {
            report.skipped += 1;
            continue;
        };
        if is_junk(&rel) {
            report.skipped += 1;
            continue;
        }
        if entry.size() > MAX_ENTRY_BYTES {
            report.skipped += 1;
            continue;
        }
        let bytes = read_limited(&mut entry, MAX_ENTRY_BYTES)?;
        state.import_file(&rel, &bytes, &mut report)?;
    }
    Ok(report)
}

/// Import every file from a folder tree. Hidden files, symlinks and OS junk
/// are skipped, matching what [`export_folder`] writes.
pub fn import_folder(
    vault_root: impl AsRef<Path>,
    source: impl AsRef<Path>,
) -> Result<ImportReport, BackupError> {
    let source = absolute(source.as_ref())?;
    let vault = Vault::open(vault_root)?;
    if source.starts_with(vault.root()) {
        return Err(BackupError::SourceInsideVault(source));
    }
    let (files, skipped) = collect_files(&source)?;
    let mut state = ImportState::new(vault.root())?;
    let mut report = ImportReport {
        skipped,
        ..Default::default()
    };
    for file in files {
        let bytes = fs::read(&file.abs).map_err(|e| io_err(&file.abs, e))?;
        state.import_file(&file.rel, &bytes, &mut report)?;
    }
    Ok(report)
}

fn import_markdown_file(
    vault_root: impl AsRef<Path>,
    source: &Path,
) -> Result<ImportReport, BackupError> {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BackupError::NoteNotFound(source.display().to_string()))?;
    let rel = safe_rel_path(name).unwrap_or_else(|| "imported.md".to_string());
    let bytes = fs::read(source).map_err(|e| io_err(source, e))?;
    let mut state = ImportState::new(vault_root)?;
    let mut report = ImportReport::default();
    state.import_file(&rel, &bytes, &mut report)?;
    Ok(report)
}

struct ImportState {
    vault: Vault,
    ids: HashSet<Uuid>,
    total: u64,
}

impl ImportState {
    fn new(vault_root: impl AsRef<Path>) -> Result<Self, BackupError> {
        let vault = Vault::open(vault_root)?;
        let mut ids = HashSet::new();
        for rel in vault.walk_notes()? {
            if let Ok(parsed) = vault.parse_file(&rel)
                && let Some(meta) = parsed.meta
            {
                ids.insert(meta.id);
            }
        }
        Ok(Self {
            vault,
            ids,
            total: 0,
        })
    }

    fn import_file(
        &mut self,
        rel: &str,
        bytes: &[u8],
        report: &mut ImportReport,
    ) -> Result<(), BackupError> {
        self.total = self.total.saturating_add(bytes.len() as u64);
        if self.total > MAX_ARCHIVE_BYTES {
            return Err(BackupError::TooLarge);
        }
        if is_note(rel) {
            self.import_note(rel, bytes, report)
        } else {
            self.import_attachment(rel, bytes, report)
        }
    }

    fn import_note(
        &mut self,
        rel: &str,
        bytes: &[u8],
        report: &mut ImportReport,
    ) -> Result<(), BackupError> {
        let Ok(text) = std::str::from_utf8(bytes) else {
            report.skipped += 1;
            return Ok(());
        };
        let parsed = match Vault::parse_text(rel, text) {
            Ok(parsed) => parsed,
            Err(_) => {
                report.skipped += 1;
                return Ok(());
            }
        };
        let source_meta = parsed.meta.as_ref();
        let mut id = source_meta.map(|meta| meta.id).unwrap_or_else(Uuid::new_v4);
        let mut title = source_meta
            .map(|meta| meta.title.clone())
            .or_else(|| parsed.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| derive_title(&parsed.body, rel));
        if let Some(meta) = source_meta
            && self.ids.contains(&meta.id)
        {
            // Same identity already lives here (re-import): keep the words,
            // give the copy a fresh id and a title that says so.
            id = Uuid::new_v4();
            title = imported_title(&title);
            report.renamed += 1;
        }
        let now = now_rfc3339();
        let note = Note {
            meta: NoteMeta {
                id,
                title: title.clone(),
                tags: parsed.tags,
                pinned: parsed.pinned,
                created: source_meta
                    .and_then(|meta| meta.created.clone())
                    .or(Some(now.clone())),
                updated: source_meta
                    .and_then(|meta| meta.updated.clone())
                    .or(Some(now)),
            },
            body: parsed.body,
            rel_path: unique_note_path(&self.vault, rel, &title, id),
            extra: parsed.extra,
        };
        self.vault.write(&note)?;
        self.ids.insert(id);
        report.notes += 1;
        report.bytes = report.bytes.saturating_add(bytes.len() as u64);
        Ok(())
    }

    fn import_attachment(
        &mut self,
        rel: &str,
        bytes: &[u8],
        report: &mut ImportReport,
    ) -> Result<(), BackupError> {
        let target = unique_asset_path(&self.vault, rel);
        let path = self.vault.abs_path(&target);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        atomic_write(&path, bytes)?;
        report.attachments += 1;
        report.bytes = report.bytes.saturating_add(bytes.len() as u64);
        Ok(())
    }
}

fn export_report(files: &[VaultFile], skipped: usize) -> ExportReport {
    let notes = files.iter().filter(|file| is_note(&file.rel)).count();
    ExportReport {
        notes,
        attachments: files.len() - notes,
        skipped,
        bytes: files.iter().map(|file| file.size).sum(),
    }
}

fn manifest_for(report: &ExportReport) -> Manifest {
    Manifest {
        format: BACKUP_FORMAT.to_string(),
        created: now_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        notes: report.notes,
        attachments: report.attachments,
        bytes: report.bytes,
    }
}

fn write_zip(path: &Path, files: &[VaultFile], report: &ExportReport) -> Result<(), BackupError> {
    let file = File::create(path).map_err(|e| io_err(path, e))?;
    let mut zip = ZipWriter::new(file);
    let manifest = manifest_for(report);
    zip.start_file(BACKUP_MANIFEST, options_for(BACKUP_MANIFEST))?;
    serde_json::to_writer_pretty(&mut zip, &manifest)
        .map_err(|e| io_err(path, io::Error::other(e)))?;
    zip.write_all(b"\n").map_err(|e| io_err(path, e))?;
    for file in files {
        zip.start_file(&file.rel, options_for(&file.rel))?;
        let mut source = File::open(&file.abs).map_err(|e| io_err(&file.abs, e))?;
        io::copy(&mut source, &mut zip).map_err(|e| io_err(&file.abs, e))?;
    }
    let file = zip.finish()?;
    file.sync_all().map_err(|e| io_err(path, e))?;
    Ok(())
}

fn options_for(name: &str) -> SimpleFileOptions {
    let method = if is_text(name) {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    };
    SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(ZIP_MODE)
}

/// Text formats compress well; media is already compressed.
fn is_text(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "txt"
            | "csv"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "html"
            | "css"
            | "js"
            | "ts"
            | "svg"
    )
}

/// Walk `root` for files a backup should carry: no dotfiles, no symlinks, no
/// OS droppings. Counts what it skips instead of failing.
fn collect_files(root: &Path) -> Result<(Vec<VaultFile>, usize), BackupError> {
    let mut files = Vec::new();
    let mut skipped = 0usize;
    walk_files(root, root, &mut files, &mut skipped)?;
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    if files.len() > MAX_ARCHIVE_ENTRIES {
        return Err(BackupError::TooManyEntries);
    }
    let total: u64 = files.iter().map(|file| file.size).sum();
    if total > MAX_ARCHIVE_BYTES {
        return Err(BackupError::TooLarge);
    }
    Ok((files, skipped))
}

fn walk_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<VaultFile>,
    skipped: &mut usize,
) -> Result<(), BackupError> {
    let entries = fs::read_dir(dir).map_err(|e| io_err(dir, e))?;
    let mut entries: Vec<_> = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| io_err(dir, e))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            *skipped += 1;
            continue;
        };
        let file_type = entry.file_type().map_err(|e| io_err(&path, e))?;
        // The manifest is app bookkeeping, not content: ignore it quietly.
        if dir == root && name == BACKUP_MANIFEST {
            continue;
        }
        if name.starts_with('.') || file_type.is_symlink() || is_junk_name(name) {
            *skipped += 1;
            continue;
        }
        if file_type.is_dir() {
            if name == "__MACOSX" {
                *skipped += 1;
                continue;
            }
            walk_files(root, &path, out, skipped)?;
        } else if file_type.is_file() {
            let Some(rel) = rel_path(root, &path) else {
                *skipped += 1;
                continue;
            };
            let size = entry.metadata().map_err(|e| io_err(&path, e))?.len();
            out.push(VaultFile {
                rel,
                abs: path,
                size,
            });
        } else {
            *skipped += 1;
        }
    }
    Ok(())
}

fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(part) => Some(part.to_str().unwrap_or("")),
            _ => None,
        })
        .collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn rel_to_path(rel: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

/// Paths from an archive, normalized and made safe to join onto the vault.
///
/// Traversal, absolute paths and dotfiles are rejected; characters that are
/// invalid on Windows are replaced so the result is portable. Returns `None`
/// when nothing usable is left.
fn safe_rel_path(raw: &str) -> Option<String> {
    let raw = raw.replace('\\', "/");
    if raw.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.starts_with('.') {
            return None;
        }
        let part = sanitize_component(part)?;
        parts.push(part);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn sanitize_component(part: &str) -> Option<String> {
    let trimmed = part.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_control() || matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim();
    if out.is_empty() || out == "." || out == ".." {
        return None;
    }
    let mut out: String = out.chars().take(120).collect();
    if is_reserved_name(&out) {
        out.insert(0, '_');
    }
    Some(out)
}

fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_junk_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "thumbs.db" | "desktop.ini" | ".ds_store"
    )
}

fn is_junk(rel: &str) -> bool {
    if rel == BACKUP_MANIFEST {
        return true;
    }
    if rel.split('/').next() == Some("__MACOSX") {
        return true;
    }
    rel.rsplit('/').next().is_some_and(is_junk_name)
}

fn is_note(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    is_markdown(name)
}

fn imported_title(title: &str) -> String {
    const SUFFIX: &str = " (imported)";
    let title = title.trim();
    if title.is_empty() {
        return "Imported note".to_string();
    }
    if title.ends_with(SUFFIX) {
        title.to_string()
    } else {
        format!("{title}{SUFFIX}")
    }
}

fn unique_note_path(vault: &Vault, source_rel: &str, title: &str, id: Uuid) -> String {
    let dir = source_rel.rsplit_once('/').map(|(dir, _)| dir);
    let prefix = dir.map(|dir| format!("{dir}/")).unwrap_or_default();
    let slug = slugify(title);
    let short: String = id.simple().to_string().chars().take(8).collect();
    let base = format!("{prefix}{slug}-{short}");
    let candidate = format!("{base}.md");
    if !vault.abs_path(&candidate).exists() {
        return candidate;
    }
    for n in 2..=100 {
        let candidate = format!("{base}-{n}.md");
        if !vault.abs_path(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{}.md", id.simple())
}

fn unique_asset_path(vault: &Vault, rel: &str) -> String {
    if !vault.abs_path(rel).exists() {
        return rel.to_string();
    }
    let (dir, name) = match rel.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, rel),
    };
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    for n in 2..=1000 {
        let candidate_name = match ext {
            Some(ext) => format!("{stem}-{n}.{ext}"),
            None => format!("{stem}-{n}"),
        };
        let candidate = match dir {
            Some(dir) => format!("{dir}/{candidate_name}"),
            None => candidate_name,
        };
        if !vault.abs_path(&candidate).exists() {
            return candidate;
        }
    }
    let unique = Uuid::new_v4().simple().to_string();
    let suffix: String = unique.chars().take(8).collect();
    match (dir, ext) {
        (Some(dir), Some(ext)) => format!("{dir}/{stem}-{suffix}.{ext}"),
        (Some(dir), None) => format!("{dir}/{stem}-{suffix}"),
        (None, Some(ext)) => format!("{stem}-{suffix}.{ext}"),
        (None, None) => format!("{stem}-{suffix}"),
    }
}

fn read_limited(reader: &mut impl Read, limit: u64) -> Result<Vec<u8>, BackupError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| io_err("<archive entry>", e))?;
    if bytes.len() as u64 > limit {
        return Err(BackupError::TooLarge);
    }
    Ok(bytes)
}

fn write_json_file(path: &Path, manifest: &Manifest) -> Result<(), BackupError> {
    let mut bytes =
        serde_json::to_vec_pretty(manifest).map_err(|e| io_err(path, io::Error::other(e)))?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    let tmp = temp_sibling(path, "write");
    {
        let mut file = File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
        file.write_all(bytes).map_err(|e| io_err(&tmp, e))?;
        file.sync_all().map_err(|e| io_err(&tmp, e))?;
    }
    replace_file(&tmp, path)
}

fn temp_sibling(dest: &Path, tag: &str) -> PathBuf {
    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(format!(".writan-{tag}-{}.tmp", Uuid::new_v4().simple()))
}

fn replace_file(tmp: &Path, dest: &Path) -> Result<(), BackupError> {
    if dest.exists() {
        fs::remove_file(dest).map_err(|e| io_err(dest, e))?;
    }
    fs::rename(tmp, dest).map_err(|e| io_err(dest, e))
}

fn absolute(path: &Path) -> Result<PathBuf, BackupError> {
    std::path::absolute(path).map_err(|e| io_err(path, e))
}

fn ensure_outside(vault_root: &Path, dest: &Path) -> Result<(), BackupError> {
    if dest.starts_with(vault_root) {
        return Err(BackupError::DestinationInsideVault(dest.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn put_note(vault: &Vault, title: &str, rel: &str, body: &str) -> Note {
        let note = Note {
            meta: NoteMeta {
                id: Uuid::new_v4(),
                title: title.to_string(),
                tags: vec!["tag".to_string()],
                pinned: false,
                created: Some(now_rfc3339()),
                updated: Some(now_rfc3339()),
            },
            body: body.to_string(),
            rel_path: rel.to_string(),
            extra: Map::new(),
        };
        vault.write(&note).expect("write note");
        note
    }

    fn sample_vault(dir: &Path) {
        let vault = Vault::open(dir).expect("vault");
        put_note(
            &vault,
            "First Note",
            "first-note.md",
            "# First\n\nHello 世界\n",
        );
        put_note(
            &vault,
            "Nested",
            "projects/nested.md",
            "nested body with agenda\n",
        );
        fs::create_dir_all(dir.join("attachments")).expect("attachments");
        fs::write(dir.join("attachments/pic.png"), b"\x89PNG\r\n\x1a\n").expect("asset");
        fs::write(dir.join(".DS_Store"), b"junk").expect("junk");
        fs::write(dir.join("Thumbs.db"), b"junk").expect("junk");
    }

    fn notes_under(root: &Path) -> Vec<(String, Note)> {
        let vault = Vault::open(root).expect("vault");
        vault
            .walk_notes()
            .expect("walk")
            .into_iter()
            .filter_map(|rel| vault.read(&rel).ok().map(|note| (rel, note)))
            .collect()
    }

    #[test]
    fn archive_round_trip_keeps_notes_and_attachments() {
        let source = tempfile::tempdir().expect("source");
        let out = tempfile::tempdir().expect("out");
        sample_vault(source.path());
        let archive = out.path().join("backup.zip");
        let export = export_archive(source.path(), &archive).expect("export");
        assert_eq!(export.notes, 2);
        assert_eq!(export.attachments, 1);
        assert_eq!(export.skipped, 2);
        assert!(export.bytes > 0);

        let zip_file = File::open(&archive).expect("archive");
        let mut zip = ZipArchive::new(zip_file).expect("zip");
        let mut manifest = String::new();
        zip.by_name(BACKUP_MANIFEST)
            .expect("manifest")
            .read_to_string(&mut manifest)
            .expect("read manifest");
        assert!(manifest.contains(BACKUP_FORMAT), "{manifest}");
        assert!(manifest.contains("\"appVersion\""), "{manifest}");
        drop(zip);

        let target = tempfile::tempdir().expect("target");
        let import = import_archive(target.path(), &archive).expect("import");
        assert_eq!(import.notes, 2);
        assert_eq!(import.attachments, 1);
        assert_eq!(import.renamed, 0);
        assert_eq!(
            fs::read(target.path().join("attachments/pic.png")).expect("asset"),
            b"\x89PNG\r\n\x1a\n"
        );
        let notes = notes_under(target.path());
        assert_eq!(notes.len(), 2);
        let first = notes
            .iter()
            .find(|(_, note)| note.meta.title == "First Note")
            .expect("first note");
        assert!(first.1.body.contains("Hello 世界"), "{}", first.1.body);
        assert!(
            notes
                .iter()
                .any(|(rel, note)| { note.meta.title == "Nested" && rel.starts_with("projects/") })
        );
    }

    #[test]
    fn importing_the_same_notes_again_adds_copies() {
        let source = tempfile::tempdir().expect("source");
        let out = tempfile::tempdir().expect("out");
        sample_vault(source.path());
        let archive = out.path().join("backup.zip");
        export_archive(source.path(), &archive).expect("export");

        let first = import_archive(source.path(), &archive).expect("first import");
        assert_eq!(first.notes, 2);
        assert_eq!(first.renamed, 2);
        let second = import_archive(source.path(), &archive).expect("second import");
        assert_eq!(second.notes, 2);
        assert_eq!(second.renamed, 2);

        let notes = notes_under(source.path());
        assert_eq!(notes.len(), 6);
        let imported: Vec<_> = notes
            .iter()
            .filter(|(_, note)| note.meta.title.ends_with(" (imported)"))
            .collect();
        assert_eq!(imported.len(), 4);
        for (_, note) in imported {
            assert!(!note.meta.title.ends_with(" (imported) (imported)"));
        }
        let mut ids: Vec<_> = notes.iter().map(|(_, note)| note.meta.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn hostile_archive_paths_stay_inside_the_vault() {
        let out = tempfile::tempdir().expect("out");
        let archive = out.path().join("hostile.zip");
        {
            let file = File::create(&archive).expect("archive");
            let mut zip = ZipWriter::new(file);
            let options = SimpleZipOptions::default();
            for (name, body) in [
                ("../evil.md", "# Escape"),
                ("/abs.md", "# Absolute"),
                ("C:/drive.md", "# Drive"),
                ("a/../../escape.md", "# Escape"),
                (".hidden.md", "# Hidden"),
                ("Thumbs.db", "junk"),
                ("notes/hello.md", "# Hello\n\nbody text\n"),
                ("attachments/pic.bin", "binary"),
            ] {
                zip.start_file(name, options).expect("entry");
                zip.write_all(body.as_bytes()).expect("body");
            }
            zip.finish().expect("finish");
        }
        let target = tempfile::tempdir().expect("target");
        let import = import_archive(target.path(), &archive).expect("import");
        assert_eq!(import.notes, 2, "{import:?}");
        assert_eq!(import.attachments, 1, "{import:?}");
        assert_eq!(import.skipped, 5, "{import:?}");
        assert!(!out.path().join("evil.md").exists());
        assert!(!out.path().join("abs.md").exists());
        assert!(!target.path().join("..").join("evil.md").exists());
        let notes = notes_under(target.path());
        let titles: Vec<_> = notes
            .iter()
            .map(|(_, note)| note.meta.title.clone())
            .collect();
        assert!(titles.contains(&"Hello".to_string()), "{titles:?}");
        assert!(notes.iter().all(|(rel, _)| !rel.contains("..")));
    }

    #[test]
    fn foreign_markdown_gets_an_identity() {
        let out = tempfile::tempdir().expect("out");
        let source = out.path().join("plain note.md");
        fs::write(&source, "# Plain\n\njust words\n").expect("write");
        let target = tempfile::tempdir().expect("target");
        let import = import_path(target.path(), &source).expect("import");
        assert_eq!(import.notes, 1);
        assert_eq!(import.renamed, 0);
        let notes = notes_under(target.path());
        assert_eq!(notes.len(), 1);
        let (rel, note) = &notes[0];
        assert!(rel.ends_with(".md"));
        assert_eq!(note.meta.title, "Plain");
        assert!(note.body.contains("just words"));
    }

    #[test]
    fn import_only_adds_and_never_overwrites() {
        let target = tempfile::tempdir().expect("target");
        fs::write(target.path().join("a.txt"), b"original").expect("original");
        let out = tempfile::tempdir().expect("out");
        let archive = out.path().join("assets.zip");
        {
            let file = File::create(&archive).expect("archive");
            let mut zip = ZipWriter::new(file);
            zip.start_file("a.txt", SimpleZipOptions::default())
                .expect("entry");
            zip.write_all(b"imported").expect("body");
            zip.finish().expect("finish");
        }
        let import = import_archive(target.path(), &archive).expect("import");
        assert_eq!(import.attachments, 1);
        assert_eq!(
            fs::read(target.path().join("a.txt")).expect("read"),
            b"original"
        );
        assert_eq!(
            fs::read(target.path().join("a-2.txt")).expect("read"),
            b"imported"
        );
    }

    #[test]
    fn folder_export_and_import_round_trip() {
        let source = tempfile::tempdir().expect("source");
        let out = tempfile::tempdir().expect("out");
        sample_vault(source.path());
        let folder = out.path().join("copy");
        let export = export_folder(source.path(), &folder).expect("export");
        assert_eq!(export.notes, 2);
        assert!(folder.join(BACKUP_MANIFEST).is_file());

        let target = tempfile::tempdir().expect("target");
        let import = import_folder(target.path(), &folder).expect("import");
        assert_eq!(import.notes, 2);
        assert_eq!(import.attachments, 1);
        assert_eq!(import.skipped, 0);
        assert!(target.path().join("projects").is_dir());
    }

    #[test]
    fn export_note_writes_a_markdown_copy() {
        let source = tempfile::tempdir().expect("source");
        let out = tempfile::tempdir().expect("out");
        sample_vault(source.path());
        let dest = out.path().join("share.md");
        let bytes = export_note(source.path(), "first-note.md", &dest).expect("export note");
        assert!(bytes > 0);
        let text = fs::read_to_string(&dest).expect("read");
        assert!(text.contains("Hello 世界"), "{text}");
        assert!(text.starts_with("---\n"), "{text}");
    }

    #[test]
    fn refuses_to_write_a_backup_inside_the_vault() {
        let source = tempfile::tempdir().expect("source");
        sample_vault(source.path());
        let error = export_archive(source.path(), source.path().join("backup.zip"))
            .expect_err("must refuse");
        assert!(matches!(error, BackupError::DestinationInsideVault(_)));
        let error =
            export_folder(source.path(), source.path().join("copy")).expect_err("must refuse");
        assert!(matches!(error, BackupError::DestinationInsideVault(_)));
    }

    #[test]
    fn refuses_a_non_empty_folder_that_is_not_a_backup() {
        let source = tempfile::tempdir().expect("source");
        let out = tempfile::tempdir().expect("out");
        sample_vault(source.path());
        fs::write(out.path().join("other.txt"), b"keep me").expect("write");
        let error = export_folder(source.path(), out.path()).expect_err("must refuse");
        assert!(matches!(error, BackupError::DestinationNotEmpty(_)));
        assert_eq!(
            fs::read(out.path().join("other.txt")).expect("read"),
            b"keep me"
        );
    }

    #[test]
    fn path_safety_normalizes_and_rejects() {
        assert_eq!(safe_rel_path("../evil.md"), None);
        assert_eq!(safe_rel_path("/absolute.md"), None);
        assert_eq!(safe_rel_path("a/../../evil.md"), None);
        assert_eq!(safe_rel_path(".hidden/file.md"), None);
        assert_eq!(safe_rel_path("a\\b.md").as_deref(), Some("a/b.md"));
        assert_eq!(safe_rel_path("C:/evil.md").as_deref(), Some("C-/evil.md"));
        assert_eq!(safe_rel_path("a//b.md").as_deref(), Some("a/b.md"));
        assert_eq!(safe_rel_path("."), None);
        assert_eq!(safe_rel_path(" "), None);
        assert_eq!(safe_rel_path("CON.md").as_deref(), Some("_CON.md"));
        assert_eq!(safe_rel_path("a:b.png").as_deref(), Some("a-b.png"));
    }

    #[test]
    fn read_limited_stops_at_the_cap() {
        let mut exact = io::Cursor::new(vec![0u8; 32]);
        assert_eq!(read_limited(&mut exact, 32).expect("exact").len(), 32);
        let mut too_big = io::Cursor::new(vec![0u8; 33]);
        assert!(matches!(
            read_limited(&mut too_big, 32),
            Err(BackupError::TooLarge)
        ));
    }

    // Kept local so the tests read like the writer API.
    use zip::write::SimpleFileOptions as SimpleZipOptions;
}
