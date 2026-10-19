//! N-device sync simulator: a deterministic, seeded soak test.
//!
//! Three simulated devices (separate vaults) create, edit, pin and delete
//! notes while syncing through one shared create-only target — a folder, or
//! an S3-compatible bucket with the `WRITAN_S3_*` environment set — in
//! interleaved order.
//! The run must end in a clean fixpoint: every device converges to the same
//! note set, and one more pass reports nothing but `unchanged`.
//!
//! Ignored by default because it is a soak-style test; nightly CI runs it in
//! release mode:
//!
//! ```text
//! cargo test -p writan-sync --release --test simulator -- --ignored --nocapture
//! ```
//!
//! The bucket variant (`three_devices_converge_through_a_bucket`) needs
//! `WRITAN_S3_ENDPOINT`, `WRITAN_S3_BUCKET`, `WRITAN_S3_ACCESS_KEY` and
//! `WRITAN_S3_SECRET_KEY` (`WRITAN_S3_REGION` defaults to `us-east-1`); the
//! bucket must exist and be empty.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use writan_core::vault::{Note, Vault};
use writan_sync::{
    BlobKind, FolderTarget, FolderVault, S3Config, S3HttpTransport, S3Target, SyncReport,
    SyncState, SyncTarget, VersionVector, blob_digest, blob_path, note_digest, open_note,
    open_tombstone, parse_blob_path, sync as run_engine,
};

const KEY: [u8; 32] = [9u8; 32];
const DEVICES: usize = 3;
const ROUNDS: usize = 40;

/// Small deterministic LCG (the same constants as the search-perf test).
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

/// Synthetic, strictly increasing timestamps ~90 s apart: usually outside the
/// 60 s later-write-wins window, so edits exercise diff3 and conflict copies.
fn stamp(tick: u64) -> String {
    let seconds = 1_767_225_600 + (tick as i64) * 90;
    OffsetDateTime::from_unix_timestamp(seconds)
        .expect("valid timestamp")
        .format(&Rfc3339)
        .expect("RFC 3339 formats")
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    title: String,
    body: String,
    pinned: bool,
    tags: Vec<String>,
}

struct Device {
    vault: Vault,
    state_path: PathBuf,
    id: Uuid,
    tick: u64,
}

impl Device {
    fn sync<T: SyncTarget>(&self, vault_id: Uuid, target: &T) -> SyncReport {
        let mut state = SyncState::load(&self.state_path).unwrap_or_default();
        let report = run_engine(
            &KEY,
            self.id,
            vault_id,
            &mut state,
            &FolderVault::new(&self.vault),
            target,
        )
        .expect("sync pass");
        state.save(&self.state_path).expect("state saved");
        report
    }

    fn snapshot(&self) -> BTreeMap<Uuid, Snapshot> {
        let mut map = BTreeMap::new();
        for rel in self.vault.walk_notes().expect("walk notes") {
            let note = self.vault.read(&rel).expect("read note");
            map.insert(
                note.meta.id,
                Snapshot {
                    title: note.meta.title.clone(),
                    body: note.body.clone(),
                    pinned: note.meta.pinned,
                    tags: note.meta.tags.clone(),
                },
            );
        }
        map
    }

    fn note_ids(&self) -> Vec<Uuid> {
        self.snapshot().into_keys().collect()
    }

    fn find(&self, id: Uuid) -> Option<Note> {
        for rel in self.vault.walk_notes().ok()? {
            let note = self.vault.read(&rel).ok()?;
            if note.meta.id == id {
                return Some(note);
            }
        }
        None
    }

    /// Next synthetic timestamp for this device.
    fn bump(&mut self) -> String {
        self.tick += 1;
        stamp(self.tick)
    }
}

fn device(root: &Path, name: &str, seed: u128) -> Device {
    Device {
        vault: Vault::open(root.join(name)).expect("vault opens"),
        state_path: root.join(format!("{name}-state.json")),
        id: Uuid::from_u128(seed),
        tick: 0,
    }
}

fn short(id: Uuid) -> String {
    let simple = id.simple().to_string();
    simple[simple.len() - 8..].to_string()
}

/// First eight characters of a hex digest.
fn hex8(digest: &str) -> &str {
    &digest[..digest.len().min(8)]
}

fn vv_text(vv: &VersionVector) -> String {
    vv.0.iter()
        .map(|(device, counter)| format!("{}:{counter}", short(*device)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Why a red sweep explains itself: every device's recorded state next to its
/// files, and every target entry with its vector and digest — the whole sync
/// cell, so a failure does not need a rerun to be understood.
fn dump_failure(label: &str, devices: &[Device], target: &impl SyncTarget) {
    for (i, dev) in devices.iter().enumerate() {
        let state = SyncState::load(&dev.state_path).unwrap_or_default();
        for rel in dev.vault.walk_notes().unwrap_or_default() {
            let Ok(note) = dev.vault.read(&rel) else {
                continue;
            };
            let id = note.meta.id;
            let recorded = state
                .digests
                .get(&id)
                .map_or_else(|| "-".to_string(), |digest| hex8(digest).to_string());
            let base = state.bases.get(&id).map_or_else(
                || "-".to_string(),
                |reference| {
                    let path = blob_path(BlobKind::Item, id, reference.device, reference.counter);
                    let blob = target
                        .read(&path)
                        .ok()
                        .and_then(|bytes| open_note(&KEY, id, &bytes).ok());
                    match blob {
                        Some(blob) => format!(
                            "{}/c{} dig={} updated={}",
                            short(reference.device),
                            reference.counter,
                            hex8(&blob_digest(&blob)),
                            blob.updated.as_deref().unwrap_or("-"),
                        ),
                        None => format!("{}/c{}", short(reference.device), reference.counter),
                    }
                },
            );
            eprintln!(
                "{label} dev {i} file {} ({rel}) {:?} updated={}: dig={} recorded={} vv={} base={base}",
                short(id),
                note.meta.title,
                note.meta.updated.as_deref().unwrap_or("-"),
                hex8(&note_digest(&note)),
                recorded,
                state
                    .vectors
                    .get(&id)
                    .map_or_else(|| "-".to_string(), vv_text),
            );
        }
        for (id, digest) in &state.digests {
            if dev.find(*id).is_none() {
                eprintln!(
                    "{label} dev {i} file {}: missing, recorded={} vv={}",
                    short(*id),
                    hex8(digest),
                    state
                        .vectors
                        .get(id)
                        .map_or_else(|| "-".to_string(), vv_text),
                );
            }
        }
    }

    let mut rows: Vec<(Uuid, u64, usize, Uuid, String)> = Vec::new();
    for path in target.list().unwrap_or_default() {
        let Some(parsed) = parse_blob_path(&path) else {
            continue;
        };
        let Ok(bytes) = target.read(&path) else {
            continue;
        };
        let row = match parsed.kind {
            BlobKind::Item => match open_note(&KEY, parsed.note, &bytes) {
                Ok(note) => {
                    let total: u64 = note.vv.0.values().sum();
                    let tail = note.vv.0.keys().next_back().copied().unwrap_or(Uuid::nil());
                    (
                        parsed.note,
                        total,
                        note.vv.0.len(),
                        tail,
                        format!(
                            "item {} {:?} dev {} c{} vv={} dig={} updated={}",
                            short(parsed.note),
                            note.title,
                            short(parsed.device),
                            parsed.counter,
                            vv_text(&note.vv),
                            hex8(&blob_digest(&note)),
                            note.updated.as_deref().unwrap_or("-"),
                        ),
                    )
                }
                Err(_) => continue,
            },
            BlobKind::Tombstone => match open_tombstone(&KEY, parsed.note, &bytes) {
                Ok(tombstone) => (
                    parsed.note,
                    0,
                    0,
                    Uuid::nil(),
                    format!(
                        "tombstone {} dev {} c{} vv={} deleted={}",
                        short(parsed.note),
                        short(parsed.device),
                        parsed.counter,
                        vv_text(&tombstone.vv),
                        tombstone.deleted_at,
                    ),
                ),
                Err(_) => continue,
            },
        };
        rows.push(row);
    }
    rows.sort_by_key(|row| (row.0, row.1, row.2, row.3));
    for (.., text) in rows {
        eprintln!("{label} target {text}");
    }
}

/// The soak loop shared by the folder and bucket variants; the bucket must
/// be empty, since the fixpoint assertions assume a fresh target.
fn run_soak<T: SyncTarget>(root: &Path, target: &T, label: &str) {
    let vault_id = Uuid::from_u128(0x51D3_0000);
    let mut devices = [
        device(root, "dev-a", 0xA110),
        device(root, "dev-b", 0xB220),
        device(root, "dev-c", 0xC330),
    ];

    let mut seed = 0x5EED_F00D_u64;
    let mut created = 0usize;

    for round in 0..ROUNDS {
        let d = (next(&mut seed) % DEVICES as u64) as usize;
        let op = next(&mut seed) % 100;
        let ids = devices[d].note_ids();

        if op < 30 || ids.is_empty() {
            let title = format!("Sim {created:02} d{d} r{round}");
            created += 1;
            let mut note = devices[d].vault.create(&title, &[]).expect("create note");
            note.body = format!("seed {created:02}\n");
            note.meta.updated = Some(devices[d].bump());
            devices[d].vault.write(&note).expect("write new note");
        } else if op < 70 {
            let id = ids[(next(&mut seed) % ids.len() as u64) as usize];
            if let Some(mut note) = devices[d].find(id) {
                note.body.push_str(&format!("line r{round} d{d}\n"));
                note.meta.updated = Some(devices[d].bump());
                devices[d].vault.write(&note).expect("write edit");
            }
        } else if op < 85 {
            let id = ids[(next(&mut seed) % ids.len() as u64) as usize];
            if let Some(mut note) = devices[d].find(id) {
                note.meta.pinned = !note.meta.pinned;
                note.meta.updated = Some(devices[d].bump());
                devices[d].vault.write(&note).expect("write pin");
            }
        } else if ids.len() > 1 {
            let id = ids[(next(&mut seed) % ids.len() as u64) as usize];
            if let Some(note) = devices[d].find(id) {
                devices[d]
                    .vault
                    .remove(&note.rel_path)
                    .expect("remove note");
            }
        }

        // Every fifth round everybody syncs; otherwise one random device does.
        if round.is_multiple_of(5) {
            for dev in &devices {
                dev.sync(vault_id, target);
            }
        } else {
            let d = (next(&mut seed) % DEVICES as u64) as usize;
            devices[d].sync(vault_id, target);
        }
    }

    // Sweep until a whole pass reports nothing but `unchanged` on every device.
    let mut converged = false;
    for sweep in 1..=12 {
        let mut clean = true;
        for (i, dev) in devices.iter().enumerate() {
            let report = dev.sync(vault_id, target);
            eprintln!(
                "{label} sweep {sweep} dev {i}: p={} m={} a={} d={} u={}",
                report.published, report.merged, report.adopted, report.deleted, report.unchanged
            );
            if report.published != 0
                || report.merged != 0
                || report.adopted != 0
                || report.deleted != 0
            {
                clean = false;
            }
        }
        if clean {
            converged = true;
            break;
        }
    }
    if !converged {
        dump_failure(label, &devices, target);
    }
    assert!(converged, "devices never reached a clean fixpoint");

    let snaps: Vec<_> = devices.iter().map(Device::snapshot).collect();
    for (i, snap) in snaps.iter().enumerate().skip(1) {
        assert_eq!(*snap, snaps[0], "device {i} diverges from device 0");
    }
    assert!(!snaps[0].is_empty(), "the simulator lost every note");
    eprintln!(
        "{label}: {} devices converged on {} notes after {ROUNDS} rounds",
        DEVICES,
        snaps[0].len()
    );

    // One more full pass must be a no-op beyond `unchanged`.
    for (i, dev) in devices.iter().enumerate() {
        let report = dev.sync(vault_id, target);
        assert_eq!(
            report.published, 0,
            "device {i} published after convergence"
        );
        assert_eq!(report.merged, 0, "device {i} merged after convergence");
        assert_eq!(report.adopted, 0, "device {i} adopted after convergence");
        assert_eq!(report.deleted, 0, "device {i} deleted after convergence");
        assert_eq!(
            report.unchanged,
            snaps[0].len(),
            "device {i} unchanged count"
        );
    }
}

#[test]
#[ignore = "N-device simulator; nightly CI runs it in release (see the module docs)"]
fn three_devices_converge_through_a_shared_folder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let target = FolderTarget::open(root.join("shared")).expect("folder target");
    run_soak(root, &target, "folder simulator");
}

#[test]
#[ignore = "S3 simulator; nightly CI runs it in release against S3Mock (see the module docs)"]
fn three_devices_converge_through_a_bucket() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let endpoint = std::env::var("WRITAN_S3_ENDPOINT").expect(
        "set WRITAN_S3_ENDPOINT (with BUCKET/ACCESS_KEY/SECRET_KEY) to run the bucket simulator",
    );
    let cfg = S3Config {
        endpoint,
        region: std::env::var("WRITAN_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
        bucket: std::env::var("WRITAN_S3_BUCKET").expect("WRITAN_S3_BUCKET"),
        access_key: std::env::var("WRITAN_S3_ACCESS_KEY").expect("WRITAN_S3_ACCESS_KEY"),
        secret_key: std::env::var("WRITAN_S3_SECRET_KEY").expect("WRITAN_S3_SECRET_KEY"),
    };
    let target = S3Target::new(S3HttpTransport::new(cfg));
    run_soak(root, &target, "s3 simulator");
}
