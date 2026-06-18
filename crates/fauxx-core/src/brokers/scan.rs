// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Broker scan snapshots and the per-broker diff timeline (C4 #22, A3).
//!
//! The C3 D1c re-listing seam ([`ListingCheck`](crate::brokers::ListingCheck))
//! only answers a single boolean: "is this persona still listed". The A3 broker
//! diff view needs something richer to DIFF over time: a
//! [`BrokerScanSnapshot`], which records, per `(broker, persona)` at a point in
//! time, the SET of identity fields/records the broker exposes about that
//! persona.
//!
//! Snapshots persist in the `broker_scan_snapshots` table (schema v10, migrated
//! forward via the `user_version` pattern) and round-trip as their exact JSON.
//!
//! ## What this computes (and what is deferred)
//!
//! - The live scanning that POPULATES a snapshot by reading a broker's public
//!   listing page is DEFERRED (exactly like the C3 live `ListingCheck`), and is
//!   noted as future work. A3 computes diffs from STORED snapshots only; there
//!   is NO scraping here.
//! - This module computes, per broker, a TIME-ORDERED diff between consecutive
//!   snapshots: each field is classified `added` / `removed` / `unchanged`
//!   ([`FieldChange`]). It also distinctly flags RE-LISTING: a field that was
//!   removed in an earlier diff and later REAPPEARS is marked
//!   [`FieldChange::Relisted`], tying back to the C3 re-listing motivation.
//! - A broker with ZERO or ONE snapshot yields a clear "no diff yet" result
//!   ([`BrokerDiffTimeline::no_diff_yet`]) rather than panicking.
//!
//! There is no GUI/CLI type here: the diff timeline is plain typed data the GUI
//! later renders.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One per-`(broker, persona)` scan snapshot (C4 #22): the set of identity
/// fields/records the broker exposed about the persona at [`scanned_at`].
///
/// The exposed fields are opaque, normalized strings (e.g. `"name"`,
/// `"city: Seattle"`, `"phone: 555-..."`); the diff treats them as set members,
/// so any future field shape is a first-class member without a schema change.
/// Stored as the exact JSON via
/// [`EncryptedStore::upsert_broker_scan_snapshot`](crate::store::EncryptedStore::upsert_broker_scan_snapshot).
///
/// [`scanned_at`]: BrokerScanSnapshot::scanned_at
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerScanSnapshot {
    /// Stable id for this snapshot (UUID v4 string).
    pub id: String,
    /// The broker id this snapshot is for.
    pub broker_id: String,
    /// The persona id this snapshot is for.
    pub persona_id: String,
    /// Epoch milliseconds when the scan was taken.
    pub scanned_at: i64,
    /// The set of identity fields/records the broker exposed about the persona
    /// at this scan. A `BTreeSet` so the membership is deduplicated and the
    /// order is deterministic (stable diffs, stable JSON). An EMPTY set is a
    /// valid snapshot: the broker exposed nothing (e.g. after a successful
    /// opt-out), which the diff reads as every prior field `removed`.
    pub fields: BTreeSet<String>,
}

impl BrokerScanSnapshot {
    /// Build a snapshot from an iterator of exposed field strings. Blank/
    /// whitespace-only fields are dropped so they cannot pollute the diff; the
    /// rest are collected into the deduplicated, ordered set.
    pub fn new(
        id: impl Into<String>,
        broker_id: &str,
        persona_id: &str,
        scanned_at: i64,
        fields: impl IntoIterator<Item = String>,
    ) -> Self {
        let fields = fields
            .into_iter()
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect();
        Self {
            id: id.into(),
            broker_id: broker_id.to_string(),
            persona_id: persona_id.to_string(),
            scanned_at,
            fields,
        }
    }

    /// Whether the broker exposed no identity fields at this scan.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// How one identity field changed between two consecutive snapshots (C4 #22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldChange {
    /// The field is present in the later snapshot but not the earlier one, and
    /// it had NOT been seen-then-removed before. A genuinely new exposure.
    Added,
    /// The field was present in the earlier snapshot but is gone in the later
    /// one. The broker stopped exposing it (e.g. an opt-out took effect).
    Removed,
    /// The field is present in both snapshots: no change.
    Unchanged,
    /// RE-LISTING: the field was REMOVED in an earlier diff and has now
    /// REAPPEARED. Distinctly flagged (it is not a plain `added`) to tie back to
    /// the C3 re-listing motivation: a broker that re-lists previously-removed
    /// data is the case the opt-out tracking most needs to surface.
    Relisted,
}

impl FieldChange {
    /// The stored/display string form (matches the serde representation).
    pub fn as_str(&self) -> &'static str {
        match self {
            FieldChange::Added => "added",
            FieldChange::Removed => "removed",
            FieldChange::Unchanged => "unchanged",
            FieldChange::Relisted => "relisted",
        }
    }
}

/// One field's classification within a single consecutive-snapshot diff step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDelta {
    /// The identity field this delta concerns.
    pub field: String,
    /// How it changed from the previous snapshot to this one.
    pub change: FieldChange,
}

/// The diff between two CONSECUTIVE snapshots (C4 #22): the later snapshot's
/// timestamp plus every field's [`FieldChange`] relative to the earlier one.
///
/// The deltas cover the UNION of both snapshots' fields, so a field present in
/// either is classified. Deltas are sorted by field name (deterministic via the
/// underlying ordered sets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDiff {
    /// The earlier snapshot's id (the "from" side).
    pub from_id: String,
    /// The later snapshot's id (the "to" side).
    pub to_id: String,
    /// Epoch milliseconds of the earlier snapshot.
    pub from_scanned_at: i64,
    /// Epoch milliseconds of the later snapshot.
    pub to_scanned_at: i64,
    /// Per-field classification, field-name ascending.
    pub deltas: Vec<FieldDelta>,
}

impl SnapshotDiff {
    /// The fields newly added in this step (plain `added`, not re-listed).
    pub fn added(&self) -> Vec<&str> {
        self.fields_with(FieldChange::Added)
    }

    /// The fields removed in this step.
    pub fn removed(&self) -> Vec<&str> {
        self.fields_with(FieldChange::Removed)
    }

    /// The fields RE-LISTED in this step (removed earlier, now reappeared).
    pub fn relisted(&self) -> Vec<&str> {
        self.fields_with(FieldChange::Relisted)
    }

    /// Whether any field was re-listed in this step.
    pub fn has_relisting(&self) -> bool {
        self.deltas
            .iter()
            .any(|d| d.change == FieldChange::Relisted)
    }

    fn fields_with(&self, change: FieldChange) -> Vec<&str> {
        self.deltas
            .iter()
            .filter(|d| d.change == change)
            .map(|d| d.field.as_str())
            .collect()
    }
}

/// The full per-broker diff timeline (C4 #22): every consecutive-snapshot diff,
/// oldest first, for one `(broker, persona)`.
///
/// An EMPTY [`diffs`](Self::diffs) with [`snapshot_count`](Self::snapshot_count)
/// of `0` or `1` is the well-formed "no diff yet" state (see
/// [`BrokerDiffTimeline::no_diff_yet`] and [`BrokerDiffTimeline::has_diff`]),
/// never a panic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerDiffTimeline {
    /// The broker id this timeline describes.
    pub broker_id: String,
    /// The persona id this timeline describes.
    pub persona_id: String,
    /// How many snapshots the timeline was built from.
    pub snapshot_count: usize,
    /// The consecutive-snapshot diffs, oldest first. Empty when fewer than two
    /// snapshots exist.
    pub diffs: Vec<SnapshotDiff>,
}

impl BrokerDiffTimeline {
    /// Whether this timeline has at least one computed diff (two or more
    /// snapshots).
    pub fn has_diff(&self) -> bool {
        !self.diffs.is_empty()
    }

    /// Whether this is the "no diff yet" state: fewer than two snapshots, so no
    /// consecutive pair could be diffed.
    pub fn no_diff_yet(&self) -> bool {
        self.snapshot_count < 2
    }

    /// Whether ANY diff in the timeline flagged a re-listing.
    pub fn has_relisting(&self) -> bool {
        self.diffs.iter().any(SnapshotDiff::has_relisting)
    }
}

/// Compute the per-broker diff timeline from a `(broker, persona)`'s scan
/// snapshots (C4 #22).
///
/// Snapshots are sorted oldest-first by `scanned_at` (ties broken by id for
/// determinism), then every consecutive pair is diffed. Each field in the union
/// of a pair is classified:
///
/// - present in both -> [`FieldChange::Unchanged`],
/// - present only in the later -> [`FieldChange::Added`], UNLESS that exact
///   field was REMOVED in an earlier diff of this same timeline, in which case
///   it is [`FieldChange::Relisted`] (the re-listing flag),
/// - present only in the earlier -> [`FieldChange::Removed`].
///
/// With zero or one snapshot the timeline is the "no diff yet" state (empty
/// diffs); it never panics. `broker_id`/`persona_id` are taken from the first
/// snapshot when present, else from the supplied fallbacks, so an empty input
/// still yields a well-labeled timeline.
pub fn compute_broker_diff_timeline(
    broker_id_fallback: &str,
    persona_id_fallback: &str,
    snapshots: &[BrokerScanSnapshot],
) -> BrokerDiffTimeline {
    let broker_id = snapshots
        .first()
        .map(|s| s.broker_id.clone())
        .unwrap_or_else(|| broker_id_fallback.to_string());
    let persona_id = snapshots
        .first()
        .map(|s| s.persona_id.clone())
        .unwrap_or_else(|| persona_id_fallback.to_string());

    let mut ordered: Vec<&BrokerScanSnapshot> = snapshots.iter().collect();
    ordered.sort_by(|a, b| {
        a.scanned_at
            .cmp(&b.scanned_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut diffs = Vec::new();
    // Fields that have been REMOVED at some earlier point in the timeline, so a
    // later reappearance is a re-listing rather than a plain addition.
    let mut ever_removed: BTreeSet<String> = BTreeSet::new();

    for pair in ordered.windows(2) {
        let (earlier, later) = (pair[0], pair[1]);
        let mut deltas = Vec::new();

        // The union of both fields, ordered (BTreeSet -> deterministic).
        let union: BTreeSet<&String> = earlier.fields.iter().chain(later.fields.iter()).collect();

        for field in union {
            let in_earlier = earlier.fields.contains(field);
            let in_later = later.fields.contains(field);
            let change = match (in_earlier, in_later) {
                (true, true) => FieldChange::Unchanged,
                (true, false) => {
                    ever_removed.insert(field.clone());
                    FieldChange::Removed
                }
                (false, true) => {
                    // A reappearance of a previously-removed field is a
                    // re-listing; an otherwise-new field is a plain addition.
                    if ever_removed.contains(field) {
                        // It is listed again; clear the removed flag so a
                        // subsequent removal-then-return is detected afresh.
                        ever_removed.remove(field);
                        FieldChange::Relisted
                    } else {
                        FieldChange::Added
                    }
                }
                // Not in either: cannot happen (the field came from the union).
                (false, false) => FieldChange::Unchanged,
            };
            deltas.push(FieldDelta {
                field: field.clone(),
                change,
            });
        }

        diffs.push(SnapshotDiff {
            from_id: earlier.id.clone(),
            to_id: later.id.clone(),
            from_scanned_at: earlier.scanned_at,
            to_scanned_at: later.scanned_at,
            deltas,
        });
    }

    BrokerDiffTimeline {
        broker_id,
        persona_id,
        snapshot_count: snapshots.len(),
        diffs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, scanned_at: i64, fields: &[&str]) -> BrokerScanSnapshot {
        BrokerScanSnapshot::new(
            id,
            "spokeo",
            "persona-1",
            scanned_at,
            fields.iter().map(|f| f.to_string()),
        )
    }

    #[test]
    fn snapshot_new_dedupes_and_drops_blanks() {
        let s = snap("s1", 100, &["name", "name", "  ", "phone", ""]);
        assert_eq!(s.fields.len(), 2);
        assert!(s.fields.contains("name"));
        assert!(s.fields.contains("phone"));
        assert!(!s.is_empty());
    }

    #[test]
    fn snapshot_json_round_trips() -> crate::Result<()> {
        let s = snap("s1", 100, &["name", "city: Seattle"]);
        let json = serde_json::to_string(&s)?;
        let back: BrokerScanSnapshot = serde_json::from_str(&json)?;
        assert_eq!(back, s);
        Ok(())
    }

    #[test]
    fn diff_classifies_added_removed_unchanged() {
        // s1: {name, phone}; s2: {name, email} -> name unchanged, phone removed,
        // email added.
        let snaps = vec![
            snap("s1", 100, &["name", "phone"]),
            snap("s2", 200, &["name", "email"]),
        ];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        assert!(tl.has_diff());
        assert!(!tl.no_diff_yet());
        assert_eq!(tl.diffs.len(), 1);
        let d = &tl.diffs[0];
        assert_eq!(d.added(), vec!["email"]);
        assert_eq!(d.removed(), vec!["phone"]);
        // unchanged is "name".
        let unchanged: Vec<&str> = d
            .deltas
            .iter()
            .filter(|x| x.change == FieldChange::Unchanged)
            .map(|x| x.field.as_str())
            .collect();
        assert_eq!(unchanged, vec!["name"]);
        assert!(!d.has_relisting());
        assert!(!tl.has_relisting());
    }

    #[test]
    fn relisting_is_flagged_when_removed_field_reappears() {
        // name removed at s2, then reappears at s3 -> flagged Relisted (not Added).
        let snaps = vec![
            snap("s1", 100, &["name", "phone"]),
            snap("s2", 200, &["phone"]),         // name removed
            snap("s3", 300, &["name", "phone"]), // name re-listed
        ];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        assert_eq!(tl.diffs.len(), 2);
        // First diff: name removed.
        assert_eq!(tl.diffs[0].removed(), vec!["name"]);
        // Second diff: name re-listed, NOT a plain add.
        assert_eq!(tl.diffs[1].relisted(), vec!["name"]);
        assert!(tl.diffs[1].added().is_empty());
        assert!(tl.diffs[1].has_relisting());
        assert!(tl.has_relisting());
    }

    #[test]
    fn first_appearance_is_added_not_relisted() {
        // A field never seen before is Added, even at a later snapshot.
        let snaps = vec![
            snap("s1", 100, &["name"]),
            snap("s2", 200, &["name", "email"]),
        ];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        assert_eq!(tl.diffs[0].added(), vec!["email"]);
        assert_eq!(tl.diffs[0].relisted(), Vec::<&str>::new());
    }

    #[test]
    fn zero_snapshots_is_no_diff_yet_no_panic() {
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &[]);
        assert_eq!(tl.snapshot_count, 0);
        assert!(tl.no_diff_yet());
        assert!(!tl.has_diff());
        assert!(tl.diffs.is_empty());
        // Falls back to the supplied labels.
        assert_eq!(tl.broker_id, "spokeo");
        assert_eq!(tl.persona_id, "persona-1");
    }

    #[test]
    fn one_snapshot_is_no_diff_yet_no_panic() {
        let snaps = vec![snap("s1", 100, &["name"])];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        assert_eq!(tl.snapshot_count, 1);
        assert!(tl.no_diff_yet());
        assert!(!tl.has_diff());
        assert!(tl.diffs.is_empty());
    }

    #[test]
    fn snapshots_are_sorted_oldest_first_before_diffing() {
        // Supplied out of order; the diff must order by scanned_at.
        let snaps = vec![
            snap("late", 300, &["name", "phone"]),
            snap("early", 100, &["name"]),
            snap("mid", 200, &["name", "email"]),
        ];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        assert_eq!(tl.diffs.len(), 2);
        // early -> mid: email added.
        assert_eq!(tl.diffs[0].from_scanned_at, 100);
        assert_eq!(tl.diffs[0].to_scanned_at, 200);
        assert_eq!(tl.diffs[0].added(), vec!["email"]);
        // mid -> late: email removed, phone added.
        assert_eq!(tl.diffs[1].from_scanned_at, 200);
        assert_eq!(tl.diffs[1].to_scanned_at, 300);
        assert_eq!(tl.diffs[1].added(), vec!["phone"]);
        assert_eq!(tl.diffs[1].removed(), vec!["email"]);
    }

    #[test]
    fn empty_later_snapshot_removes_all() {
        // After a successful opt-out the broker exposes nothing.
        let snaps = vec![snap("s1", 100, &["name", "phone"]), snap("s2", 200, &[])];
        let tl = compute_broker_diff_timeline("spokeo", "persona-1", &snaps);
        let mut removed = tl.diffs[0].removed();
        removed.sort_unstable();
        assert_eq!(removed, vec!["name", "phone"]);
    }

    #[test]
    fn field_change_strings() {
        assert_eq!(FieldChange::Added.as_str(), "added");
        assert_eq!(FieldChange::Removed.as_str(), "removed");
        assert_eq!(FieldChange::Unchanged.as_str(), "unchanged");
        assert_eq!(FieldChange::Relisted.as_str(), "relisted");
    }
}
