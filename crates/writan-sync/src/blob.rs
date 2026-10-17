//! Sealed blobs: the wire format of a sync target.
//!
//! A target holds one tree (see `docs/adr/0005-sync-blob-format.md`): note
//! versions under `items/<note-uuid>/<device-uuid>/<counter>.enc`, deletions
//! under `tombstones/<note-uuid>/<device-uuid>/<counter>.enc`, keyring
//! generations under `keys/<generation>.json`. Every item/tombstone blob is
//! compact JSON sealed with the vault key (XChaCha20-Poly1305,
//! `nonce ‖ ciphertext`) and bound to its kind and note id through the
//! associated data: a note version cannot be passed off as a tombstone, nor a
//! version of one note as a version of another.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;
use writan_crypto::{KEY_LEN, open, seal};

use crate::{DeviceId, NoteId, VersionVector};

/// Payload banner of [`BlobKind::Item`] blobs.
pub const FORMAT_NOTE: &str = "writan-note/1";
/// Payload banner of [`BlobKind::Tombstone`] blobs.
pub const FORMAT_TOMBSTONE: &str = "writan-tombstone/1";

/// Errors raised while sealing, opening or naming blobs.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error(transparent)]
    Crypto(#[from] writan_crypto::CryptoError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("payload banner `{0}` is not `{1}`")]
    Format(String, &'static str),
    #[error("payload note id {found} does not match expected {want}")]
    IdMismatch { found: NoteId, want: NoteId },
}

/// What a blob directory under a target holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    /// A note version (`items/`).
    Item,
    /// A deletion marker (`tombstones/`).
    Tombstone,
}

impl BlobKind {
    /// The byte that opens the blob's associated data.
    pub const fn code(self) -> u8 {
        match self {
            Self::Item => 0x01,
            Self::Tombstone => 0x02,
        }
    }

    /// The target directory this kind lives in.
    pub const fn dir(self) -> &'static str {
        match self {
            Self::Item => "items",
            Self::Tombstone => "tombstones",
        }
    }

    /// The payload banner this kind must carry.
    pub const fn format(self) -> &'static str {
        match self {
            Self::Item => FORMAT_NOTE,
            Self::Tombstone => FORMAT_TOMBSTONE,
        }
    }
}

/// Associated data binding a blob to its kind and note: kind byte ‖ note UUID.
pub fn blob_aad(kind: BlobKind, note: NoteId) -> [u8; 17] {
    let mut aad = [0u8; 1 + 16];
    aad[0] = kind.code();
    aad[1..].copy_from_slice(note.as_bytes());
    aad
}

/// A note version as it travels inside a sealed [`BlobKind::Item`] blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteBlob {
    /// Payload banner; [`seal_note`] and [`open_note`] enforce [`FORMAT_NOTE`].
    pub format: String,
    /// Note identity; must equal the note id the blob is addressed by.
    pub id: NoteId,
    /// Vault-relative path hint for the adopting side.
    pub rel_path: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    pub body: String,
    /// The writer's full version vector for this note at seal time.
    pub vv: VersionVector,
}

impl NoteBlob {
    /// A minimal version; tags/pinned/created/updated start empty.
    pub fn new(
        id: NoteId,
        rel_path: String,
        title: String,
        body: String,
        vv: VersionVector,
    ) -> Self {
        Self {
            format: FORMAT_NOTE.to_string(),
            id,
            rel_path,
            title,
            tags: Vec::new(),
            pinned: false,
            created: None,
            updated: None,
            body,
            vv,
        }
    }
}

/// A deletion marker as it travels inside a sealed [`BlobKind::Tombstone`] blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TombstoneBlob {
    /// Payload banner; [`seal_tombstone`] and [`open_tombstone`] enforce
    /// [`FORMAT_TOMBSTONE`].
    pub format: String,
    /// Note identity; must equal the note id the blob is addressed by.
    pub id: NoteId,
    /// Deletion time (ISO-8601, informational).
    pub deleted_at: String,
    /// The deleter's full version vector for this note at seal time.
    pub vv: VersionVector,
}

impl TombstoneBlob {
    pub fn new(id: NoteId, deleted_at: String, vv: VersionVector) -> Self {
        Self {
            format: FORMAT_TOMBSTONE.to_string(),
            id,
            deleted_at,
            vv,
        }
    }
}

/// Seal any payload as a `kind`/`note` blob: compact JSON, then encrypt.
pub fn seal_blob<T: Serialize>(
    key: &[u8; KEY_LEN],
    kind: BlobKind,
    note: NoteId,
    payload: &T,
) -> Result<Vec<u8>, BlobError> {
    let json = serde_json::to_vec(payload)?;
    Ok(seal(key, &blob_aad(kind, note), &json)?)
}

/// Open a blob sealed by [`seal_blob`] and decode its JSON payload.
pub fn open_blob<T: DeserializeOwned>(
    key: &[u8; KEY_LEN],
    kind: BlobKind,
    note: NoteId,
    blob: &[u8],
) -> Result<T, BlobError> {
    let json = open(key, &blob_aad(kind, note), blob)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Seal a note version.
pub fn seal_note(key: &[u8; KEY_LEN], note: &NoteBlob) -> Result<Vec<u8>, BlobError> {
    check_format(&note.format, FORMAT_NOTE)?;
    seal_blob(key, BlobKind::Item, note.id, note)
}

/// Open a note version addressed by `id`.
pub fn open_note(key: &[u8; KEY_LEN], id: NoteId, blob: &[u8]) -> Result<NoteBlob, BlobError> {
    let note: NoteBlob = open_blob(key, BlobKind::Item, id, blob)?;
    check_format(&note.format, FORMAT_NOTE)?;
    if note.id != id {
        return Err(BlobError::IdMismatch {
            found: note.id,
            want: id,
        });
    }
    Ok(note)
}

/// Seal a tombstone.
pub fn seal_tombstone(
    key: &[u8; KEY_LEN],
    tombstone: &TombstoneBlob,
) -> Result<Vec<u8>, BlobError> {
    check_format(&tombstone.format, FORMAT_TOMBSTONE)?;
    seal_blob(key, BlobKind::Tombstone, tombstone.id, tombstone)
}

/// Open a tombstone addressed by `id`.
pub fn open_tombstone(
    key: &[u8; KEY_LEN],
    id: NoteId,
    blob: &[u8],
) -> Result<TombstoneBlob, BlobError> {
    let tombstone: TombstoneBlob = open_blob(key, BlobKind::Tombstone, id, blob)?;
    check_format(&tombstone.format, FORMAT_TOMBSTONE)?;
    if tombstone.id != id {
        return Err(BlobError::IdMismatch {
            found: tombstone.id,
            want: id,
        });
    }
    Ok(tombstone)
}

fn check_format(found: &str, want: &'static str) -> Result<(), BlobError> {
    if found == want {
        Ok(())
    } else {
        Err(BlobError::Format(found.to_string(), want))
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Target-relative path of a blob:
/// `<items|tombstones>/<note-uuid>/<device-uuid>/<counter>.enc`.
pub fn blob_path(kind: BlobKind, note: NoteId, device: DeviceId, counter: u64) -> String {
    format!("{}/{note}/{device}/{counter}.enc", kind.dir())
}

/// The components of a blob path under a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobPath {
    pub kind: BlobKind,
    pub note: NoteId,
    pub device: DeviceId,
    pub counter: u64,
}

/// Parse a target-relative blob path as written by [`blob_path`] (forward
/// slashes, exactly `<kind>/<note>/<device>/<counter>.enc`).
pub fn parse_blob_path(path: &str) -> Option<BlobPath> {
    let mut parts = path.split('/');
    let kind = match parts.next()? {
        "items" => BlobKind::Item,
        "tombstones" => BlobKind::Tombstone,
        _ => return None,
    };
    let note = Uuid::parse_str(parts.next()?).ok()?;
    let device = Uuid::parse_str(parts.next()?).ok()?;
    let counter = parts.next()?.strip_suffix(".enc")?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(BlobPath {
        kind,
        note,
        device,
        counter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; KEY_LEN] = [7u8; KEY_LEN];

    fn vv_of(device: DeviceId, counter: u64) -> VersionVector {
        let mut vv = VersionVector::new();
        vv.observe(&device, counter);
        vv
    }

    fn sample_note(id: NoteId, vv: VersionVector) -> NoteBlob {
        let mut note = NoteBlob::new(
            id,
            "notes/a.md".to_string(),
            "A".to_string(),
            "# top secret\n".to_string(),
            vv,
        );
        note.tags = vec!["misc".to_string()];
        note.pinned = true;
        note.created = Some("2026-10-17T09:00:00Z".to_string());
        note
    }

    #[test]
    fn note_blob_roundtrips() {
        let id = Uuid::new_v4();
        let note = sample_note(id, vv_of(Uuid::new_v4(), 3));
        let blob = seal_note(&KEY, &note).unwrap();
        assert!(
            !String::from_utf8_lossy(&blob).contains("top secret"),
            "sealed blob must not leak the body"
        );
        let back = open_note(&KEY, id, &blob).unwrap();
        assert_eq!(back, note);
    }

    #[test]
    fn tombstone_blob_roundtrips() {
        let id = Uuid::new_v4();
        let stone = TombstoneBlob::new(
            id,
            "2026-10-17T10:00:00Z".to_string(),
            vv_of(Uuid::new_v4(), 4),
        );
        let blob = seal_tombstone(&KEY, &stone).unwrap();
        let back = open_tombstone(&KEY, id, &blob).unwrap();
        assert_eq!(back, stone);
    }

    #[test]
    fn aad_binds_kind_and_identity() {
        let id = Uuid::new_v4();
        let note = sample_note(id, VersionVector::new());
        let blob = seal_note(&KEY, &note).unwrap();
        assert!(
            open_note(&KEY, Uuid::new_v4(), &blob).is_err(),
            "wrong note id must fail"
        );
        assert!(
            open_tombstone(&KEY, id, &blob).is_err(),
            "a note is not a tombstone"
        );
        assert!(
            open_note(&[8u8; KEY_LEN], id, &blob).is_err(),
            "wrong key must fail"
        );
    }

    #[test]
    fn format_banner_is_enforced() {
        let id = Uuid::new_v4();
        let mut note = sample_note(id, VersionVector::new());
        note.format = "writan-note/2".to_string();
        assert!(matches!(seal_note(&KEY, &note), Err(BlobError::Format(..))));
        let foreign = seal_blob(&KEY, BlobKind::Item, id, &note).unwrap();
        assert!(matches!(
            open_note(&KEY, id, &foreign),
            Err(BlobError::Format(..))
        ));
    }

    #[test]
    fn id_mismatch_is_rejected() {
        let aad_id = Uuid::new_v4();
        let note = sample_note(Uuid::new_v4(), VersionVector::new());
        let blob = seal_blob(&KEY, BlobKind::Item, aad_id, &note).unwrap();
        match open_note(&KEY, aad_id, &blob) {
            Err(BlobError::IdMismatch { found, want }) => {
                assert_eq!(found, note.id);
                assert_eq!(want, aad_id);
            }
            other => panic!("expected id mismatch, got {other:?}"),
        }
    }

    #[test]
    fn blob_paths_roundtrip() {
        let note = Uuid::new_v4();
        let device = Uuid::new_v4();
        let path = blob_path(BlobKind::Item, note, device, 12_345);
        assert_eq!(path, format!("items/{note}/{device}/12345.enc"));
        assert_eq!(
            parse_blob_path(&path),
            Some(BlobPath {
                kind: BlobKind::Item,
                note,
                device,
                counter: 12_345,
            })
        );
        let stone = blob_path(BlobKind::Tombstone, note, device, 1);
        assert_eq!(stone, format!("tombstones/{note}/{device}/1.enc"));
        assert_eq!(
            parse_blob_path(&stone),
            Some(BlobPath {
                kind: BlobKind::Tombstone,
                note,
                device,
                counter: 1,
            })
        );
    }

    #[test]
    fn blob_path_rejects_malformed() {
        let note = Uuid::new_v4();
        let device = Uuid::new_v4();
        for bad in [
            format!("other/{note}/{device}/1.enc"),
            format!("items/not-a-uuid/{device}/1.enc"),
            format!("items/{note}/not-a-uuid/1.enc"),
            format!("items/{note}/{device}/1.bin"),
            format!("items/{note}/{device}/x.enc"),
            format!("items/{note}/{device}/.enc"),
            format!("items/{note}/{device}"),
            format!("items/{note}/{device}/1.enc/2"),
        ] {
            assert_eq!(parse_blob_path(&bad), None, "must reject {bad:?}");
        }
    }

    #[test]
    fn payload_is_compact_json_with_banner() {
        let id = Uuid::new_v4();
        let note = sample_note(id, vv_of(Uuid::new_v4(), 2));
        let blob = seal_note(&KEY, &note).unwrap();
        let json = open(&KEY, &blob_aad(BlobKind::Item, id), &blob).unwrap();
        let text = String::from_utf8(json).unwrap();
        assert!(
            text.starts_with("{\"format\":\"writan-note/1\""),
            "banner must come first: {text}"
        );
    }
}
