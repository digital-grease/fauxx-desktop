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

//! Idle-aware rate planning (C8 #32, U1).
//!
//! The [`RatePlanner`] turns the [`IdleState`] sampled from an [`IdleSource`]
//! into a concrete scheduling decision over a base [`IntensityLevel`]: SCALE
//! UP once an idle threshold is crossed, and PAUSE (or throttle) while the user
//! is active or the session is locked. Both the threshold and the active-state
//! behavior are configurable ([`IdleScalingConfig`]).
//!
//! This sits between the U2 campaign planner / raw intensity setting (which
//! pick the BASE intensity) and the C1 household timeline scheduler (which
//! samples the Poisson stream at the resulting rate). It owns the gating policy
//! only; it does not itself sample timing.

use serde::{Deserialize, Serialize};

use super::{IdleSource, IdleState};
use crate::orchestration::IntensityLevel;

/// How a non-idle (active or locked) session caps decoy activity (C8 #32).
///
/// Configurable per the acceptance criteria: a privacy-first deployment pauses
/// outright, a "keep a trickle going" deployment throttles to the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveBehavior {
    /// Stop decoy browsing entirely while the user is active or locked.
    Pause,
    /// Throttle to the lowest intensity floor instead of stopping. Keeps a thin
    /// background stream so the profile does not go fully silent during long
    /// active stretches.
    Throttle(IntensityLevel),
}

impl Default for ActiveBehavior {
    /// Pause: the conservative, resource-friendly default for the always-on box.
    fn default() -> Self {
        ActiveBehavior::Pause
    }
}

/// Tuning for the idle-aware rate planner (C8 #32). Both fields are configurable
/// per the acceptance criteria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleScalingConfig {
    /// Continuous idle time (in seconds) past which decoy intensity scales up.
    pub idle_threshold_secs: u64,
    /// One step up the [`IntensityLevel`] ladder per this many WHOLE multiples
    /// of the threshold spent idle, clamped at [`IntensityLevel::Extreme`]. At
    /// least the threshold itself yields one step; `0` disables stepping (idle
    /// uses the base intensity unscaled).
    pub steps_per_threshold: u32,
    /// What an active or locked session does to decoy activity.
    pub active_behavior: ActiveBehavior,
}

impl IdleScalingConfig {
    /// The default policy: ramp after five minutes idle, one ladder step per
    /// five-minute multiple, and pause while active/locked.
    pub fn new() -> Self {
        Self {
            idle_threshold_secs: 5 * 60,
            steps_per_threshold: 1,
            active_behavior: ActiveBehavior::default(),
        }
    }

    /// Override the idle threshold (seconds).
    pub fn with_idle_threshold_secs(mut self, secs: u64) -> Self {
        self.idle_threshold_secs = secs;
        self
    }

    /// Override how many ladder steps each threshold-multiple of idle adds.
    pub fn with_steps_per_threshold(mut self, steps: u32) -> Self {
        self.steps_per_threshold = steps;
        self
    }

    /// Override the active/locked behavior.
    pub fn with_active_behavior(mut self, behavior: ActiveBehavior) -> Self {
        self.active_behavior = behavior;
        self
    }
}

impl Default for IdleScalingConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The scheduling decision the planner emits for one tick (C8 #32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "intensity")]
pub enum RateDecision {
    /// No decoy activity this tick (paused).
    Paused,
    /// Run decoy activity at this intensity.
    Run(IntensityLevel),
}

impl RateDecision {
    /// The effective intensity, or `None` when paused. Convenience for callers
    /// feeding the C1 timeline scheduler (which needs a rate or nothing).
    pub fn intensity(&self) -> Option<IntensityLevel> {
        match self {
            RateDecision::Run(level) => Some(*level),
            RateDecision::Paused => None,
        }
    }

    /// Whether this decision runs any decoy activity.
    pub fn is_running(&self) -> bool {
        matches!(self, RateDecision::Run(_))
    }
}

/// The ordered intensity ladder, lowest to highest. Stepping walks this.
const LADDER: [IntensityLevel; 4] = [
    IntensityLevel::Low,
    IntensityLevel::Medium,
    IntensityLevel::High,
    IntensityLevel::Extreme,
];

/// The index of `level` on the ladder.
fn ladder_index(level: IntensityLevel) -> usize {
    LADDER.iter().position(|l| *l == level).unwrap_or(0)
}

/// Step `base` up the ladder by `steps`, clamped at the top.
fn step_up(base: IntensityLevel, steps: u32) -> IntensityLevel {
    let idx = ladder_index(base).saturating_add(steps as usize);
    let clamped = idx.min(LADDER.len() - 1);
    LADDER[clamped]
}

/// The idle-aware rate planner (C8 #32, U1). Holds the detection source and the
/// scaling policy; [`plan`](RatePlanner::plan) maps a sampled state + a base
/// intensity to a [`RateDecision`].
pub struct RatePlanner {
    source: Box<dyn IdleSource>,
    config: IdleScalingConfig,
}

impl std::fmt::Debug for RatePlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RatePlanner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RatePlanner {
    /// Build a planner over an idle source and a scaling config.
    pub fn new(source: Box<dyn IdleSource>, config: IdleScalingConfig) -> Self {
        Self { source, config }
    }

    /// Build a planner with the dep-free [`ConservativeIdleSource`] and the
    /// default policy (the always-on default until real detection is wired).
    pub fn conservative() -> Self {
        Self::new(
            Box::new(super::ConservativeIdleSource),
            IdleScalingConfig::new(),
        )
    }

    /// The scaling config in force.
    pub fn config(&self) -> IdleScalingConfig {
        self.config
    }

    /// Decide this tick's rate over `base`: sample the idle source, then apply
    /// the gating policy.
    pub async fn plan(&self, base: IntensityLevel) -> RateDecision {
        let state = self.source.idle_state().await;
        self.decide(state, base)
    }

    /// Sample the current idle/lock state from the source, WITHOUT applying the
    /// gating policy. For status/visibility surfaces (the #36 HA status sensor).
    pub async fn sample(&self) -> IdleState {
        self.source.idle_state().await
    }

    /// The pure gating decision for an already-sampled `state` and `base`.
    ///
    /// - [`IdleState::Active`] / [`IdleState::Locked`]: apply the configured
    ///   [`ActiveBehavior`] (pause, or throttle to a floor).
    /// - [`IdleState::Idle`] BELOW the threshold: run at `base` unchanged.
    /// - [`IdleState::Idle`] AT/ABOVE the threshold: scale `base` UP one ladder
    ///   step per whole threshold-multiple of idle, clamped at `Extreme`.
    pub fn decide(&self, state: IdleState, base: IntensityLevel) -> RateDecision {
        match state {
            IdleState::Active | IdleState::Locked => match self.config.active_behavior {
                ActiveBehavior::Pause => RateDecision::Paused,
                // Throttle to the floor, but never ABOVE the base (a tiny base
                // should not be raised by an active session).
                ActiveBehavior::Throttle(floor) => {
                    let level = if ladder_index(floor) <= ladder_index(base) {
                        floor
                    } else {
                        base
                    };
                    RateDecision::Run(level)
                }
            },
            IdleState::Idle(idle_for) => {
                let threshold_secs = self.config.idle_threshold_secs;
                if threshold_secs == 0 || idle_for.as_secs() < threshold_secs {
                    // Not yet over the threshold (or stepping disabled): base.
                    return RateDecision::Run(base);
                }
                // Whole multiples of the threshold spent idle, times the
                // per-threshold step count, walk us up the ladder.
                let multiples = (idle_for.as_secs() / threshold_secs) as u32;
                let steps = multiples.saturating_mul(self.config.steps_per_threshold);
                RateDecision::Run(step_up(base, steps))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::idle::StubIdleSource;

    fn planner_with(stub: StubIdleSource, config: IdleScalingConfig) -> RatePlanner {
        RatePlanner::new(Box::new(stub), config)
    }

    #[tokio::test]
    async fn active_pauses_by_default() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::Active);
        let planner = planner_with(stub, IdleScalingConfig::new());
        assert_eq!(
            planner.plan(IntensityLevel::Medium).await,
            RateDecision::Paused
        );
        Ok(())
    }

    #[tokio::test]
    async fn locked_pauses_by_default() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::Locked);
        let planner = planner_with(stub, IdleScalingConfig::new());
        assert_eq!(
            planner.plan(IntensityLevel::High).await,
            RateDecision::Paused
        );
        Ok(())
    }

    #[tokio::test]
    async fn active_throttles_to_floor_when_configured() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::Active);
        let config = IdleScalingConfig::new()
            .with_active_behavior(ActiveBehavior::Throttle(IntensityLevel::Low));
        let planner = planner_with(stub, config);
        // Throttled to the floor, not paused.
        assert_eq!(
            planner.plan(IntensityLevel::High).await,
            RateDecision::Run(IntensityLevel::Low)
        );
        Ok(())
    }

    #[tokio::test]
    async fn idle_below_threshold_runs_base() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::idle_secs(30));
        let config = IdleScalingConfig::new().with_idle_threshold_secs(60);
        let planner = planner_with(stub, config);
        // Idle, but not yet past the threshold: unchanged base.
        assert_eq!(
            planner.plan(IntensityLevel::Low).await,
            RateDecision::Run(IntensityLevel::Low)
        );
        Ok(())
    }

    #[tokio::test]
    async fn idle_past_threshold_scales_up_one_step() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::idle_secs(90));
        let config = IdleScalingConfig::new()
            .with_idle_threshold_secs(60)
            .with_steps_per_threshold(1);
        let planner = planner_with(stub, config);
        // 90s / 60s = 1 whole multiple -> one ladder step up from Low.
        assert_eq!(
            planner.plan(IntensityLevel::Low).await,
            RateDecision::Run(IntensityLevel::Medium)
        );
        Ok(())
    }

    #[tokio::test]
    async fn deep_idle_scales_up_and_clamps_at_extreme() -> Result<()> {
        let stub = StubIdleSource::new(IdleState::idle_secs(60 * 60));
        let config = IdleScalingConfig::new()
            .with_idle_threshold_secs(60)
            .with_steps_per_threshold(1);
        let planner = planner_with(stub, config);
        // Many multiples -> clamped at the top of the ladder.
        assert_eq!(
            planner.plan(IntensityLevel::Medium).await,
            RateDecision::Run(IntensityLevel::Extreme)
        );
        Ok(())
    }

    #[tokio::test]
    async fn state_transition_active_to_idle_to_locked() -> Result<()> {
        // One planner, the stub flipped across all three states (the U1 AC).
        let stub = StubIdleSource::new(IdleState::Active);
        let config = IdleScalingConfig::new()
            .with_idle_threshold_secs(60)
            .with_steps_per_threshold(1);
        let planner = planner_with(stub.clone(), config);

        // Active: paused.
        assert_eq!(
            planner.plan(IntensityLevel::Medium).await,
            RateDecision::Paused
        );
        // Idle past threshold: scaled up.
        stub.set(IdleState::idle_secs(120));
        assert_eq!(
            planner.plan(IntensityLevel::Medium).await,
            RateDecision::Run(IntensityLevel::Extreme)
        );
        // Locked: paused again.
        stub.set(IdleState::Locked);
        assert_eq!(
            planner.plan(IntensityLevel::Medium).await,
            RateDecision::Paused
        );
        Ok(())
    }

    #[test]
    fn decision_intensity_accessor() {
        assert_eq!(RateDecision::Paused.intensity(), None);
        assert!(!RateDecision::Paused.is_running());
        assert_eq!(
            RateDecision::Run(IntensityLevel::High).intensity(),
            Some(IntensityLevel::High)
        );
        assert!(RateDecision::Run(IntensityLevel::High).is_running());
    }
}
