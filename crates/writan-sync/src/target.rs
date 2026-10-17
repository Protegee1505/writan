//! Sync targets: dumb storage the engine reads and writes.
//!
//! v1 ships the folder target — any directory, local or mounted — and S3 and
//! writan-server targets reuse the same trait later. A target is "dumb": it
//! stores create-only blob files named per ADR-0005 and knows nothing about
//! notes, vectors or their contents.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Errors a target can raise.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    #[error("target I/O: {0}")]
    Io(#[from] io::Error),
    #[error("target path `{0}` escapes the target root")]
    Escape(String),
    #[error("target blob `{0}` already exists with different bytes")]
    Diverged(String),
}

/// A blob store the engine reads and writes.
pub trait SyncTarget {
    /// Every stored file, forward-slashed, sorted, relative to the root.
    fn list(&self) -> Result<Vec<String>, TargetError>;
    /// The bytes stored at `path` (a path returned by [`SyncTarget::list`]).
    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError>;
    /// Store `bytes` at `path`. Create-only: an existing blob is kept —
    /// identical bytes are accepted (a retried write), different bytes are
    /// [`TargetError::Diverged`].
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError>;
}

/// A sync target rooted at a directory.
#[derive(Debug, Clone)]
pub struct FolderTarget {
    root: PathBuf,
}

impl FolderTarget {
    /// Open (creating if needed) a directory as a target.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, TargetError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// The directory this target stores blobs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, TargetError> {
        let mut full = self.root.clone();
        for part in path.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(TargetError::Escape(path.to_string()));
            }
            full.push(part);
        }
        if !full.starts_with(&self.root) {
            return Err(TargetError::Escape(path.to_string()));
        }
        Ok(full)
    }
}

impl SyncTarget for FolderTarget {
    fn list(&self) -> Result<Vec<String>, TargetError> {
        let mut paths = Vec::new();
        collect(&self.root, &self.root, &mut paths)?;
        paths.sort();
        Ok(paths)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError> {
        Ok(fs::read(self.resolve(path)?)?)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if fs::read(&full)? == bytes {
                    Ok(())
                } else {
                    Err(TargetError::Diverged(path.to_string()))
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn collect(root: &Path, dir: &Path, paths: &mut Vec<String>) -> Result<(), TargetError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect(root, &entry.path(), paths)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| TargetError::Escape(entry.path().display().to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            paths.push(relative);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_target_roundtrips_and_lists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("open");
        target
            .write("items/a/b/1.enc", b"one")
            .expect("first write");
        target
            .write("items/a/b/1.enc", b"one")
            .expect("identical rewrite is a retried write");
        target.write("tombstones/c/d/2.enc", b"two").expect("write");
        assert_eq!(
            target.list().expect("list"),
            vec![
                "items/a/b/1.enc".to_string(),
                "tombstones/c/d/2.enc".to_string()
            ]
        );
        assert_eq!(target.read("items/a/b/1.enc").expect("read"), b"one");
    }

    #[test]
    fn folder_target_refuses_diverged_and_escaping_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("open");
        target.write("items/a/b/1.enc", b"one").expect("write");
        assert!(matches!(
            target.write("items/a/b/1.enc", b"other"),
            Err(TargetError::Diverged(_))
        ));
        assert!(matches!(
            target.read("../outside"),
            Err(TargetError::Escape(_))
        ));
        assert!(matches!(target.read(""), Err(TargetError::Escape(_))));
    }
}
