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

//! Per-platform drift series and heatmap data (C4 #20, A1).
//!
//! A [`Platform`] is one tracked surface that forms a picture of the user. Each
//! platform has its own data source on the desktop:
//!
//! - [`Platform::Google`]: driven by the R2 Privacy Sandbox Topics read-backs
//!   ([`TopicsMeasurement`]). Each read-back is one timestamped category
//!   distribution (the assigned topics).
//! - [`Platform::Brokers`]: driven by the D1c data-broker scan/submission
//!   history ([`BrokerSubmission`]). Each submission's broker id is the category;
//!   the distribution at a timestamp is the cumulative tally of brokers the
//!   persona appears on, so removals visibly shrink it.
//! - [`Platform::Meta`]: there is no desktop data source yet, so it yields an
//!   EMPTY (no-data) series gracefully rather than failing.
//!
//! The platform set is EXTENSIBLE: [`Platform::Other`] carries a free label so a
//! future surface (or a test) is a first-class platform without a schema change.
//!
//! For each platform we derive, from the stored measurements, one
//! [`CategoryDistribution`] per timestamp, then compute:
//!
//! - a scalar KL-divergence DRIFT timeline ([`DriftSeries`]): `timestamp -> KL`,
//!   using a documented BASELINE (see [`Baseline`]); and
//! - a per-category drift HEATMAP ([`HeatmapSeries`]): `category x time -> the
//!   per-category contribution to that timestamp's divergence`.
//!
//! Everything degrades gracefully: empty inputs yield empty series (never a
//! panic, a divide-by-zero, or a `NaN`), and the DEVICE dimension aggregates
//! across paired devices into one combined view while still working with a
//! single device.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::brokers::{BrokerSubmission, SubmissionStatus};
use crate::measurement::distribution::{kl_divergence_breakdown, CategoryDistribution, Smoothing};
use crate::persona::{CategoryPool, SyntheticPersona};
use crate::store::TopicsMeasurement;

/// A tracked profiling surface (C4 #20). Extensible via [`Platform::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Google, driven by the R2 Privacy Sandbox Topics read-backs.
    Google,
    /// Data brokers, driven by the D1c broker scan/submission history.
    Brokers,
    /// Meta. No desktop data source yet, so its series is empty (no-data).
    Meta,
    /// An extension point: a future or test platform, identified by a label.
    Other(String),
}

impl Platform {
    /// A stable, human-readable label for this platform (for series keys and the
    /// dashboard legend).
    pub fn label(&self) -> String {
        match self {
            Platform::Google => "Google".to_string(),
            Platform::Brokers => "Brokers".to_string(),
            Platform::Meta => "Meta".to_string(),
            Platform::Other(name) => name.clone(),
        }
    }

    /// The built-in platforms that always appear in the dashboard, in display
    /// order. [`Platform::Other`] is added by callers as needed.
    pub fn builtins() -> Vec<Platform> {
        vec![Platform::Google, Platform::Brokers, Platform::Meta]
    }
}

/// How the baseline (reference) distribution for the drift metric is chosen
/// (C4 #20). The baseline is the `p` in `D_KL(p || observed)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Baseline {
    /// The persona's INTENDED category weights: the categories the persona is
    /// configured to follow, each weighted by the aligned topic weight. This is
    /// the preferred baseline because it states the GOAL the profile is steered
    /// toward, so drift measures distance from intent. Built via
    /// [`Baseline::from_persona`].
    PersonaIntent(CategoryDistribution),
    /// The FIRST observed snapshot in the series. Used when no persona intent is
    /// available; drift then measures movement away from the starting picture.
    FirstSnapshot,
    /// An explicit, caller-supplied baseline distribution.
    Explicit(CategoryDistribution),
}

impl Baseline {
    /// Build the preferred [`Baseline::PersonaIntent`] from a persona's declared
    /// interests: each known [`CategoryPool`] interest contributes the aligned
    /// topic weight ([`crate::constants::ALIGNED_WEIGHT`]). Interests that are not
    /// known category names are skipped (they cannot be a baseline category). An
    /// interest-less persona yields an empty intent, which the series handles as
    /// the no-baseline case.
    pub fn from_persona(persona: &SyntheticPersona) -> Self {
        let mut dist = CategoryDistribution::new();
        for interest in &persona.interests {
            if CategoryPool::from_name(interest).is_some() {
                dist.add(interest.clone(), crate::constants::ALIGNED_WEIGHT);
            }
        }
        Baseline::PersonaIntent(dist)
    }

    /// Resolve this baseline to a concrete distribution, given the observed
    /// snapshots (oldest first) the series was built from. Returns `None` when no
    /// baseline can be formed (e.g. [`Baseline::FirstSnapshot`] with no
    /// snapshots, or an empty persona intent), which the series treats as the
    /// gracefully-degrading no-baseline case (an all-zero-drift series).
    fn resolve(&self, snapshots: &[(i64, CategoryDistribution)]) -> Option<CategoryDistribution> {
        match self {
            Baseline::PersonaIntent(dist) | Baseline::Explicit(dist) => {
                if dist.is_empty() {
                    None
                } else {
                    Some(dist.clone())
                }
            }
            Baseline::FirstSnapshot => snapshots
                .iter()
                .find(|(_, d)| !d.is_empty())
                .map(|(_, d)| d.clone()),
        }
    }
}

/// One point on a scalar drift timeline: a timestamp and the KL divergence of
/// that snapshot from the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriftPoint {
    /// Epoch milliseconds of the snapshot.
    pub timestamp: i64,
    /// `D_KL(baseline || observed)` at this timestamp. Finite and `>= 0`.
    pub divergence: f64,
}

/// A per-platform scalar KL-divergence drift timeline (C4 #20): one series of
/// `(timestamp, divergence)` points, oldest first. An EMPTY `points` is the
/// well-formed no-data case (e.g. Meta, or a persona with no read-backs yet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftSeries {
    /// The platform this series describes.
    pub platform: Platform,
    /// The drift points, oldest first. Empty when there is no data.
    pub points: Vec<DriftPoint>,
}

impl DriftSeries {
    /// An empty (no-data) series for a platform.
    pub fn empty(platform: Platform) -> Self {
        Self {
            platform,
            points: Vec::new(),
        }
    }

    /// Whether the series has no data points.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The most recent divergence value, or `None` when the series is empty.
    pub fn latest(&self) -> Option<f64> {
        self.points.last().map(|p| p.divergence)
    }
}

/// Per-category drift heatmap data (C4 #20): for each `(category, timestamp)`
/// cell, the per-category contribution to that timestamp's divergence.
///
/// Stored column-major: [`timestamps`](Self::timestamps) are the time axis and
/// [`rows`](Self::rows) maps each category to its value at each timestamp (same
/// length and order as `timestamps`). A `0.0` cell means that category did not
/// contribute at that time. Empty when there is no data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeatmapSeries {
    /// The platform this heatmap describes.
    pub platform: Platform,
    /// The time axis (epoch millis), oldest first.
    pub timestamps: Vec<i64>,
    /// Category -> its contribution at each timestamp. Each `Vec<f64>` has the
    /// same length and order as [`timestamps`](Self::timestamps). Sorted by
    /// category label (the `BTreeMap` guarantees deterministic order).
    pub rows: BTreeMap<String, Vec<f64>>,
}

impl HeatmapSeries {
    /// An empty (no-data) heatmap for a platform.
    pub fn empty(platform: Platform) -> Self {
        Self {
            platform,
            timestamps: Vec::new(),
            rows: BTreeMap::new(),
        }
    }

    /// Whether the heatmap has no data.
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }
}

/// The full per-platform analytics bundle the dashboard renders (C4 #20): the
/// scalar drift timeline AND the per-category heatmap, plus the resolved
/// baseline support for legends. Empty inputs yield empty (no-panic) series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformDrift {
    /// The scalar KL-divergence timeline.
    pub series: DriftSeries,
    /// The per-category drift heatmap.
    pub heatmap: HeatmapSeries,
}

impl PlatformDrift {
    /// An empty (no-data) bundle for a platform (e.g. Meta).
    pub fn empty(platform: Platform) -> Self {
        Self {
            series: DriftSeries::empty(platform.clone()),
            heatmap: HeatmapSeries::empty(platform),
        }
    }
}

/// Derive the category DISTRIBUTION at each Topics read-back for a persona
/// (Google), oldest first. Each read-back's assigned topics become a tally:
/// every topic is one observation, labeled by its human-readable `name` when the
/// browser reported one, else by its numeric topic id. An available-but-EMPTY
/// read-back (the common epoch-boundary case) contributes an empty distribution
/// at its timestamp, which is well-formed (it reads as maximal drift from a
/// non-empty baseline, honestly reflecting "nothing observed yet").
pub fn topics_snapshots(measurements: &[TopicsMeasurement]) -> Vec<(i64, CategoryDistribution)> {
    let mut snapshots: Vec<(i64, CategoryDistribution)> = measurements
        .iter()
        .map(|m| {
            let mut dist = CategoryDistribution::new();
            for topic in &m.topics {
                let label = topic
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("topic:{}", topic.topic_id));
                dist.observe(label);
            }
            (m.recorded_at, dist)
        })
        .collect();
    snapshots.sort_by_key(|(ts, _)| *ts);
    snapshots
}

/// Derive the category distribution at each broker-submission event (Brokers),
/// oldest first. The category is the broker id; the distribution at a timestamp
/// is the CUMULATIVE set of brokers the persona is currently listed on as of that
/// event, so an opt-out that reaches `removed` visibly shrinks the picture and
/// drives drift back toward an empty (clean) profile.
///
/// "Currently listed" = the broker's latest submission status up to that
/// timestamp is outstanding (`drafted`/`submitted`/`relisted`) or `confirmed`
/// (confirmed receipt, but the listing may persist until `removed`). A `removed`
/// status drops that broker from the tally.
pub fn broker_snapshots(submissions: &[BrokerSubmission]) -> Vec<(i64, CategoryDistribution)> {
    // Order events by time so the cumulative state is built correctly.
    let mut events: Vec<&BrokerSubmission> = submissions.iter().collect();
    events.sort_by_key(|s| s.submitted_at);

    // Latest known status per broker as we sweep forward in time.
    let mut current: BTreeMap<String, SubmissionStatus> = BTreeMap::new();
    let mut snapshots = Vec::with_capacity(events.len());
    for sub in events {
        current.insert(sub.broker_id.clone(), sub.status);
        let mut dist = CategoryDistribution::new();
        for (broker_id, status) in &current {
            // A removed listing no longer contributes to the broker picture.
            if *status != SubmissionStatus::Removed {
                dist.observe(broker_id.clone());
            }
        }
        snapshots.push((sub.submitted_at, dist));
    }
    snapshots
}

/// Build a [`PlatformDrift`] bundle (scalar timeline + heatmap) for a platform
/// from its ordered `(timestamp, distribution)` snapshots and a [`Baseline`].
///
/// Degrades gracefully:
///
/// - No snapshots -> an empty bundle (no panic, no NaN).
/// - A baseline that cannot be resolved (no persona intent, no first snapshot)
///   -> an all-zero-drift series over the snapshot timestamps, so the timeline
///   still renders rather than vanishing.
pub fn build_platform_drift(
    platform: Platform,
    snapshots: &[(i64, CategoryDistribution)],
    baseline: &Baseline,
    smoothing: Smoothing,
) -> PlatformDrift {
    if snapshots.is_empty() {
        return PlatformDrift::empty(platform);
    }

    let baseline_dist = baseline.resolve(snapshots);

    let mut points = Vec::with_capacity(snapshots.len());
    let mut timestamps = Vec::with_capacity(snapshots.len());
    let mut rows: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for (idx, (timestamp, observed)) in snapshots.iter().enumerate() {
        timestamps.push(*timestamp);

        let (divergence, contributions) = match &baseline_dist {
            Some(base) => {
                let breakdown = kl_divergence_breakdown(base, observed, smoothing);
                let contribs: BTreeMap<String, f64> = breakdown
                    .contributions
                    .iter()
                    .map(|c| (c.category.clone(), c.contribution))
                    .collect();
                (breakdown.total, contribs)
            }
            // No baseline: zero drift everywhere, no per-category contributions.
            None => (0.0, BTreeMap::new()),
        };

        points.push(DriftPoint {
            timestamp: *timestamp,
            divergence,
        });

        // Fill the heatmap, keeping every category's row the same length as
        // `timestamps` by back-filling 0.0 for any category first seen now.
        for (category, value) in &contributions {
            let row = rows
                .entry(category.clone())
                .or_insert_with(|| vec![0.0; idx]);
            // Pad if this category appeared in an earlier cell but not here.
            while row.len() < idx {
                row.push(0.0);
            }
            row.push(*value);
        }
        // Any category present in earlier cells but absent now gets a trailing 0.
        for row in rows.values_mut() {
            while row.len() <= idx {
                row.push(0.0);
            }
        }
    }

    PlatformDrift {
        series: DriftSeries {
            platform: platform.clone(),
            points,
        },
        heatmap: HeatmapSeries {
            platform,
            timestamps,
            rows,
        },
    }
}

/// Aggregate the SAME platform's snapshots across multiple devices into one
/// combined, time-ordered snapshot stream (C4 #20 device dimension).
///
/// Snapshots from every device are merged and sorted by timestamp; snapshots at
/// the exact same timestamp (e.g. two devices read at once) are MERGED into one
/// combined distribution by summing their category counts. This yields the
/// single combined household view. It DEGRADES to single-device data naturally:
/// one device's snapshots pass through unchanged. An empty input (no devices, no
/// snapshots) yields an empty stream without panicking.
pub fn aggregate_devices(
    per_device: &[Vec<(i64, CategoryDistribution)>],
) -> Vec<(i64, CategoryDistribution)> {
    // Merge by timestamp, summing distributions that share a timestamp.
    let mut merged: BTreeMap<i64, CategoryDistribution> = BTreeMap::new();
    for device in per_device {
        for (ts, dist) in device {
            let entry = merged.entry(*ts).or_default();
            for (category, count) in dist.iter() {
                entry.add(category, count);
            }
        }
    }
    merged.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::AssignedTopic;
    use crate::persona::{AgeRange, Profession, Region};

    fn topic(id: i64, name: Option<&str>) -> AssignedTopic {
        AssignedTopic {
            topic_id: id,
            taxonomy_version: None,
            model_version: None,
            version: None,
            name: name.map(str::to_string),
        }
    }

    fn topics_measurement(ts: i64, topics: Vec<AssignedTopic>) -> TopicsMeasurement {
        TopicsMeasurement {
            persona_id: "p".to_string(),
            decoy_id: "d".to_string(),
            recorded_at: ts,
            available: true,
            topics,
        }
    }

    fn persona_with(interests: &[CategoryPool]) -> SyntheticPersona {
        SyntheticPersona::new(
            "plat-test-0000-4000-8000-000000000000".to_string(),
            "Plat".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_000,
            2_000,
        )
    }

    #[test]
    fn platform_labels_and_builtins() {
        assert_eq!(Platform::Google.label(), "Google");
        assert_eq!(Platform::Brokers.label(), "Brokers");
        assert_eq!(Platform::Meta.label(), "Meta");
        assert_eq!(Platform::Other("X".into()).label(), "X");
        assert_eq!(Platform::builtins().len(), 3);
    }

    #[test]
    fn meta_yields_empty_no_data_series() {
        // No desktop data source: building from no snapshots is an empty bundle.
        let bundle = build_platform_drift(
            Platform::Meta,
            &[],
            &Baseline::FirstSnapshot,
            Smoothing::new(),
        );
        assert!(bundle.series.is_empty());
        assert!(bundle.heatmap.is_empty());
        assert_eq!(bundle.series.latest(), None);
    }

    #[test]
    fn topics_snapshots_label_by_name_or_id() {
        let ms = vec![
            topics_measurement(10, vec![topic(1, Some("/Tech")), topic(2, None)]),
            topics_measurement(5, vec![]), // out of order + empty
        ];
        let snaps = topics_snapshots(&ms);
        // Sorted oldest first.
        assert_eq!(snaps[0].0, 5);
        assert!(snaps[0].1.is_empty());
        assert_eq!(snaps[1].0, 10);
        assert_eq!(snaps[1].1.count("/Tech"), 1.0);
        assert_eq!(snaps[1].1.count("topic:2"), 1.0);
    }

    #[test]
    fn broker_snapshots_shrink_when_removed() -> crate::Result<()> {
        use crate::brokers::registry::broker;
        let spokeo = broker("spokeo")?;
        let whitepages = broker("whitepages")?;

        let mut s1 = BrokerSubmission::draft("1".into(), "spokeo", "p", spokeo, 100);
        let s2 = BrokerSubmission::draft("2".into(), "whitepages", "p", whitepages, 200);
        // A later removal of spokeo.
        let mut s3 = BrokerSubmission::draft("3".into(), "spokeo", "p", spokeo, 300);
        s3.status = SubmissionStatus::Removed;
        s1.status = SubmissionStatus::Submitted;

        let snaps = broker_snapshots(&[s1, s2, s3]);
        assert_eq!(snaps.len(), 3);
        // At t=100: spokeo only.
        assert_eq!(snaps[0].1.len(), 1);
        // At t=200: spokeo + whitepages.
        assert_eq!(snaps[1].1.len(), 2);
        // At t=300: spokeo removed -> whitepages only.
        assert_eq!(snaps[2].1.len(), 1);
        assert_eq!(snaps[2].1.count("whitepages"), 1.0);
        assert_eq!(snaps[2].1.count("spokeo"), 0.0);
        Ok(())
    }

    #[test]
    fn heatmap_rows_align_with_timestamps_and_contributions_sum() {
        let snaps = topics_snapshots(&[
            topics_measurement(1, vec![topic(1, Some("a")), topic(2, Some("b"))]),
            topics_measurement(2, vec![topic(1, Some("a")), topic(3, Some("c"))]),
        ]);
        // An explicit baseline overlapping the observed labels so drift is real.
        let explicit =
            Baseline::Explicit(CategoryDistribution::from_counts([("a", 1.0), ("b", 1.0)]));
        let bundle = build_platform_drift(Platform::Google, &snaps, &explicit, Smoothing::new());

        // Two timestamps; every category row matches that length.
        assert_eq!(bundle.heatmap.timestamps, vec![1, 2]);
        for row in bundle.heatmap.rows.values() {
            assert_eq!(row.len(), 2);
        }
        // At each timestamp, the per-category contributions sum to the scalar
        // divergence of that point.
        for (idx, point) in bundle.series.points.iter().enumerate() {
            let col_sum: f64 = bundle.heatmap.rows.values().map(|r| r[idx]).sum();
            assert!(
                (col_sum - point.divergence).abs() < 1e-12,
                "column {idx} sum {col_sum} != divergence {}",
                point.divergence
            );
        }
    }

    #[test]
    fn no_baseline_yields_zero_drift_series_not_empty() {
        // FirstSnapshot with all-empty snapshots cannot resolve a baseline, so
        // the series is all-zero but still spans the timestamps (graceful).
        let snaps = vec![
            (1_i64, CategoryDistribution::new()),
            (2, CategoryDistribution::new()),
        ];
        let bundle = build_platform_drift(
            Platform::Google,
            &snaps,
            &Baseline::FirstSnapshot,
            Smoothing::new(),
        );
        assert_eq!(bundle.series.points.len(), 2);
        assert!(bundle.series.points.iter().all(|p| p.divergence == 0.0));
    }

    #[test]
    fn identical_to_baseline_is_zero_drift() {
        let base = CategoryDistribution::from_counts([("a", 1.0), ("b", 1.0)]);
        let snaps = vec![(1_i64, base.clone())];
        let bundle = build_platform_drift(
            Platform::Google,
            &snaps,
            &Baseline::Explicit(base),
            Smoothing::new(),
        );
        match bundle.series.latest() {
            Some(drift) => assert!(drift.abs() < 1e-9, "identical -> ~0 drift, got {drift}"),
            None => panic!("expected one drift point"),
        }
    }

    #[test]
    fn persona_intent_baseline_resolves_from_interests() {
        // The preferred baseline: the persona's declared interests become the
        // reference distribution. A read-back disjoint from the interests drifts.
        let snaps = topics_snapshots(&[topics_measurement(1, vec![topic(1, Some("unrelated"))])]);
        let baseline = Baseline::from_persona(&persona_with(&[
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
        ]));
        let bundle = build_platform_drift(Platform::Google, &snaps, &baseline, Smoothing::new());
        assert_eq!(bundle.series.points.len(), 1);
        // Disjoint support -> positive, finite drift.
        let drift = bundle.series.points[0].divergence;
        assert!(drift.is_finite() && drift > 0.0, "got {drift}");
    }

    #[test]
    fn single_device_aggregate_passes_through() {
        let snaps = topics_snapshots(&[
            topics_measurement(1, vec![topic(1, Some("a"))]),
            topics_measurement(2, vec![topic(2, Some("b"))]),
        ]);
        let combined = aggregate_devices(std::slice::from_ref(&snaps));
        assert_eq!(combined.len(), 2);
        assert_eq!(combined[0].0, 1);
        assert_eq!(combined[1].0, 2);
    }

    #[test]
    fn multi_device_aggregate_merges_same_timestamp() {
        let dev_a = vec![(10_i64, CategoryDistribution::from_counts([("a", 1.0)]))];
        let dev_b = vec![
            (10_i64, CategoryDistribution::from_counts([("b", 2.0)])),
            (20, CategoryDistribution::from_counts([("c", 1.0)])),
        ];
        let combined = aggregate_devices(&[dev_a, dev_b]);
        assert_eq!(combined.len(), 2);
        // t=10 merged a+b.
        assert_eq!(combined[0].0, 10);
        assert_eq!(combined[0].1.count("a"), 1.0);
        assert_eq!(combined[0].1.count("b"), 2.0);
        // t=20 from device b only.
        assert_eq!(combined[1].0, 20);
        assert_eq!(combined[1].1.count("c"), 1.0);
    }

    #[test]
    fn empty_aggregate_is_empty_no_panic() {
        assert!(aggregate_devices(&[]).is_empty());
        assert!(aggregate_devices(&[Vec::new(), Vec::new()]).is_empty());
    }
}
