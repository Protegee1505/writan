//! Merge engine: bounded three-way diff with a last-writer-wins window.
//!
//! Two devices that edited the same note are reconciled by [`LwwDiff3`], the
//! v1 [`MergeStrategy`] (ADR-0001, `docs/PLAN.md`):
//!
//! 1. A side that already contains the other (version-vector dominance, or
//!    content unchanged since the stored base) is adopted as-is.
//! 2. Edits whose `updated` stamps lie within 60 s of each other resolve to
//!    the newer side, whole (Notesnook-style window).
//! 3. Otherwise a bounded line-based three-way diff merges both bodies
//!    (≤ 5 000 lines per body, ≤ 1 s wall clock).
//! 4. Whatever step 3 cannot settle — conflicts, oversized bodies, an
//!    over-budget run, or no stored base — becomes an explicit conflict copy:
//!    the newer side keeps the note, the older side materialises as a new
//!    note.
//!
//! Never a silent overwrite: every [`MergePlan`] either keeps one side whole
//! or carries the losing side in `conflict_copy`. The strategy sits behind a
//! trait so real-time co-editing (CRDT) can be added later without changing
//! the storage model.

use std::time::{Duration, Instant};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::VersionVector;
use crate::blob::NoteBlob;

/// How two concurrent versions of one note are reconciled.
///
/// `base` is the stored common ancestor of `local` and `remote`, when one is
/// known (a note synced for the first time may have none).
pub trait MergeStrategy {
    fn resolve(&self, base: Option<&NoteBlob>, local: &NoteBlob, remote: &NoteBlob) -> MergePlan;
}

/// Bounds and windows of the v1 strategy; defaults follow ADR-0001.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergePolicy {
    /// Edits at most this far apart resolve to the newer side, whole.
    pub lww_window: Duration,
    /// A body longer than this many lines forces a conflict copy.
    pub max_lines: usize,
    /// Wall-clock budget for one three-way diff; over it, conflict copy.
    pub time_budget: Duration,
}

impl Default for MergePolicy {
    fn default() -> Self {
        Self {
            lww_window: Duration::from_secs(60),
            max_lines: 5_000,
            time_budget: Duration::from_secs(1),
        }
    }
}

/// The v1 strategy: ~60 s LWW window, bounded line diff3, conflict copies.
#[derive(Debug, Clone, Copy, Default)]
pub struct LwwDiff3 {
    pub policy: MergePolicy,
}

/// The decision for one pair of concurrent versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlan {
    /// The version to keep under the note id.
    pub kept: NoteBlob,
    /// A new note to materialise alongside, when the merge gave up.
    pub conflict_copy: Option<NoteBlob>,
    /// Which rule decided the plan.
    pub resolution: Resolution,
}

/// Which rule decided a [`MergePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// One side was adopted whole (dominance, equality, or untouched side).
    FastForward,
    /// Within the LWW window, the newer side was adopted whole.
    Lww,
    /// The three-way diff merged both bodies.
    Diff3,
    /// The three-way diff could not settle it; a conflict copy was made.
    ConflictCopy,
}

impl MergeStrategy for LwwDiff3 {
    fn resolve(&self, base: Option<&NoteBlob>, local: &NoteBlob, remote: &NoteBlob) -> MergePlan {
        if same_content(local, remote) {
            return plan(local.clone(), Resolution::FastForward);
        }
        // Vector dominance is only meaningful once both sides carry history;
        // an empty vector knows nothing and would dominate vacuously.
        if !local.vv.0.is_empty() && !remote.vv.0.is_empty() {
            if remote.vv.contains(&local.vv) {
                return plan(remote.clone(), Resolution::FastForward);
            }
            if local.vv.contains(&remote.vv) {
                return plan(local.clone(), Resolution::FastForward);
            }
        }
        let local_changed = changed(base, local);
        let remote_changed = changed(base, remote);
        if !local_changed {
            return plan(remote.clone(), Resolution::FastForward);
        }
        if !remote_changed {
            return plan(local.clone(), Resolution::FastForward);
        }
        let (newer, older) = ordered_halves(local, remote);
        if self.within_lww_window(local, remote) {
            return plan(newer.clone(), Resolution::Lww);
        }
        let Some(base) = base else {
            return conflict_plan(newer, older);
        };
        if lines(&base.body) > self.policy.max_lines
            || lines(&local.body) > self.policy.max_lines
            || lines(&remote.body) > self.policy.max_lines
        {
            return conflict_plan(newer, older);
        }
        let started = Instant::now();
        let merged_body = diffy::merge(&base.body, &local.body, &remote.body);
        let over_budget = started.elapsed() > self.policy.time_budget;
        match merged_body {
            Ok(body) if !over_budget => plan(
                self.merge_meta(base, local, remote, body),
                Resolution::Diff3,
            ),
            _ => conflict_plan(newer, older),
        }
    }
}

impl LwwDiff3 {
    /// True when both `updated` stamps parse and lie within the LWW window.
    fn within_lww_window(&self, local: &NoteBlob, remote: &NoteBlob) -> bool {
        let (Some(local_time), Some(remote_time)) =
            (parse_time(&local.updated), parse_time(&remote.updated))
        else {
            return false;
        };
        let delta_secs =
            (local_time.unix_timestamp() - remote_time.unix_timestamp()).unsigned_abs();
        delta_secs <= self.policy.lww_window.as_secs()
    }

    /// Field-level merge on top of the diff3 body: a side that did not touch a
    /// field passes the other side's value through; concurrent edits to one
    /// field go to the newer side (tags union instead — loss-free).
    fn merge_meta(
        &self,
        base: &NoteBlob,
        local: &NoteBlob,
        remote: &NoteBlob,
        body: String,
    ) -> NoteBlob {
        let (newer, _) = ordered_halves(local, remote);
        let mut vv = local.vv.clone();
        vv.merge(&remote.vv);
        NoteBlob {
            format: local.format.clone(),
            id: local.id,
            rel_path: pick3(
                &base.rel_path,
                &local.rel_path,
                &remote.rel_path,
                &newer.rel_path,
            ),
            title: pick3(&base.title, &local.title, &remote.title, &newer.title),
            tags: merge_tags(&base.tags, &local.tags, &remote.tags, &newer.tags),
            pinned: pick3(&base.pinned, &local.pinned, &remote.pinned, &newer.pinned),
            created: match (&local.created, &remote.created) {
                (Some(local_created), Some(remote_created)) => {
                    Some(local_created.min(remote_created).clone())
                }
                (Some(value), None) | (None, Some(value)) => Some(value.clone()),
                (None, None) => None,
            },
            updated: newer.updated.clone(),
            body,
            vv,
        }
    }
}

fn plan(kept: NoteBlob, resolution: Resolution) -> MergePlan {
    MergePlan {
        kept,
        conflict_copy: None,
        resolution,
    }
}

fn conflict_plan(newer: &NoteBlob, older: &NoteBlob) -> MergePlan {
    MergePlan {
        kept: newer.clone(),
        conflict_copy: Some(conflict_copy(older)),
        resolution: Resolution::ConflictCopy,
    }
}

/// The losing side, turned into an explicit conflict copy (ADR-0001): a fresh
/// note id and vector, and a name that says what it is.
fn conflict_copy(older: &NoteBlob) -> NoteBlob {
    let mut copy = older.clone();
    copy.id = Uuid::new_v4();
    copy.vv = VersionVector::new();
    let stamp = parse_time(&older.updated).map(|time| time.date().to_string());
    copy.title = labelled(&older.title, stamp.as_deref());
    copy.rel_path = labelled_path(&older.rel_path, stamp.as_deref());
    copy
}

fn labelled(name: &str, stamp: Option<&str>) -> String {
    match stamp {
        Some(stamp) => format!("{name} (conflict copy {stamp})"),
        None => format!("{name} (conflict copy)"),
    }
}

fn labelled_path(path: &str, stamp: Option<&str>) -> String {
    let label = match stamp {
        Some(stamp) => format!(" (conflict copy {stamp})"),
        None => " (conflict copy)".to_string(),
    };
    if let Some((stem, ext)) = path.rsplit_once('.')
        && !stem.is_empty()
        && !ext.contains('/')
    {
        return format!("{stem}{label}.{ext}");
    }
    format!("{path}{label}")
}

/// Content equality ignoring timestamps and vectors.
fn same_content(a: &NoteBlob, b: &NoteBlob) -> bool {
    a.body == b.body
        && a.title == b.title
        && a.tags == b.tags
        && a.pinned == b.pinned
        && a.rel_path == b.rel_path
}

fn changed(base: Option<&NoteBlob>, side: &NoteBlob) -> bool {
    match base {
        Some(base) => !same_content(base, side),
        None => true,
    }
}

/// Which of the two versions is newer. Deterministic and device-symmetric, so
/// both sides of a conflict agree on who keeps the note and who becomes the
/// copy: `updated` decides when both parse; equal or missing stamps fall back
/// to the full content, compared the same way on every device.
fn a_is_newer(a: &NoteBlob, b: &NoteBlob) -> bool {
    match (parse_time(&a.updated), parse_time(&b.updated)) {
        (Some(a_time), Some(b_time)) if a_time != b_time => a_time > b_time,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => content_key(a) >= content_key(b),
    }
}

fn content_key(note: &NoteBlob) -> (&str, &str, &str, &[String], bool) {
    (
        &note.title,
        &note.body,
        &note.rel_path,
        &note.tags,
        note.pinned,
    )
}

fn ordered_halves<'a>(a: &'a NoteBlob, b: &'a NoteBlob) -> (&'a NoteBlob, &'a NoteBlob) {
    if a_is_newer(a, b) { (a, b) } else { (b, a) }
}

/// Three-way pick for one field: an untouched side defers to the other side;
/// two untouched-relative-to-each-other or conflicting edits go to `newer`.
fn pick3<T: PartialEq + Clone>(base: &T, local: &T, remote: &T, newer: &T) -> T {
    if local == remote || remote == base {
        local.clone()
    } else if local == base {
        remote.clone()
    } else {
        newer.clone()
    }
}

/// Tags: untouched-side rules first, otherwise a loss-free union (the newer
/// side's order first), so neither device's additions are dropped.
fn merge_tags(
    base: &[String],
    local: &[String],
    remote: &[String],
    newer: &[String],
) -> Vec<String> {
    if local == remote {
        return local.to_vec();
    }
    if remote == base {
        return local.to_vec();
    }
    if local == base {
        return remote.to_vec();
    }
    let mut tags = newer.to_vec();
    for tag in local.iter().chain(remote.iter()) {
        if !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }
    tags
}

fn parse_time(value: &Option<String>) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value.as_deref()?, &Rfc3339).ok()
}

fn lines(body: &str) -> usize {
    body.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoteId;

    const T0: &str = "2026-10-17T10:00:00Z";
    const MIN_1: &str = "2026-10-17T10:01:00Z";
    const MIN_10: &str = "2026-10-17T10:10:00Z";

    fn note(id: NoteId, body: &str, updated: Option<&str>) -> NoteBlob {
        let mut note = NoteBlob::new(
            id,
            "notes/a.md".to_string(),
            "A".to_string(),
            body.to_string(),
            VersionVector::new(),
        );
        note.updated = updated.map(str::to_string);
        note
    }

    fn base_body() -> String {
        let mut body = String::new();
        for i in 1..=10 {
            body.push_str(&format!("line {i}\n"));
        }
        body
    }

    fn strategy() -> LwwDiff3 {
        LwwDiff3::default()
    }

    #[test]
    fn vector_dominance_adopts_the_dominant_side() {
        let id = Uuid::new_v4();
        let device_a = Uuid::new_v4();
        let device_b = Uuid::new_v4();
        let mut local = note(id, "local\n", Some(MIN_10));
        local.vv.observe(&device_a, 1);
        local.vv.observe(&device_b, 2);
        let mut remote = note(id, "remote\n", Some(MIN_10));
        remote.vv.observe(&device_a, 1);
        let plan = strategy().resolve(None, &local, &remote);
        assert_eq!(plan.resolution, Resolution::FastForward);
        assert_eq!(plan.kept.body, "local\n");
        assert!(plan.conflict_copy.is_none());
    }

    #[test]
    fn side_untouched_since_base_adopts_the_other() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(id, &base_body(), Some(MIN_10));
        let remote = note(
            id,
            &base_body().replace("line 3\n", "line 3 remote\n"),
            Some(MIN_10),
        );
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::FastForward);
        assert_eq!(plan.kept.body, remote.body);
    }

    #[test]
    fn near_simultaneous_edits_resolve_to_the_newer() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(id, "completely local\n", Some(T0));
        let remote = note(id, "completely remote\n", Some(MIN_1));
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::Lww);
        assert_eq!(plan.kept.body, "completely remote\n");
        assert!(plan.conflict_copy.is_none());
    }

    #[test]
    fn disjoint_edits_three_way_merge() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(
            id,
            &base_body().replace("line 1\n", "line 1 local\n"),
            Some(T0),
        );
        let remote = note(
            id,
            &base_body().replace("line 10\n", "line 10 remote\n"),
            Some(MIN_10),
        );
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::Diff3);
        assert!(plan.kept.body.contains("line 1 local"));
        assert!(plan.kept.body.contains("line 10 remote"));
        assert!(plan.conflict_copy.is_none());
    }

    #[test]
    fn overlapping_edits_become_a_conflict_copy() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(
            id,
            &base_body().replace("line 3\n", "local version\n"),
            Some(T0),
        );
        let remote = note(
            id,
            &base_body().replace("line 3\n", "remote version\n"),
            Some(MIN_10),
        );
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::ConflictCopy);
        assert_eq!(plan.kept.body, remote.body, "newer side keeps the note");
        let copy = plan.conflict_copy.expect("losing side must be preserved");
        assert_ne!(copy.id, id);
        assert_eq!(copy.body, local.body);
        assert!(copy.title.contains("(conflict copy"));
        assert!(copy.rel_path.contains("(conflict copy"));
        assert!(copy.rel_path.ends_with(".md"));
        assert_eq!(copy.vv, VersionVector::new(), "a copy starts fresh");
    }

    #[test]
    fn oversized_bodies_never_reach_the_diff() {
        let id = Uuid::new_v4();
        let mut big = String::new();
        for i in 0..6_000 {
            big.push_str(&format!("line {i}\n"));
        }
        let base = note(id, &big, Some("2026-10-17T09:00:00Z"));
        let local = note(id, &big.replace("line 0\n", "line 0 local\n"), Some(T0));
        let remote = note(
            id,
            &big.replace("line 5999\n", "line 5999 remote\n"),
            Some(MIN_10),
        );
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::ConflictCopy);
        assert!(plan.conflict_copy.is_some());
    }

    #[test]
    fn no_base_falls_back_to_a_conflict_copy() {
        let id = Uuid::new_v4();
        let local = note(id, "local\n", Some(T0));
        let remote = note(id, "remote\n", Some(MIN_10));
        let plan = strategy().resolve(None, &local, &remote);
        assert_eq!(plan.resolution, Resolution::ConflictCopy);
        assert_eq!(plan.kept.body, "remote\n");
        assert_eq!(
            plan.conflict_copy
                .expect("losing side must be preserved")
                .body,
            "local\n"
        );
    }

    #[test]
    fn concurrent_metadata_edits_merge_loss_free() {
        let id = Uuid::new_v4();
        let mut base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        base.tags = vec!["keep".to_string()];
        let mut local = note(id, &base_body(), Some(T0));
        local.title = "Local title".to_string();
        local.tags = vec!["keep".to_string(), "local".to_string()];
        let mut remote = note(id, &base_body(), Some(MIN_10));
        remote.tags = vec!["keep".to_string(), "remote".to_string()];
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::Diff3, "only metadata diverged");
        assert_eq!(plan.kept.title, "Local title");
        for tag in ["keep", "local", "remote"] {
            assert!(
                plan.kept.tags.iter().any(|t| t == tag),
                "tag {tag} must survive"
            );
        }
    }

    #[test]
    fn missing_timestamps_use_the_diff() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(id, &base_body().replace("line 1\n", "line 1 local\n"), None);
        let remote = note(
            id,
            &base_body().replace("line 10\n", "line 10 remote\n"),
            None,
        );
        let plan = strategy().resolve(Some(&base), &local, &remote);
        assert_eq!(plan.resolution, Resolution::Diff3);
    }

    #[test]
    fn conflict_plan_is_device_symmetric() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(
            id,
            &base_body().replace("line 3\n", "local version\n"),
            Some(T0),
        );
        let remote = note(
            id,
            &base_body().replace("line 3\n", "remote version\n"),
            Some(MIN_10),
        );
        let straight = strategy().resolve(Some(&base), &local, &remote);
        let flipped = strategy().resolve(Some(&base), &remote, &local);
        assert_eq!(straight.resolution, flipped.resolution);
        assert_eq!(straight.kept.body, flipped.kept.body);
        let straight_copy = straight.conflict_copy.expect("straight copy");
        let flipped_copy = flipped.conflict_copy.expect("flipped copy");
        assert_eq!(straight_copy.body, flipped_copy.body);
        assert_eq!(straight_copy.title, flipped_copy.title);
        assert_eq!(straight_copy.rel_path, flipped_copy.rel_path);
    }

    #[test]
    fn lww_plan_is_device_symmetric() {
        let id = Uuid::new_v4();
        let base = note(id, &base_body(), Some("2026-10-17T09:00:00Z"));
        let local = note(id, "aaa\n", Some(T0));
        let remote = note(id, "bbb\n", Some(MIN_1));
        let straight = strategy().resolve(Some(&base), &local, &remote);
        let flipped = strategy().resolve(Some(&base), &remote, &local);
        assert_eq!(straight.resolution, Resolution::Lww);
        assert_eq!(straight.kept.body, "bbb\n");
        assert_eq!(straight.kept.body, flipped.kept.body);
    }
}
