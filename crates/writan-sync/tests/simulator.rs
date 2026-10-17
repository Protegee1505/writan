//! N-device sync simulator: a deterministic, seeded soak test.
//!
//! Three simulated devices (separate vaults) create, edit, pin and delete
//! notes while syncing through one shared folder target in interleaved order.
//! The run must end in a clean fixpoint: every device converges to the same
//! note set, and one more pass reports nothing but `unchanged`.
//!
//! Ignored by default because it is a soak-style test; nightly CI runs it in
//! release mode:
//!
//! ```text
//! cargo test -p writan-sync --release --test simulator -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use writan_core::vault::{Note, Vault};
use writan_sync::{FolderTarget, FolderVault, SyncReport, SyncState, sync as run_engine};

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
    fn sync(&self, vault_id: Uuid, target: &FolderTarget) -> SyncReport {
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

#[test]
#[ignore = "N-device simulator; nightly CI runs it in release (see the module docs)"]
fn three_devices_converge_through_a_shared_folder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let target = FolderTarget::open(root.join("shared")).expect("folder target");
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
                dev.sync(vault_id, &target);
            }
        } else {
            let d = (next(&mut seed) % DEVICES as u64) as usize;
            devices[d].sync(vault_id, &target);
        }
    }

    // Sweep until a whole pass reports nothing but `unchanged` on every device.
    let mut converged = false;
    for _ in 0..6 {
        let mut clean = true;
        for dev in &devices {
            let report = dev.sync(vault_id, &target);
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
    assert!(converged, "devices never reached a clean fixpoint");

    let snaps: Vec<_> = devices.iter().map(Device::snapshot).collect();
    for (i, snap) in snaps.iter().enumerate().skip(1) {
        assert_eq!(*snap, snaps[0], "device {i} diverges from device 0");
    }
    assert!(!snaps[0].is_empty(), "the simulator lost every note");
    eprintln!(
        "simulator: {} devices converged on {} notes after {ROUNDS} rounds",
        DEVICES,
        snaps[0].len()
    );

    // One more full pass must be a no-op beyond `unchanged`.
    for (i, dev) in devices.iter().enumerate() {
        let report = dev.sync(vault_id, &target);
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
