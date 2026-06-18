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

//! The live closed-loop metric source (C8 #33, U2): the bridge from the C4 A1
//! drift analytics to a campaign's per-segment signal.
//!
//! [`MeasurementMetricSource`] implements [`MetricSource`] over the C4
//! [`MeasurementEngine`]. For [`TargetMetric::SegmentDrift`] it reads the A1
//! per-category drift heatmap for the persona (the Google Topics platform, the
//! desktop's primary closed loop) and returns the MOST RECENT drift contribution
//! for the target segment. No data yet yields `None`, which the planner treats
//! as "hold steady" rather than fabricating progress.

use std::sync::Arc;

use async_trait::async_trait;

use super::planner::MetricSource;
use super::TargetMetric;
use crate::error::Result;
use crate::measurement::{Baseline, MeasurementEngine, Platform};
use crate::store::EncryptedStore;
use tokio::sync::Mutex;

/// The live metric source backed by the C4 A1 drift analytics (C8 #33).
#[derive(Clone)]
pub struct MeasurementMetricSource {
    engine: MeasurementEngine,
    store: Arc<Mutex<EncryptedStore>>,
}

impl std::fmt::Debug for MeasurementMetricSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeasurementMetricSource")
            .finish_non_exhaustive()
    }
}

impl MeasurementMetricSource {
    /// Build over the C4 engine and the shared store (the store resolves the
    /// persona for the preferred persona-intent baseline).
    pub fn new(engine: MeasurementEngine, store: Arc<Mutex<EncryptedStore>>) -> Self {
        Self { engine, store }
    }

    /// The most recent A1 drift contribution for `segment` on the Google Topics
    /// platform for `persona_id`, or `None` when there is no read-back history
    /// for that segment yet.
    async fn segment_drift(&self, persona_id: &str, segment: &str) -> Result<Option<f64>> {
        // The preferred baseline is the persona's declared intent; if the
        // persona is unknown, fall back to the first observed snapshot so the
        // loop still has a reference.
        let persona = self.store.lock().await.get_persona(persona_id)?;
        let baseline = match persona {
            Some(p) => Baseline::from_persona(&p),
            None => Baseline::FirstSnapshot,
        };
        let bundle = self
            .engine
            .platform_drift(Platform::Google, persona_id, &baseline)
            .await?;
        // The heatmap row for the target segment carries that segment's drift
        // contribution at each timestamp; the last entry is the current signal.
        Ok(bundle
            .heatmap
            .rows
            .get(segment)
            .and_then(|row| row.last().copied()))
    }
}

#[async_trait]
impl MetricSource for MeasurementMetricSource {
    async fn metric_value(
        &self,
        metric: TargetMetric,
        persona_id: &str,
        segment: &str,
    ) -> Result<Option<f64>> {
        match metric {
            TargetMetric::SegmentDrift => self.segment_drift(persona_id, segment).await,
        }
    }
}
