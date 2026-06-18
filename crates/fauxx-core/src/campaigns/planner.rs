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

//! Campaign closed-loop planning (C8 #33, U2): the metric source seam, the
//! gap-to-intensity mapping with back-off, and the persisting planner.
//!
//! The [`MetricSource`] trait is the closed-loop signal the planner reads each
//! tick. Production wires it to the C4 [`MeasurementEngine`](crate::measurement);
//! tests inject a [`StubMetricSource`] so the campaign logic is verified without
//! a live measurement run. [`directive_for_gap`] is the pure mapping every tick
//! uses; [`CampaignPlanner`] is the persisting orchestration over a store.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{Campaign, CampaignStatus, TargetMetric};
use crate::error::{CoreError, Result};
use crate::orchestration::IntensityLevel;
use crate::store::EncryptedStore;

/// The closed-loop signal a campaign reads each tick (C8 #33).
///
/// `metric_value` returns the current value of `metric` for a persona's target
/// segment. The live implementation reads the C4 A1 drift series; tests inject a
/// [`StubMetricSource`]. Object-safe (`async_trait`, `Send + Sync`) so the
/// planner can hold a `Box<dyn MetricSource>`.
#[async_trait]
pub trait MetricSource: Send + Sync {
    /// The current value of `metric` for `persona_id`'s `segment`, or `None`
    /// when there is no data yet (the planner holds intensity steady, never
    /// fabricating progress against a missing signal).
    async fn metric_value(
        &self,
        metric: TargetMetric,
        persona_id: &str,
        segment: &str,
    ) -> Result<Option<f64>>;
}

/// A test metric source whose value is set explicitly (C8 #33). Lets the unit
/// tests drive the closed loop without a live measurement run.
#[derive(Debug, Clone, Default)]
pub struct StubMetricSource {
    value: Arc<Mutex<Option<f64>>>,
}

impl StubMetricSource {
    /// A stub fixed at an initial value (`None` = no data yet).
    pub fn new(initial: Option<f64>) -> Self {
        Self {
            value: Arc::new(Mutex::new(initial)),
        }
    }

    /// Overwrite the value the next [`MetricSource::metric_value`] returns.
    pub async fn set(&self, value: Option<f64>) {
        *self.value.lock().await = value;
    }
}

#[async_trait]
impl MetricSource for StubMetricSource {
    async fn metric_value(
        &self,
        _metric: TargetMetric,
        _persona_id: &str,
        _segment: &str,
    ) -> Result<Option<f64>> {
        Ok(*self.value.lock().await)
    }
}

/// The scheduler directive a campaign emits for one tick (C8 #33).
///
/// It carries the [`IntensityLevel`] the C1 timeline scheduler should run at
/// (or `None` to idle) and a topic-selection bias toward the target segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignDirective {
    /// The intensity to run at, or `None` when the campaign is not driving
    /// activity this tick (planned/paused/achieved, or goal already met).
    pub intensity: Option<IntensityLevel>,
    /// The target segment decoy topic selection should bias toward. Empty when
    /// the directive is idle.
    pub target_segment: String,
}

impl CampaignDirective {
    /// An idle directive: no scheduling, no bias.
    pub fn idle() -> Self {
        Self {
            intensity: None,
            target_segment: String::new(),
        }
    }

    /// A running directive at `intensity` biased toward `segment`.
    pub fn running(intensity: IntensityLevel, segment: impl Into<String>) -> Self {
        Self {
            intensity: Some(intensity),
            target_segment: segment.into(),
        }
    }

    /// Whether this directive drives any decoy activity.
    pub fn is_running(&self) -> bool {
        self.intensity.is_some()
    }

    /// Reorder `categories` so this directive's target segment LEADS, the decoy
    /// topic-selection bias: the targeted segment first, then the rest in their
    /// original first-seen order. An idle directive, an empty target segment, or a
    /// segment NOT present in `categories` returns the list unchanged (the campaign
    /// can only bias toward a segment the persona actually carries). Pure: the
    /// caller resolves the reordered categories to sites so the targeted segment's
    /// pages lead, and dominate, the capped decoy plan.
    pub fn bias_categories(&self, categories: &[String]) -> Vec<String> {
        if self.target_segment.is_empty() || !categories.iter().any(|c| c == &self.target_segment) {
            return categories.to_vec();
        }
        let mut out = Vec::with_capacity(categories.len());
        out.push(self.target_segment.clone());
        out.extend(
            categories
                .iter()
                .filter(|c| *c != &self.target_segment)
                .cloned(),
        );
        out
    }
}

/// The per-second action rate a directive implies (an idle directive is `0.0`),
/// used to rank which of several running campaigns steers a shared persona.
fn directive_rate(d: &CampaignDirective) -> f64 {
    d.intensity.map(|i| i.rate_per_second()).unwrap_or(0.0)
}

/// The gap (in metric units) at or below which the campaign BACKS OFF to the
/// gentlest intensity: the metric is close enough that hammering it risks
/// overshoot, so the loop eases off and lets the dwell confirm the goal.
pub const BACKOFF_GAP: f64 = 0.1;

/// The gap above which the campaign drives at the highest intensity: a wide gap
/// means a lot of steering is needed, so push hard.
pub const FAR_GAP: f64 = 1.0;

/// Map a signed `gap` (see [`super::Comparator::gap`]) to a scheduler directive
/// for the target `segment` (C8 #33). This is the gap-to-intensity mapping the
/// closed loop applies each tick, with explicit BACK-OFF near the threshold:
///
/// - gap `<= 0`: the goal is met THIS tick; emit a gentle [`IntensityLevel::Low`]
///   holding directive (the dwell logic in [`Campaign::tick`] decides Achieved;
///   the loop should not slam to zero on the first satisfied tick).
/// - `0 < gap <= BACKOFF_GAP`: near the threshold; back off to `Low`.
/// - `BACKOFF_GAP < gap <= mid`: `Medium`.
/// - `mid < gap <= FAR_GAP`: `High`.
/// - gap `> FAR_GAP`: far from the goal; `Extreme`.
///
/// A non-finite `gap` (which cannot arise from a finite-threshold goal and a
/// finite observation, but is handled defensively) maps to `Low`.
pub fn directive_for_gap(gap: f64, segment: &str) -> CampaignDirective {
    if !gap.is_finite() {
        return CampaignDirective::running(IntensityLevel::Low, segment);
    }
    // The midpoint between the back-off band and the far band.
    let mid = (BACKOFF_GAP + FAR_GAP) / 2.0;
    let level = if gap <= BACKOFF_GAP {
        // At or under the back-off band (including a met goal, gap <= 0).
        IntensityLevel::Low
    } else if gap <= mid {
        IntensityLevel::Medium
    } else if gap <= FAR_GAP {
        IntensityLevel::High
    } else {
        IntensityLevel::Extreme
    };
    CampaignDirective::running(level, segment)
}

/// The persisting campaign planner (C8 #33). Owns the campaign store access and
/// the closed-loop metric source; drives a campaign one tick and persists the
/// advanced state so it survives restart.
///
/// Cheap to clone (shared state is behind `Arc`). Holds NO GUI/CLI types.
#[derive(Clone)]
pub struct CampaignPlanner {
    store: Arc<Mutex<EncryptedStore>>,
    metrics: Arc<dyn MetricSource>,
}

impl std::fmt::Debug for CampaignPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CampaignPlanner").finish_non_exhaustive()
    }
}

impl CampaignPlanner {
    /// Build a planner over a shared store and a closed-loop metric source.
    pub fn new(store: Arc<Mutex<EncryptedStore>>, metrics: Arc<dyn MetricSource>) -> Self {
        Self { store, metrics }
    }

    /// Persist (insert or replace) a campaign definition + progress.
    pub async fn save(&self, campaign: &Campaign) -> Result<()> {
        self.store.lock().await.upsert_campaign(campaign)
    }

    /// Fetch a campaign by id, or `None` if absent.
    pub async fn get(&self, id: &str) -> Result<Option<Campaign>> {
        self.store.lock().await.get_campaign(id)
    }

    /// List campaigns. Scoped to a persona when `persona_id` is `Some`; else all.
    pub async fn list(&self, persona_id: Option<&str>) -> Result<Vec<Campaign>> {
        self.store.lock().await.list_campaigns(persona_id)
    }

    /// Delete a campaign by id. Returns `true` if a row was removed.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        self.store.lock().await.delete_campaign(id)
    }

    /// Start a campaign (`Planned`/`Paused` -> `Running`), persisting it.
    pub async fn start(&self, id: &str, now: i64) -> Result<Campaign> {
        let mut campaign = self.require(id).await?;
        campaign.start(now)?;
        self.save(&campaign).await?;
        Ok(campaign)
    }

    /// Pause a campaign on user request, persisting it.
    pub async fn pause(&self, id: &str, now: i64) -> Result<Campaign> {
        let mut campaign = self.require(id).await?;
        campaign.pause(now);
        self.save(&campaign).await?;
        Ok(campaign)
    }

    /// Adjust a campaign's goal threshold (the HA "adjust" command), persisting
    /// it. Rejects a non-finite threshold. Resets the satisfied-dwell window so
    /// the new threshold is earned afresh; leaves the lifecycle status untouched.
    pub async fn adjust_threshold(&self, id: &str, threshold: f64, now: i64) -> Result<Campaign> {
        if !threshold.is_finite() {
            return Err(CoreError::Campaign(
                "adjusted threshold must be a finite number".to_string(),
            ));
        }
        let mut campaign = self.require(id).await?;
        // Moving the goalpost re-earns the dwell honestly: a window that was
        // accumulating toward success under the prior threshold must not carry
        // over (otherwise hardening the threshold could flip to Achieved on a
        // dwell earned against the easier goal). Mirrors pause()'s dwell reset.
        campaign.progress.satisfied_since = None;
        campaign.goal.threshold = threshold;
        campaign.updated_at = now;
        self.save(&campaign).await?;
        Ok(campaign)
    }

    /// Advance one campaign's closed loop: read the current metric for its
    /// target segment, [`Campaign::tick`] it, persist the advanced state, and
    /// return the scheduler [`CampaignDirective`].
    ///
    /// A missing metric (no data yet) holds the campaign steady: it records no
    /// progress and emits an idle directive rather than fabricating a gap.
    pub async fn tick(&self, id: &str, now: i64) -> Result<CampaignDirective> {
        let mut campaign = self.require(id).await?;
        let observed = self
            .metrics
            .metric_value(
                campaign.goal.metric,
                &campaign.persona_id,
                &campaign.target_segment,
            )
            .await?;
        let directive = match observed {
            Some(value) => campaign.tick(value, now),
            // No closed-loop data yet: do not advance the dwell or invent a gap.
            None => CampaignDirective::idle(),
        };
        self.save(&campaign).await?;
        Ok(directive)
    }

    /// The directive that should steer `persona_id`'s decoy activity RIGHT NOW,
    /// derived from its running campaigns' PERSISTED progress (no tick, no metric
    /// read). Plan builders (the household schedule, the extension decoy plan)
    /// consult this so gap-to-goal sets scheduler intensity and biases decoy topic
    /// selection toward the target segment, with the back-off already baked into
    /// [`directive_for_gap`].
    ///
    /// With multiple running campaigns for one persona a plan can bias toward only
    /// one segment at a time, so the MOST AGGRESSIVE directive wins (the widest gap
    /// needs the most steering); ties break by the lower campaign id for a
    /// deterministic choice. Returns [`CampaignDirective::idle`] when the persona
    /// has no running campaign currently driving activity.
    pub async fn effective_directive(&self, persona_id: &str) -> Result<CampaignDirective> {
        let campaigns = self.list(Some(persona_id)).await?;
        let mut best: Option<(String, CampaignDirective)> = None;
        for campaign in &campaigns {
            let directive = campaign.current_directive();
            if !directive.is_running() {
                continue;
            }
            let replace = match &best {
                None => true,
                Some((best_id, best_directive)) => {
                    let rate = directive_rate(&directive);
                    let best_rate = directive_rate(best_directive);
                    rate > best_rate || (rate == best_rate && campaign.id < *best_id)
                }
            };
            if replace {
                best = Some((campaign.id.clone(), directive));
            }
        }
        Ok(best.map(|(_, d)| d).unwrap_or_else(CampaignDirective::idle))
    }

    /// Tick every `Running` campaign, returning each `(id, directive)`.
    pub async fn tick_all_running(&self, now: i64) -> Result<Vec<(String, CampaignDirective)>> {
        let running: Vec<Campaign> = self
            .list(None)
            .await?
            .into_iter()
            .filter(|c| c.status == CampaignStatus::Running)
            .collect();
        let mut out = Vec::with_capacity(running.len());
        for campaign in running {
            let directive = self.tick(&campaign.id, now).await?;
            out.push((campaign.id, directive));
        }
        Ok(out)
    }

    /// Load a campaign or fail with [`CoreError::Campaign`] (a tick/command on a
    /// missing campaign is a logic error, not a silent no-op).
    async fn require(&self, id: &str) -> Result<Campaign> {
        self.get(id)
            .await?
            .ok_or_else(|| CoreError::Campaign(format!("no such campaign {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaigns::{Comparator, Goal};

    #[test]
    fn gap_to_intensity_mapping_is_monotonic_with_backoff() {
        // A met goal (gap <= 0) holds gently at Low (the dwell decides Achieved).
        assert_eq!(
            directive_for_gap(-0.5, "TECHNOLOGY").intensity,
            Some(IntensityLevel::Low)
        );
        assert_eq!(
            directive_for_gap(0.0, "TECHNOLOGY").intensity,
            Some(IntensityLevel::Low)
        );
        // Near the threshold (within BACKOFF_GAP): backs off to Low.
        assert_eq!(
            directive_for_gap(BACKOFF_GAP, "TECHNOLOGY").intensity,
            Some(IntensityLevel::Low)
        );
        // Progressively wider gaps step the ladder up.
        assert_eq!(
            directive_for_gap(0.3, "TECHNOLOGY").intensity,
            Some(IntensityLevel::Medium)
        );
        assert_eq!(
            directive_for_gap(0.9, "TECHNOLOGY").intensity,
            Some(IntensityLevel::High)
        );
        // Far from the goal: full intensity.
        assert_eq!(
            directive_for_gap(5.0, "TECHNOLOGY").intensity,
            Some(IntensityLevel::Extreme)
        );
    }

    #[test]
    fn bias_categories_leads_with_the_target_segment() {
        let cats = vec![
            "TECHNOLOGY".to_string(),
            "SCIENCE".to_string(),
            "GAMING".to_string(),
        ];
        // A segment present in the list leads; the rest keep their order.
        let d = directive_for_gap(2.0, "GAMING");
        assert_eq!(
            d.bias_categories(&cats),
            vec![
                "GAMING".to_string(),
                "TECHNOLOGY".to_string(),
                "SCIENCE".to_string()
            ]
        );
        // A segment NOT in the list leaves the order unchanged (can only bias
        // toward a segment the persona actually carries).
        assert_eq!(
            directive_for_gap(2.0, "FINANCE").bias_categories(&cats),
            cats
        );
        // An idle directive never reorders.
        assert_eq!(CampaignDirective::idle().bias_categories(&cats), cats);
    }

    #[test]
    fn directive_carries_the_target_segment_bias() {
        let d = directive_for_gap(2.0, "FINANCE");
        assert_eq!(d.target_segment, "FINANCE");
        assert!(d.is_running());
        let idle = CampaignDirective::idle();
        assert!(!idle.is_running());
        assert!(idle.target_segment.is_empty());
    }

    #[test]
    fn back_off_holds_as_metric_approaches_threshold() -> Result<()> {
        // Drive the TECH segment's drift UP to at least 1.0. As the observed
        // value climbs toward 1.0 the gap shrinks and the intensity backs off.
        let goal = Goal::new(TargetMetric::SegmentDrift, Comparator::AtLeast, 1.0)?;
        // Far below: large gap -> Extreme.
        assert_eq!(
            directive_for_gap(goal.gap(-1.5), "TECHNOLOGY").intensity,
            Some(IntensityLevel::Extreme)
        );
        // Closing in (0.7 -> gap 0.3) -> Medium.
        assert_eq!(
            directive_for_gap(goal.gap(0.7), "TECHNOLOGY").intensity,
            Some(IntensityLevel::Medium)
        );
        // Almost there (0.95 -> gap 0.05) -> back off to Low.
        assert_eq!(
            directive_for_gap(goal.gap(0.95), "TECHNOLOGY").intensity,
            Some(IntensityLevel::Low)
        );
        Ok(())
    }

    #[tokio::test]
    async fn stub_metric_source_reads_then_updates() -> Result<()> {
        let src = StubMetricSource::new(None);
        assert_eq!(
            src.metric_value(TargetMetric::SegmentDrift, "p1", "TECHNOLOGY")
                .await?,
            None
        );
        src.set(Some(0.42)).await;
        assert_eq!(
            src.metric_value(TargetMetric::SegmentDrift, "p1", "TECHNOLOGY")
                .await?,
            Some(0.42)
        );
        Ok(())
    }
}
