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

//! Goal-driven campaigns (C8 #33, U2), the closed loop.
//!
//! A [`Campaign`] turns the companion from a raw noise generator into something
//! that AIMS at a measurable target: "drop the TECH segment's drift below X" and
//! stop once it holds. It is fed by the C4 A1 closed-loop signal (the
//! KL-divergence drift metric for the targeted segment), computes the GAP to the
//! goal each tick, and translates that gap into scheduler intensity and a decoy
//! topic bias toward the target segment, BACKING OFF as the metric approaches
//! the threshold.
//!
//! ## Model
//!
//! - [`Goal`]: a [`TargetMetric`], a [`Comparator`], and a threshold.
//! - [`Campaign`]: a goal, a target segment/category, the persona it drives, a
//!   [`CampaignStatus`] lifecycle, and the closed-loop [`CampaignProgress`].
//! - [`CampaignStatus`]: `Planned -> Running -> Achieved`, with `Paused` on user
//!   request from any active state.
//!
//! ## Closed loop
//!
//! Each tick the planner reads the current metric for the target segment from a
//! [`MetricSource`] (the live source is the C4 [`MeasurementEngine`]; tests
//! inject a [`StubMetricSource`]), then [`Campaign::tick`]:
//!
//! 1. computes the signed [`gap`](CampaignProgress::gap) to the threshold,
//! 2. maps the gap to a scheduler [`CampaignDirective`] (intensity +
//!    target-segment bias), backing off near the threshold,
//! 3. advances the lifecycle: the goal holding for a configurable DWELL marks
//!    the campaign `Achieved`; otherwise it stays `Running`.
//!
//! ## Persistence
//!
//! Campaigns and their progress persist in the `campaigns` table (schema
//! v14 -> v15) so they survive restart with the dwell clock intact (a box that
//! reboots mid-campaign resumes, rather than restarting the goal).

pub mod metric;
pub mod planner;

pub use metric::MeasurementMetricSource;
pub use planner::{
    directive_for_gap, CampaignDirective, CampaignPlanner, MetricSource, StubMetricSource,
    BACKOFF_GAP, FAR_GAP,
};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The metric a campaign goal is evaluated against (C8 #33).
///
/// Today the only closed-loop signal is the C4 A1 KL-divergence drift for a
/// segment; the enum is `#[non_exhaustive]` so future signals (confidence
/// scores, A/B effect size) can be added without breaking persisted goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TargetMetric {
    /// The A1 KL-divergence drift for the target segment (higher = more drift
    /// from the baseline, i.e. a more steered profile). A campaign typically
    /// aims to RAISE this (steer the segment away from intent) or LOWER it
    /// (collapse a segment's confidence back toward the noise floor).
    SegmentDrift,
}

impl TargetMetric {
    /// The stable string form persisted in the goal JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetMetric::SegmentDrift => "segment_drift",
        }
    }
}

/// How the observed metric is compared to the goal threshold (C8 #33).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparator {
    /// Goal met when `observed >= threshold` (drive the metric UP).
    AtLeast,
    /// Goal met when `observed <= threshold` (drive the metric DOWN).
    AtMost,
}

impl Comparator {
    /// Whether `observed` satisfies the goal at `threshold`.
    pub fn satisfied(&self, observed: f64, threshold: f64) -> bool {
        match self {
            Comparator::AtLeast => observed >= threshold,
            Comparator::AtMost => observed <= threshold,
        }
    }

    /// The SIGNED remaining distance to the threshold, in the direction the
    /// goal drives, so a satisfied goal is `<= 0` and an unmet goal is `> 0`.
    ///
    /// - [`Comparator::AtLeast`]: `threshold - observed` (positive while still
    ///   below the bar we are trying to reach).
    /// - [`Comparator::AtMost`]: `observed - threshold` (positive while still
    ///   above the ceiling we are trying to get under).
    pub fn gap(&self, observed: f64, threshold: f64) -> f64 {
        match self {
            Comparator::AtLeast => threshold - observed,
            Comparator::AtMost => observed - threshold,
        }
    }
}

/// A campaign goal: a metric, a comparator, and a threshold (C8 #33).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    /// Which closed-loop metric this goal targets.
    pub metric: TargetMetric,
    /// How the observed value is compared to [`Goal::threshold`].
    pub comparator: Comparator,
    /// The target threshold value. Must be finite.
    pub threshold: f64,
}

impl Goal {
    /// Build a goal, rejecting a non-finite threshold (fail closed: a NaN/inf
    /// threshold could never be evaluated and would corrupt the gap math).
    pub fn new(metric: TargetMetric, comparator: Comparator, threshold: f64) -> Result<Self> {
        if !threshold.is_finite() {
            return Err(CoreError::Campaign(
                "campaign goal threshold must be a finite number".to_string(),
            ));
        }
        Ok(Self {
            metric,
            comparator,
            threshold,
        })
    }

    /// Whether `observed` satisfies this goal.
    pub fn satisfied(&self, observed: f64) -> bool {
        self.comparator.satisfied(observed, self.threshold)
    }

    /// The signed gap to the threshold (see [`Comparator::gap`]).
    pub fn gap(&self, observed: f64) -> f64 {
        self.comparator.gap(observed, self.threshold)
    }
}

/// The campaign lifecycle (C8 #33).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignStatus {
    /// Defined but not yet driving the scheduler.
    Planned,
    /// Actively driving the scheduler and being re-evaluated each tick.
    Running,
    /// The goal has held for the configured dwell; no longer driving activity.
    Achieved,
    /// Paused on user request; not driving the scheduler until resumed.
    Paused,
}

impl CampaignStatus {
    /// The stable string form persisted in the scalar `status` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            CampaignStatus::Planned => "planned",
            CampaignStatus::Running => "running",
            CampaignStatus::Achieved => "achieved",
            CampaignStatus::Paused => "paused",
        }
    }
}

/// The closed-loop progress carried by a running campaign (C8 #33).
///
/// Persisted with the campaign so the dwell clock survives restart.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignProgress {
    /// The last observed metric value, or `None` before the first tick.
    pub last_metric: Option<f64>,
    /// The last computed signed gap (`<= 0` means satisfied), or `None` before
    /// the first tick.
    pub last_gap: Option<f64>,
    /// Epoch-millis timestamp the goal FIRST became continuously satisfied in
    /// the current run, or `None` while it is not satisfied. The Achieved dwell
    /// is measured from here.
    pub satisfied_since: Option<i64>,
    /// Epoch-millis timestamp of the most recent tick, or `None` before any.
    pub last_tick_at: Option<i64>,
}

impl CampaignProgress {
    /// The signed gap from the most recent tick, if any.
    pub fn gap(&self) -> Option<f64> {
        self.last_gap
    }
}

/// Default dwell (ms) the goal must hold continuously before Achieved: ten
/// minutes. Long enough that a single noisy tick does not trip Achieved.
pub const DEFAULT_DWELL_MS: i64 = 10 * 60 * 1_000;

/// A goal-driven campaign (C8 #33). The persisted, restart-surviving unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Campaign {
    /// Stable id (a UUID), the primary key.
    pub id: String,
    /// Human-readable label for the campaign.
    pub label: String,
    /// The persona this campaign drives.
    pub persona_id: String,
    /// The target segment/category name (a `CategoryPool` name, e.g. the TECH
    /// segment). Decoy topic selection biases toward this segment.
    pub target_segment: String,
    /// The goal being driven toward.
    pub goal: Goal,
    /// How long (ms) the goal must hold continuously before Achieved.
    pub dwell_ms: i64,
    /// The current lifecycle state.
    pub status: CampaignStatus,
    /// The closed-loop progress (persisted so the dwell survives restart).
    pub progress: CampaignProgress,
    /// Epoch millis the campaign was created.
    pub created_at: i64,
    /// Epoch millis of the most recent state/progress change.
    pub updated_at: i64,
}

impl Campaign {
    /// Build a new `Planned` campaign with the default dwell.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        persona_id: impl Into<String>,
        target_segment: impl Into<String>,
        goal: Goal,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            persona_id: persona_id.into(),
            target_segment: target_segment.into(),
            goal,
            dwell_ms: DEFAULT_DWELL_MS,
            status: CampaignStatus::Planned,
            progress: CampaignProgress::default(),
            created_at,
            updated_at: created_at,
        }
    }

    /// Override the Achieved dwell (ms). A negative value is clamped to zero
    /// (Achieved the first satisfied tick).
    pub fn with_dwell_ms(mut self, dwell_ms: i64) -> Self {
        self.dwell_ms = dwell_ms.max(0);
        self
    }

    /// Whether this campaign is in an active (scheduler-driving) state.
    pub fn is_active(&self) -> bool {
        matches!(self.status, CampaignStatus::Running)
    }

    /// Transition `Planned`/`Paused` -> `Running`. Idempotent for `Running`.
    /// Errors if the campaign is already `Achieved` (a met goal is not re-run;
    /// the caller starts a fresh campaign instead).
    pub fn start(&mut self, now: i64) -> Result<()> {
        match self.status {
            CampaignStatus::Planned | CampaignStatus::Paused | CampaignStatus::Running => {
                self.status = CampaignStatus::Running;
                self.touch(now);
                Ok(())
            }
            CampaignStatus::Achieved => Err(CoreError::Campaign(format!(
                "campaign {} is already achieved and cannot be restarted",
                self.id
            ))),
        }
    }

    /// Transition to `Paused` on user request from any active state. Idempotent.
    /// The dwell clock is reset so a resume re-earns the dwell honestly rather
    /// than counting paused time toward Achieved.
    pub fn pause(&mut self, now: i64) {
        self.status = CampaignStatus::Paused;
        self.progress.satisfied_since = None;
        self.touch(now);
    }

    /// Bump `updated_at`.
    fn touch(&mut self, now: i64) {
        self.updated_at = now;
    }

    /// Advance the closed loop one tick against the freshly-observed metric
    /// `observed` at wall-clock `now` (epoch millis). Returns the directive the
    /// scheduler should apply this tick.
    ///
    /// Only a `Running` campaign drives activity: a `Planned`/`Paused`/`Achieved`
    /// campaign records the observation but emits [`CampaignDirective::idle`]
    /// (no scheduling). A `Running` campaign:
    ///
    /// 1. records `observed` and the signed gap,
    /// 2. tracks the continuously-satisfied window for the dwell,
    /// 3. flips to `Achieved` once the goal has held for `dwell_ms`,
    /// 4. otherwise maps the gap to intensity + target-segment bias, backing off
    ///    as the metric nears the threshold.
    pub fn tick(&mut self, observed: f64, now: i64) -> CampaignDirective {
        let gap = self.goal.gap(observed);
        self.progress.last_metric = Some(observed);
        self.progress.last_gap = Some(gap);
        self.progress.last_tick_at = Some(now);

        if self.status != CampaignStatus::Running {
            self.touch(now);
            return CampaignDirective::idle();
        }

        let satisfied = self.goal.satisfied(observed);
        if satisfied {
            // Start (or continue) the continuously-satisfied window.
            let since = *self.progress.satisfied_since.get_or_insert(now);
            if now.saturating_sub(since) >= self.dwell_ms {
                // Held for the full dwell: the goal is achieved, stop driving.
                self.status = CampaignStatus::Achieved;
                self.touch(now);
                return CampaignDirective::idle();
            }
        } else {
            // The goal slipped: reset the dwell clock.
            self.progress.satisfied_since = None;
        }

        self.touch(now);
        planner::directive_for_gap(gap, &self.target_segment)
    }

    /// The scheduler directive implied by this campaign's CURRENT persisted state,
    /// WITHOUT advancing the loop (no metric read, no dwell change). Plan builders
    /// consult this BETWEEN ticks so the last computed gap keeps steering activity
    /// (scheduler intensity + decoy topic bias) until the next tick refreshes it.
    ///
    /// Idle unless the campaign is `Running` and has ticked at least once (its
    /// `last_gap` is set); then it maps that persisted gap to intensity + a
    /// target-segment bias via [`directive_for_gap`], exactly as the most recent
    /// [`tick`](Campaign::tick) did. A `Planned`/`Paused`/`Achieved` campaign, or a
    /// `Running` one not yet ticked, drives nothing.
    pub fn current_directive(&self) -> CampaignDirective {
        if self.status != CampaignStatus::Running {
            return CampaignDirective::idle();
        }
        match self.progress.last_gap {
            Some(gap) => planner::directive_for_gap(gap, &self.target_segment),
            None => CampaignDirective::idle(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drift_goal(comparator: Comparator, threshold: f64) -> Result<Goal> {
        Goal::new(TargetMetric::SegmentDrift, comparator, threshold)
    }

    #[test]
    fn goal_rejects_non_finite_threshold() {
        assert!(matches!(
            Goal::new(TargetMetric::SegmentDrift, Comparator::AtLeast, f64::NAN),
            Err(CoreError::Campaign(_))
        ));
        assert!(matches!(
            Goal::new(
                TargetMetric::SegmentDrift,
                Comparator::AtMost,
                f64::INFINITY
            ),
            Err(CoreError::Campaign(_))
        ));
    }

    #[test]
    fn gap_is_signed_toward_the_goal() -> Result<()> {
        // AtLeast 0.5: below the bar is a positive gap, at/above is <= 0.
        let up = drift_goal(Comparator::AtLeast, 0.5)?;
        assert!((up.gap(0.2) - 0.3).abs() < 1e-12);
        assert!(up.gap(0.5) <= 0.0);
        assert!(up.gap(0.8) < 0.0);
        assert!(!up.satisfied(0.2));
        assert!(up.satisfied(0.5));

        // AtMost 0.5: above the ceiling is a positive gap, at/below is <= 0.
        let down = drift_goal(Comparator::AtMost, 0.5)?;
        assert!((down.gap(0.8) - 0.3).abs() < 1e-12);
        assert!(down.gap(0.5) <= 0.0);
        assert!(down.satisfied(0.4));
        assert!(!down.satisfied(0.6));
        Ok(())
    }

    #[test]
    fn planned_campaign_does_not_drive_until_started() -> Result<()> {
        let mut c = Campaign::new(
            "c1",
            "TECH down",
            "p1",
            "TECHNOLOGY",
            drift_goal(Comparator::AtLeast, 1.0)?,
            1_000,
        );
        // Planned: a tick records but emits idle.
        let d = c.tick(0.0, 2_000);
        assert!(!d.is_running());
        assert_eq!(c.status, CampaignStatus::Planned);
        assert_eq!(c.progress.last_metric, Some(0.0));

        // Start, then it drives.
        c.start(3_000)?;
        assert_eq!(c.status, CampaignStatus::Running);
        let d = c.tick(0.0, 4_000);
        assert!(d.is_running());
        Ok(())
    }

    #[test]
    fn achieved_after_dwell_and_pause_resets_dwell() -> Result<()> {
        let goal = drift_goal(Comparator::AtMost, 0.5)?;
        let mut c =
            Campaign::new("c2", "TECH cap", "p1", "TECHNOLOGY", goal, 0).with_dwell_ms(1_000);
        c.start(0)?;

        // Satisfied at t=0 (observed 0.4 <= 0.5): starts the dwell window.
        let d = c.tick(0.4, 0);
        assert!(d.is_running(), "still driving during the dwell");
        assert_eq!(c.status, CampaignStatus::Running);
        assert_eq!(c.progress.satisfied_since, Some(0));

        // Still satisfied but dwell not elapsed: keeps running.
        c.tick(0.45, 500);
        assert_eq!(c.status, CampaignStatus::Running);

        // Dwell elapsed while continuously satisfied: Achieved, idle directive.
        let d = c.tick(0.45, 1_000);
        assert!(!d.is_running());
        assert_eq!(c.status, CampaignStatus::Achieved);

        // Achieved cannot be restarted.
        assert!(matches!(c.start(2_000), Err(CoreError::Campaign(_))));
        Ok(())
    }

    #[test]
    fn slipping_goal_resets_the_dwell_clock() -> Result<()> {
        let goal = drift_goal(Comparator::AtMost, 0.5)?;
        let mut c =
            Campaign::new("c3", "TECH cap", "p1", "TECHNOLOGY", goal, 0).with_dwell_ms(1_000);
        c.start(0)?;
        c.tick(0.4, 0); // satisfied, window opens at 0
        assert_eq!(c.progress.satisfied_since, Some(0));
        c.tick(0.9, 500); // slips above the ceiling: window resets
        assert_eq!(c.progress.satisfied_since, None);
        assert_eq!(c.status, CampaignStatus::Running);
        c.tick(0.3, 700); // satisfied again: window reopens at the new time
        assert_eq!(c.progress.satisfied_since, Some(700));
        // The earlier satisfied time did NOT count toward the dwell.
        c.tick(0.3, 1_500);
        assert_eq!(c.status, CampaignStatus::Running);
        Ok(())
    }

    #[test]
    fn pause_from_running_clears_dwell_and_stops_driving() -> Result<()> {
        let goal = drift_goal(Comparator::AtMost, 0.5)?;
        let mut c =
            Campaign::new("c4", "TECH cap", "p1", "TECHNOLOGY", goal, 0).with_dwell_ms(10_000);
        c.start(0)?;
        c.tick(0.4, 0);
        assert!(c.progress.satisfied_since.is_some());
        c.pause(100);
        assert_eq!(c.status, CampaignStatus::Paused);
        assert_eq!(c.progress.satisfied_since, None);
        // Paused: a tick records but does not drive.
        let d = c.tick(0.4, 200);
        assert!(!d.is_running());
        // Resume returns to Running.
        c.start(300)?;
        assert_eq!(c.status, CampaignStatus::Running);
        Ok(())
    }

    #[test]
    fn current_directive_reflects_running_state_and_last_gap() -> Result<()> {
        let goal = drift_goal(Comparator::AtLeast, 1.0)?;
        let mut c = Campaign::new("c6", "Steer TECH", "p1", "TECHNOLOGY", goal, 0);

        // Planned, never ticked: idle (drives nothing).
        assert!(!c.current_directive().is_running());

        // Running but not yet ticked (no last_gap): still idle.
        c.start(0)?;
        assert!(!c.current_directive().is_running());

        // After a tick, current_directive mirrors the PERSISTED gap mapping
        // WITHOUT advancing the loop, and equals the directive that tick returned.
        let live = c.tick(0.0, 100); // observed 0.0, AtLeast 1.0 -> gap 1.0, drives
        let current = c.current_directive();
        assert!(current.is_running());
        assert_eq!(current.target_segment, "TECHNOLOGY");
        assert_eq!(
            current, live,
            "current_directive must equal the last tick's directive"
        );
        let gap = match c.progress.last_gap {
            Some(g) => g,
            None => panic!("a tick must record last_gap"),
        };
        assert_eq!(current, directive_for_gap(gap, "TECHNOLOGY"));

        // Paused (and likewise Achieved): idle regardless of the persisted gap.
        c.pause(200);
        assert!(!c.current_directive().is_running());
        Ok(())
    }

    #[test]
    fn campaign_json_round_trips() -> Result<()> {
        let goal = drift_goal(Comparator::AtLeast, 0.75)?;
        let mut c = Campaign::new("c5", "Steer TECH", "p1", "TECHNOLOGY", goal, 1_234);
        c.start(1_234)?;
        c.tick(0.1, 2_000);
        let json = serde_json::to_string(&c)?;
        let back: Campaign = serde_json::from_str(&json)?;
        assert_eq!(back, c);
        Ok(())
    }
}
