//! Vault attachments: files notes link to, kept in a vault-level
//! `attachments/` folder. Names are deterministic (`<slug>-<hash8>.<ext>`),
//! so storing the same bytes twice reuses one file.

use std::path::{Path, PathBuf};

use crate::vault::slugify;

/// Refuse anything larger, so one attachment cannot stall the shell.
pub const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("attachment is empty")]
    Empty,
    #[error("attachment is larger than {} MB", MAX_ATTACHMENT_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Deterministic attachment file name: `<slug>-<hash8>.<ext>`.
pub fn attachment_name(file_name: &str, bytes: &[u8]) -> String {
    let path = Path::new(file_name);
    let mut stem = slugify(
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("file"),
    );
    if stem.is_empty() {
        stem = "file".to_string();
    }
    let ext: String = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let hash = blake3::hash(bytes);
    let short = &hash.to_hex()[..8];
    if ext.is_empty() {
        format!("{stem}-{short}")
    } else {
        format!("{stem}-{short}.{ext}")
    }
}

/// Store `bytes` as `attachments/<name>` under `vault_root` and return the
/// vault-relative path. An existing file is kept, so identical content maps
/// to the same file.
pub fn write_attachment(
    vault_root: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<String, AttachmentError> {
    if bytes.is_empty() {
        return Err(AttachmentError::Empty);
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentError::TooLarge);
    }
    let name = attachment_name(file_name, bytes);
    let dir = vault_root.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|source| AttachmentError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(&name);
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|source| AttachmentError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(format!("attachments/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "writan-attachments-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn names_are_slugged_hashed_and_deterministic() {
        let name = attachment_name("My Photo.PNG", b"hello");
        assert!(name.starts_with("my-photo-"), "got {name}");
        assert!(name.ends_with(".png"), "got {name}");
        assert_eq!(name, attachment_name("My Photo.PNG", b"hello"));
        assert_ne!(name, attachment_name("My Photo.PNG", b"other"));
        assert!(attachment_name("", b"x").starts_with("file-"));
        assert!(!attachment_name("weird.na me!", b"x").contains(' '));
        assert!(!attachment_name("no-extension", b"x").contains('.'));
    }

    #[test]
    fn writes_into_attachments_and_reuses_identical_files() {
        let root = temp_root("reuse");
        let first = write_attachment(&root, "note.txt", b"same").expect("write");
        let second = write_attachment(&root, "note.txt", b"same").expect("write again");
        assert_eq!(first, second);
        assert_eq!(std::fs::read(root.join(&first)).expect("read"), b"same");
        let other = write_attachment(&root, "note.txt", b"different").expect("write other");
        assert_ne!(first, other);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_empty_and_oversize_payloads() {
        let root = temp_root("empty");
        assert!(matches!(
            write_attachment(&root, "a.png", b""),
            Err(AttachmentError::Empty)
        ));
        let huge = vec![0u8; MAX_ATTACHMENT_BYTES + 1];
        assert!(matches!(
            write_attachment(&root, "a.bin", &huge),
            Err(AttachmentError::TooLarge)
        ));
        assert!(!root.exists(), "no folder is created for refused payloads");
    }
}
