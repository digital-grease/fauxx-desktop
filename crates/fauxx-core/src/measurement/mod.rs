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

//! Measurement and analytics (C4 milestone, issues #20 A1 and #21 A2).
//!
//! This is the headless measurement core the dashboard later renders. It owns
//! NO charting or GUI code: it computes the time series and the statistical
//! comparison from stored data and returns typed values over the Core async API.
//!
//! - [`distribution`] holds the [`CategoryDistribution`] unit and the KL-
//!   divergence drift metric with Laplace/epsilon smoothing (A1).
//! - [`platform`] defines the extensible [`Platform`] notion (Google from R2
//!   Topics read-backs, Brokers from D1c submission history, Meta as a
//!   gracefully-empty no-data series), derives a category distribution per
//!   timestamp, and builds the per-platform scalar drift timeline plus the per-
//!   category drift heatmap, with a documented [`Baseline`] and a device
//!   dimension that aggregates across paired devices and degrades to one device
//!   (A1).
//! - [`stats`] holds Cohen's `d` (effect size, plain math) and the two-sample
//!   t-test (significance via the `statrs` Student-t CDF), both guarding tiny/
//!   degenerate samples (A2).
//! - [`shadow`] models the treated/control [`ShadowProfile`] arms (persisted in
//!   the new `shadow_profiles` table) and the plainly-readable
//!   [`CohortComparison`] across cohorts (A2).
//!
//! The [`MeasurementEngine`] orchestrates these over the shared encrypted store,
//! so the same numbers are reachable headless (and later over MQTT). It is the
//! type the [`Core`](crate::Core) facade delegates the C4 API to.

pub mod distribution;
pub mod export;
pub mod platform;
pub mod shadow;
pub mod stats;

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;

pub use distribution::{
    kl_divergence, kl_divergence_breakdown, CategoryContribution, CategoryDistribution,
    DriftBreakdown, Smoothing, DEFAULT_EPSILON,
};
pub use export::{
    export_efficacy_snapshot, EfficacySnapshotData, ExportArtifact, ExportFormat, ExportMetadata,
};
pub use platform::{
    aggregate_devices, broker_snapshots, build_platform_drift, topics_snapshots, Baseline,
    DriftPoint, DriftSeries, HeatmapSeries, Platform, PlatformDrift,
};
pub use shadow::{compare_cohorts, Arm, CohortComparison, ShadowProfile};
pub use stats::{
    cohens_d, cohens_d_from_stats, two_sample_t_test, two_sample_t_test_from_stats, SampleStats,
    TTestKind, TTestResult,
};

use crate::error::Result;
use crate::store::EncryptedStore;

/// The measurement engine: computes the C4 series and A/B comparison from stored
/// data behind the Core async API (C4 #20/#21).
///
/// Cheap to clone (shared state is behind an `Arc`). Reaches the Topics read-back
/// history, the broker submission history, and the new shadow-profile table
/// through the SAME encrypted store the rest of the core uses. It is headless: no
/// GUI/CLI types cross this boundary, and every method degrades gracefully on
/// empty/no-data inputs.
#[derive(Clone)]
pub struct MeasurementEngine {
    store: Arc<Mutex<EncryptedStore>>,
    smoothing: Smoothing,
}

impl std::fmt::Debug for MeasurementEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeasurementEngine")
            .field("smoothing", &self.smoothing)
            .finish_non_exhaustive()
    }
}

impl MeasurementEngine {
    /// Build an engine over the shared store with [`Smoothing::new`] (the default
    /// [`DEFAULT_EPSILON`]).
    pub fn new(store: Arc<Mutex<EncryptedStore>>) -> Self {
        Self {
            store,
            smoothing: Smoothing::new(),
        }
    }

    /// Build an engine with an explicit smoothing setting (tests can pin a tiny
    /// epsilon to compare against textbook KL values).
    pub fn with_smoothing(store: Arc<Mutex<EncryptedStore>>, smoothing: Smoothing) -> Self {
        Self { store, smoothing }
    }

    /// The smoothing this engine applies to every divergence.
    pub fn smoothing(&self) -> Smoothing {
        self.smoothing
    }

    // --- A1: per-platform drift series + heatmap ----------------------------

    /// Build the [`PlatformDrift`] bundle (scalar KL timeline + per-category
    /// heatmap) for one platform and persona, against `baseline`.
    ///
    /// Data sourcing per platform:
    ///
    /// - [`Platform::Google`]: the persona's Topics read-backs from the store.
    /// - [`Platform::Brokers`]: the persona's broker submissions from the store.
    /// - [`Platform::Meta`]: no desktop data source, so an empty bundle.
    /// - [`Platform::Other`]: no built-in source, so an empty bundle (callers
    ///   with their own data use [`build_platform_drift`] directly).
    ///
    /// Never panics on no data: an absent history yields an empty bundle.
    pub async fn platform_drift(
        &self,
        platform: Platform,
        persona_id: &str,
        baseline: &Baseline,
    ) -> Result<PlatformDrift> {
        let snapshots = self.platform_snapshots(&platform, persona_id).await?;
        Ok(build_platform_drift(
            platform,
            &snapshots,
            baseline,
            self.smoothing,
        ))
    }

    /// The scalar drift timeline for one platform/persona (a thin accessor over
    /// [`platform_drift`](Self::platform_drift) returning just the series).
    pub async fn platform_drift_series(
        &self,
        platform: Platform,
        persona_id: &str,
        baseline: &Baseline,
    ) -> Result<DriftSeries> {
        Ok(self
            .platform_drift(platform, persona_id, baseline)
            .await?
            .series)
    }

    /// Build the drift bundles for EVERY built-in platform for a persona, in
    /// display order (Google, Brokers, Meta). Meta is the gracefully-empty
    /// no-data series. This is the single call the dashboard's multi-series
    /// timeline consumes.
    pub async fn all_platform_drift(
        &self,
        persona_id: &str,
        baseline: &Baseline,
    ) -> Result<Vec<PlatformDrift>> {
        let mut out = Vec::new();
        for platform in Platform::builtins() {
            out.push(self.platform_drift(platform, persona_id, baseline).await?);
        }
        Ok(out)
    }

    /// Build a platform's drift bundle aggregated ACROSS DEVICES into one
    /// combined view (C4 #20 device dimension). Each `persona_ids` entry is one
    /// device's persona; their snapshots are merged by timestamp. DEGRADES to
    /// single-device data (one id) and to no-data (empty list) without panicking.
    pub async fn combined_platform_drift(
        &self,
        platform: Platform,
        persona_ids: &[String],
        baseline: &Baseline,
    ) -> Result<PlatformDrift> {
        let mut per_device = Vec::with_capacity(persona_ids.len());
        for persona_id in persona_ids {
            per_device.push(self.platform_snapshots(&platform, persona_id).await?);
        }
        let combined = aggregate_devices(&per_device);
        Ok(build_platform_drift(
            platform,
            &combined,
            baseline,
            self.smoothing,
        ))
    }

    /// Derive the `(timestamp, distribution)` snapshots for a platform/persona
    /// from the store. The internal seam shared by every A1 accessor.
    async fn platform_snapshots(
        &self,
        platform: &Platform,
        persona_id: &str,
    ) -> Result<Vec<(i64, CategoryDistribution)>> {
        match platform {
            Platform::Google => {
                let measurements = self.store.lock().await.topics_for(persona_id)?;
                Ok(topics_snapshots(&measurements))
            }
            Platform::Brokers => {
                let submissions = self
                    .store
                    .lock()
                    .await
                    .list_broker_submissions(Some(persona_id))?;
                Ok(broker_snapshots(&submissions))
            }
            // No desktop data source yet (Meta), or no built-in source (Other):
            // a gracefully-empty snapshot stream.
            Platform::Meta | Platform::Other(_) => Ok(Vec::new()),
        }
    }

    // --- A2: shadow profiles + cohort comparison ----------------------------

    /// Persist (insert or replace) a shadow-profile definition.
    pub async fn save_shadow_profile(&self, profile: &ShadowProfile) -> Result<()> {
        self.store.lock().await.upsert_shadow_profile(profile)
    }

    /// List all shadow-profile definitions, newest-defined first.
    pub async fn list_shadow_profiles(&self) -> Result<Vec<ShadowProfile>> {
        self.store.lock().await.list_shadow_profiles()
    }

    /// Fetch one shadow-profile definition by id, or `None` if absent.
    pub async fn get_shadow_profile(&self, id: &str) -> Result<Option<ShadowProfile>> {
        self.store.lock().await.get_shadow_profile(id)
    }

    /// Delete a shadow-profile definition by id. Returns `true` if a row was
    /// removed.
    pub async fn delete_shadow_profile(&self, id: &str) -> Result<bool> {
        self.store.lock().await.delete_shadow_profile(id)
    }

    /// The drift SAMPLE for one shadow profile on a platform: the scalar KL
    /// divergence at every snapshot of that profile's persona, against
    /// `baseline`. This is the per-arm sample fed into [`compare_cohorts`], so
    /// the A/B numbers reuse the A1 metric exactly.
    pub async fn shadow_drift_sample(
        &self,
        profile: &ShadowProfile,
        platform: Platform,
        baseline: &Baseline,
    ) -> Result<Vec<f64>> {
        let series = self
            .platform_drift_series(platform, &profile.persona_id, baseline)
            .await?;
        Ok(series.points.iter().map(|p| p.divergence).collect())
    }

    /// Run the treated-vs-control A/B comparison across the defined shadow
    /// profiles on `platform` (C4 #21).
    ///
    /// All TREATED profiles' drift samples are pooled into the treated cohort and
    /// all CONTROL profiles' into the control cohort, then compared via
    /// [`compare_cohorts`] (effect size + significance + plain summary). The
    /// `kind` selects Welch (default) or pooled significance. Each profile's
    /// drift uses ITS OWN persona's baseline when `per_profile_baseline` is
    /// `None`; otherwise the supplied shared baseline is used for every arm.
    ///
    /// Degrades gracefully: with no profiles, or only one arm populated, the
    /// comparison reports the conservative "not enough data" result rather than
    /// panicking.
    pub async fn compare_shadow_cohorts(
        &self,
        platform: Platform,
        baseline: &Baseline,
        kind: TTestKind,
    ) -> Result<CohortComparison> {
        let profiles = self.list_shadow_profiles().await?;
        let mut treated = Vec::new();
        let mut control = Vec::new();
        for profile in &profiles {
            let sample = self
                .shadow_drift_sample(profile, platform.clone(), baseline)
                .await?;
            match profile.arm {
                Arm::Treated => treated.extend(sample),
                Arm::Control => control.extend(sample),
            }
        }
        Ok(compare_cohorts(&treated, &control, kind))
    }

    // --- A4: efficacy-snapshot export (C4 #23) ------------------------------

    /// Build the [`EfficacySnapshotData`] for a persona as of `as_of_millis`:
    /// the per-platform A1 drift bundles for every built-in platform, against
    /// `baseline`. The single structure the CSV/JSON/PDF exports all derive from.
    pub async fn efficacy_snapshot_data(
        &self,
        persona_id: &str,
        baseline: &Baseline,
        as_of_millis: i64,
    ) -> Result<EfficacySnapshotData> {
        let platforms = self.all_platform_drift(persona_id, baseline).await?;
        Ok(EfficacySnapshotData::new(
            persona_id,
            as_of_millis,
            platforms,
        ))
    }

    /// Build AND export the efficacy snapshot for a persona to `format`,
    /// returning the in-memory [`ExportArtifact`] (bytes + metadata). The
    /// as-of date is embedded in every format. The artifact is the clean
    /// signing seam: a future ed25519 layer wraps it without reworking this.
    pub async fn export_efficacy_snapshot(
        &self,
        persona_id: &str,
        baseline: &Baseline,
        as_of_millis: i64,
        format: ExportFormat,
    ) -> Result<ExportArtifact> {
        let data = self
            .efficacy_snapshot_data(persona_id, baseline, as_of_millis)
            .await?;
        export_efficacy_snapshot(&data, format)
    }
}

/// A convenience grouping of a platform's built-in drift bundles by platform
/// label, for callers (CLI/JSON) that want a map rather than an ordered vec.
pub fn drift_by_label(bundles: Vec<PlatformDrift>) -> BTreeMap<String, PlatformDrift> {
    bundles
        .into_iter()
        .map(|b| (b.series.platform.label(), b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::registry::broker;
    use crate::brokers::{BrokerSubmission, SubmissionStatus};
    use crate::browser::AssignedTopic;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region, SyntheticPersona};
    use crate::store::{KeySource, TopicsMeasurement};
    use std::path::Path;
    use tempfile::tempdir;

    fn source(dir: &Path) -> KeySource {
        KeySource::EncryptedFile {
            path: dir.join("key.bin"),
            passphrase: "measurement-test".to_string(),
        }
    }

    fn open_store(dir: &Path) -> Result<Arc<Mutex<EncryptedStore>>> {
        let store = EncryptedStore::open_at(&dir.join("fauxx.db"), &source(dir))?;
        Ok(Arc::new(Mutex::new(store)))
    }

    fn persona(id: &str, interests: &[CategoryPool]) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            "M".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_000,
            2_000,
        )
    }

    fn topic(name: &str) -> AssignedTopic {
        AssignedTopic {
            topic_id: 1,
            taxonomy_version: None,
            model_version: None,
            version: None,
            name: Some(name.to_string()),
        }
    }

    #[tokio::test]
    async fn meta_platform_is_empty_no_data() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        let eng = MeasurementEngine::new(store);
        let bundle = eng
            .platform_drift(Platform::Meta, "any", &Baseline::FirstSnapshot)
            .await?;
        assert!(bundle.series.is_empty());
        assert!(bundle.heatmap.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn google_drift_from_stored_topics() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        {
            let guard = store.lock().await;
            // Two read-backs over time; the second drifts toward a new topic.
            guard.insert_topics_measurement(&TopicsMeasurement {
                persona_id: "p".into(),
                decoy_id: "d".into(),
                recorded_at: 100,
                available: true,
                topics: vec![topic("a"), topic("b")],
            })?;
            guard.insert_topics_measurement(&TopicsMeasurement {
                persona_id: "p".into(),
                decoy_id: "d".into(),
                recorded_at: 200,
                available: true,
                topics: vec![topic("a"), topic("c")],
            })?;
        }
        let eng = MeasurementEngine::new(store);
        let bundle = eng
            .platform_drift(Platform::Google, "p", &Baseline::FirstSnapshot)
            .await?;
        // Two points; first equals the baseline (~0 drift), second has drifted.
        assert_eq!(bundle.series.points.len(), 2);
        assert!(bundle.series.points[0].divergence.abs() < 1e-9);
        assert!(bundle.series.points[1].divergence > 0.0);
        Ok(())
    }

    #[tokio::test]
    async fn brokers_drift_from_submissions() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        let spokeo = broker("spokeo")?;
        let whitepages = broker("whitepages")?;
        {
            let guard = store.lock().await;
            // t=100: listed on spokeo. t=150: also on whitepages (baseline is the
            // first snapshot, spokeo-only). t=200: spokeo removed -> whitepages
            // only, a real shift across the two-broker support, so drift > 0.
            let s1 = BrokerSubmission::draft("1".into(), "spokeo", "p", spokeo, 100);
            let s2 = BrokerSubmission::draft("2".into(), "whitepages", "p", whitepages, 150);
            let mut s3 = BrokerSubmission::draft("3".into(), "spokeo", "p", spokeo, 200);
            s3.status = SubmissionStatus::Removed;
            guard.upsert_broker_submission(&s1)?;
            guard.upsert_broker_submission(&s2)?;
            guard.upsert_broker_submission(&s3)?;
        }
        let eng = MeasurementEngine::new(store);
        let bundle = eng
            .platform_drift(Platform::Brokers, "p", &Baseline::FirstSnapshot)
            .await?;
        assert_eq!(bundle.series.points.len(), 3);
        // The first snapshot equals the baseline (~0 drift); the last, after the
        // spokeo removal shifts mass to whitepages, has drifted.
        assert!(bundle.series.points[0].divergence.abs() < 1e-9);
        assert!(bundle.series.points[2].divergence > 0.0);
        Ok(())
    }

    #[tokio::test]
    async fn combined_devices_degrades_to_single_device() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        {
            let guard = store.lock().await;
            guard.insert_topics_measurement(&TopicsMeasurement {
                persona_id: "solo".into(),
                decoy_id: "d".into(),
                recorded_at: 100,
                available: true,
                topics: vec![topic("a")],
            })?;
        }
        let eng = MeasurementEngine::new(store);
        let combined = eng
            .combined_platform_drift(
                Platform::Google,
                &["solo".to_string()],
                &Baseline::FirstSnapshot,
            )
            .await?;
        assert_eq!(combined.series.points.len(), 1);
        // And an empty device list is graceful.
        let none = eng
            .combined_platform_drift(Platform::Google, &[], &Baseline::FirstSnapshot)
            .await?;
        assert!(none.series.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn shadow_profiles_round_trip_through_store() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        let eng = MeasurementEngine::new(store);

        assert!(eng.list_shadow_profiles().await?.is_empty());
        assert!(eng.get_shadow_profile("nope").await?.is_none());

        let p = persona("persona-1", &[CategoryPool::MUSIC]);
        let treated = ShadowProfile::treated("t1", "Treated", &p, 100);
        let control = ShadowProfile::control("c1", "Control", &p, 200);
        eng.save_shadow_profile(&treated).await?;
        eng.save_shadow_profile(&control).await?;

        let all = eng.list_shadow_profiles().await?;
        assert_eq!(all.len(), 2);
        // Newest-defined first.
        assert_eq!(all[0].id, "c1");

        let back = eng
            .get_shadow_profile("t1")
            .await?
            .ok_or_else(|| crate::CoreError::Key("t1 missing".into()))?;
        assert_eq!(back, treated);

        assert!(eng.delete_shadow_profile("t1").await?);
        assert!(!eng.delete_shadow_profile("t1").await?);
        assert_eq!(eng.list_shadow_profiles().await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn cohort_comparison_over_stored_profiles() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        // Treated persona has several drifting read-backs; control barely moves.
        {
            let guard = store.lock().await;
            // Treated: baseline {a,b}; then increasingly diverges.
            for (ts, topics) in [
                (100_i64, vec![topic("a"), topic("b")]),
                (200, vec![topic("a"), topic("c")]),
                (300, vec![topic("c"), topic("d")]),
                (400, vec![topic("d"), topic("e")]),
            ] {
                guard.insert_topics_measurement(&TopicsMeasurement {
                    persona_id: "treated-p".into(),
                    decoy_id: "d".into(),
                    recorded_at: ts,
                    available: true,
                    topics,
                })?;
            }
            // Control: baseline {a,b}; stays near it.
            for (ts, topics) in [
                (100_i64, vec![topic("a"), topic("b")]),
                (200, vec![topic("a"), topic("b")]),
                (300, vec![topic("a"), topic("b")]),
                (400, vec![topic("a"), topic("b")]),
            ] {
                guard.insert_topics_measurement(&TopicsMeasurement {
                    persona_id: "control-p".into(),
                    decoy_id: "d".into(),
                    recorded_at: ts,
                    available: true,
                    topics,
                })?;
            }
        }
        let eng = MeasurementEngine::new(store);
        let tp = persona("treated-p", &[CategoryPool::MUSIC]);
        let cp = persona("control-p", &[CategoryPool::MUSIC]);
        eng.save_shadow_profile(&ShadowProfile::treated("t", "Treated", &tp, 1))
            .await?;
        eng.save_shadow_profile(&ShadowProfile::control("c", "Control", &cp, 2))
            .await?;

        let cmp = eng
            .compare_shadow_cohorts(Platform::Google, &Baseline::FirstSnapshot, TTestKind::Welch)
            .await?;
        // Treated drifted more on average than control.
        assert!(cmp.treated.mean > cmp.control.mean);
        assert!(cmp.direction.contains("treated profile drifted more"));
        assert!(!cmp.summary.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn all_platform_drift_lists_builtins() -> Result<()> {
        let dir = tempdir()?;
        let store = open_store(dir.path())?;
        let eng = MeasurementEngine::new(store);
        let bundles = eng
            .all_platform_drift("p", &Baseline::FirstSnapshot)
            .await?;
        assert_eq!(bundles.len(), 3);
        let labels: Vec<String> = bundles.iter().map(|b| b.series.platform.label()).collect();
        assert_eq!(labels, vec!["Google", "Brokers", "Meta"]);
        let by_label = drift_by_label(bundles);
        assert!(by_label.contains_key("Meta"));
        Ok(())
    }
}
