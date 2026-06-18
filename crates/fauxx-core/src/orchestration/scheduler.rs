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

//! Household timeline scheduler (C1 #10, O4).
//!
//! Each paired device already schedules its own local browsing. This module
//! coordinates the household ACROSS devices WITHOUT replacing that local
//! timing: it aggregates each device's intended action windows and treats the
//! whole household as ONE Poisson-like stream so the combined cadence stays
//! human, rather than N independent streams summing to an unnatural rate.
//!
//! ## Timing model (mirrors the Android `PoissonScheduler`)
//!
//! - [`IntensityLevel`] sets actions/hour: LOW=12, MEDIUM=60, HIGH=200,
//!   EXTREME=500.
//! - Activity is circadian: the active window is 07:00-23:00 local; 23:00-07:00
//!   is quiet (no actions emitted).
//! - Inter-arrival delay is Poisson: `delay = -ln(1 - u) / rate` for a uniform
//!   `u` in `[0, 1)` (the same formula the phone's `PoissonScheduler` uses).
//!   `rate` is in actions per second.
//!
//! ## Aggregation and modes
//!
//! - The household rate is the SUM of per-device rates, then the single
//!   aggregate stream is sampled from that combined rate (one Poisson process of
//!   rate `R = sum r_i` is statistically equivalent to the superposition of
//!   independent processes of rates `r_i`, so the household looks like one
//!   person, not N). Each emitted action is attributed to a device in
//!   proportion to its rate.
//! - [`CoherentHousehold`](super::CoordinationMode::CoherentHousehold): one
//!   person using several devices; plausible overlap is allowed, but the
//!   anti-collision rule still prevents two devices firing the *same* query in
//!   the same instant (a person does not use two devices in the same second
//!   either).
//! - [`Fragmentation`](super::CoordinationMode::Fragmentation): personas must
//!   look unrelated, so timing is forced to be non-correlated: overlapping
//!   intents are offset by at least the collision window so no two devices ever
//!   emit within it.
//!
//! ## Degradation
//!
//! Planning takes whichever devices are currently present; an offline peer
//! simply is not in the input, so the household degrades to local-only
//! scheduling for the devices that remain, with no stall.
//!
//! ## Determinism
//!
//! All randomness flows through a seedable [`StdRng`]; a fixed seed yields a
//! fully deterministic plan, which is how the hermetic tests verify the
//! Poisson-like, circadian, collision-free output.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

use super::CoordinationMode;
use crate::error::{CoreError, Result};

/// Seconds in a 24-hour day.
const SECONDS_PER_DAY: i64 = 86_400;
/// Start of the active window (07:00), in seconds past local midnight.
pub const ACTIVE_WINDOW_START_SECS: i64 = 7 * 3_600;
/// End of the active window (23:00), in seconds past local midnight.
pub const ACTIVE_WINDOW_END_SECS: i64 = 23 * 3_600;
/// In coherent mode the household allows plausible overlap; only a literal
/// same-second firing collides. This is that minimum gap (one second).
const COHERENT_MIN_GAP_SECS: i64 = 1;

/// Activity intensity, in actions per hour. Mirrors the Android intensity
/// ladder exactly so cross-device cadence agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum IntensityLevel {
    /// 12 actions/hour.
    Low,
    /// 60 actions/hour.
    Medium,
    /// 200 actions/hour.
    High,
    /// 500 actions/hour.
    Extreme,
}

impl IntensityLevel {
    /// Actions per hour for this level (the frozen Android figures).
    pub fn actions_per_hour(&self) -> u32 {
        match self {
            IntensityLevel::Low => 12,
            IntensityLevel::Medium => 60,
            IntensityLevel::High => 200,
            IntensityLevel::Extreme => 500,
        }
    }

    /// The Poisson rate in actions per SECOND.
    pub fn rate_per_second(&self) -> f64 {
        f64::from(self.actions_per_hour()) / 3_600.0
    }
}

/// One paired device's intended contribution to the household stream.
///
/// This is the input contract O4 consumes: a device declares its identity (its
/// base64url public key, or empty for the local device), the persona it is
/// driving, and its local intensity. The scheduler aggregates these. An offline
/// peer is simply absent from the list (graceful degradation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIntent {
    /// The device's base64url public key, or empty for the local device.
    pub device_key: String,
    /// The persona id this device is currently driving.
    pub persona_id: String,
    /// The device's local intensity level.
    pub intensity: IntensityLevel,
}

impl DeviceIntent {
    /// Build a device intent.
    pub fn new(
        device_key: impl Into<String>,
        persona_id: impl Into<String>,
        intensity: IntensityLevel,
    ) -> Self {
        Self {
            device_key: device_key.into(),
            persona_id: persona_id.into(),
            intensity,
        }
    }
}

/// One scheduled household action: when it fires (seconds past local midnight)
/// and which device/persona emits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledAction {
    /// Offset from local midnight, in seconds (always inside the active window).
    pub at_secs: i64,
    /// The device that emits this action (base64url public key, or empty for
    /// the local device).
    pub device_key: String,
    /// The persona id driving this action.
    pub persona_id: String,
}

/// Tuning for a household plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanConfig {
    /// Minimum gap, in seconds, the anti-collision rule enforces between any two
    /// actions (Coherent) or between any two actions of distinct devices
    /// (Fragmentation, which additionally forbids near-correlation).
    pub collision_window_secs: i64,
    /// The active mode, which selects the anti-collision strictness.
    pub mode: CoordinationMode,
}

impl PlanConfig {
    /// A plan config for `mode` with the default 2-second collision window
    /// (two devices firing within 2 seconds reads as one coordinated burst).
    pub fn new(mode: CoordinationMode) -> Self {
        Self {
            collision_window_secs: 2,
            mode,
        }
    }

    /// Override the collision window.
    pub fn with_collision_window_secs(mut self, secs: i64) -> Self {
        self.collision_window_secs = secs.max(0);
        self
    }
}

/// Whether a given second-of-day falls inside the active (07:00-23:00) window.
pub fn is_active_window(secs_into_day: i64) -> bool {
    let s = secs_into_day.rem_euclid(SECONDS_PER_DAY);
    (ACTIVE_WINDOW_START_SECS..ACTIVE_WINDOW_END_SECS).contains(&s)
}

/// Plan the household's action timeline over one active day from the given
/// device intents, as a single Poisson-like, circadian, collision-free stream.
///
/// `seed` makes the plan fully deterministic. An empty `intents` (every peer
/// offline) yields an empty plan rather than stalling. The returned actions are
/// sorted by `at_secs` ascending and all fall inside the active window.
pub fn plan_household_day(
    intents: &[DeviceIntent],
    config: PlanConfig,
    seed: u64,
) -> Result<Vec<ScheduledAction>> {
    // Graceful degradation: no present devices => no actions, no stall.
    if intents.is_empty() {
        return Ok(Vec::new());
    }

    // The household is ONE Poisson process whose rate is the sum of the
    // per-device rates. Sampling from the combined rate (rather than N separate
    // streams) is what keeps the aggregate human: it is the superposition of
    // the independent device processes, with the same statistics as one person
    // browsing across several devices.
    let rates: Vec<f64> = intents
        .iter()
        .map(|d| d.intensity.rate_per_second())
        .collect();
    let total_rate: f64 = rates.iter().sum();
    if total_rate <= 0.0 {
        return Err(CoreError::Orchestration(
            "household rate is zero; no device declared a positive intensity".to_string(),
        ));
    }

    let mut rng = StdRng::seed_from_u64(seed);
    let mut actions: Vec<ScheduledAction> = Vec::new();

    // Walk the active window sampling Poisson inter-arrival delays. We start at
    // the window open and advance by `-ln(1 - u) / total_rate` each step.
    let mut t = ACTIVE_WINDOW_START_SECS as f64;
    while t < ACTIVE_WINDOW_END_SECS as f64 {
        // Uniform u in [0, 1); StandardUniform for f64 is [0, 1). The Poisson
        // inter-arrival delay is the phone's PoissonScheduler formula verbatim
        // (see `next_arrival`), which also refuses a draw that cannot advance `t`
        // and would otherwise spin this window-fill loop forever.
        let u: f64 = rng.random::<f64>();
        t = match next_arrival(t, u, total_rate) {
            Some(next) => next,
            None => break,
        };
        if t >= ACTIVE_WINDOW_END_SECS as f64 {
            break;
        }

        // Attribute this arrival to a device in proportion to its rate (the
        // standard thinning of a superposed Poisson process).
        let pick: f64 = rng.random::<f64>() * total_rate;
        let mut acc = 0.0;
        let mut idx = rates.len() - 1;
        for (i, r) in rates.iter().enumerate() {
            acc += r;
            if pick < acc {
                idx = i;
                break;
            }
        }
        let device = &intents[idx];

        actions.push(ScheduledAction {
            at_secs: t.floor() as i64,
            device_key: device.device_key.clone(),
            persona_id: device.persona_id.clone(),
        });
    }

    apply_anti_collision(&mut actions, config);
    Ok(actions)
}

/// Compute the next Poisson arrival time from `t` given a uniform draw `u` in
/// `[0, 1)` and the household `total_rate` (events/second). The inter-arrival
/// delay is `-ln(1 - u) / total_rate`, the phone's PoissonScheduler formula.
///
/// Returns `None` when the delay does not advance `t`. `StandardUniform` can
/// yield `u == 0` (delay `0`), and a near-zero `u` yields a delay below the ULP
/// at a large `t`, so `t + delay == t`. Either way the window-fill loop in
/// [`plan_household_day`] would never reach `ACTIVE_WINDOW_END_SECS` and would
/// spin forever; the caller treats `None` as "window filled to the resolution
/// f64 affords here" and stops.
fn next_arrival(t: f64, u: f64, total_rate: f64) -> Option<f64> {
    let delay = -(1.0 - u).ln() / total_rate;
    let next = t + delay;
    if next <= t {
        None
    } else {
        Some(next)
    }
}

/// Enforce the anti-collision rule, offsetting overlapping intents so the
/// stream never reads as two devices firing the same instant.
///
/// Coherent: plausible overlap is allowed, so only a literal same-second firing
/// collides; it is nudged to the next second ([`COHERENT_MIN_GAP_SECS`]).
/// Fragmentation: no two actions of DISTINCT devices may fall within the
/// collision window, so per-persona timing stays non-correlated; a collision is
/// pushed a full window ahead. In both modes, an offset can push the tail of
/// the day forward, so any action that lands at or past the active-window close
/// (23:00) is dropped rather than allowed to spill into the quiet window.
fn apply_anti_collision(actions: &mut Vec<ScheduledAction>, config: PlanConfig) {
    let window = config.collision_window_secs;
    if window <= 0 || actions.len() < 2 {
        return;
    }
    // Process in time order, sweeping a monotonic frontier forward. Each action
    // is first clamped so it never lands before the previous one (keeps the
    // sequence sorted and emission order intact), then pushed ahead when it
    // would collide. The two modes differ in WHICH proximity counts as a
    // collision and in how far the offset pushes.
    actions.sort_by_key(|a| a.at_secs);
    for i in 1..actions.len() {
        let prev = actions[i - 1].at_secs;
        let prev_key = actions[i - 1].device_key.clone();
        // Monotonic: an action never precedes the one before it, even when it
        // is same-device and not separated (its own local cadence is kept, but
        // it cannot reorder behind an earlier, already-placed action of another
        // device, which is what reintroduced cross-device collisions before).
        if actions[i].at_secs < prev {
            actions[i].at_secs = prev;
        }
        let (collides, min_gap) = match config.mode {
            // Coherent: one person, several devices. Plausible overlap is fine;
            // only two actions in the SAME second collide (a person does not use
            // two devices in the same instant). Nudge to the next second.
            CoordinationMode::CoherentHousehold => (
                actions[i].at_secs - prev < COHERENT_MIN_GAP_SECS,
                COHERENT_MIN_GAP_SECS,
            ),
            // Fragmentation: only distinct-device proximity is a correlation
            // leak. Same-device back-to-back is the device's own local cadence
            // and is left alone. A leak is pushed a full window ahead.
            CoordinationMode::Fragmentation => (
                actions[i].device_key != prev_key && actions[i].at_secs - prev < window,
                window,
            ),
        };
        if collides {
            actions[i].at_secs = prev + min_gap;
        }
    }
    // Offsetting can march the tail past 23:00; drop anything that would land in
    // the quiet window rather than letting the frontier run past it.
    actions.retain(|a| a.at_secs < ACTIVE_WINDOW_END_SECS);
}

/// Whether any two actions of DISTINCT devices fall within `window` seconds of
/// each other. The collision-freedom invariant the tests assert against.
pub fn has_cross_device_collision(actions: &[ScheduledAction], window: i64) -> bool {
    if window <= 0 {
        return false;
    }
    let mut sorted: Vec<&ScheduledAction> = actions.iter().collect();
    sorted.sort_by_key(|a| a.at_secs);
    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.device_key != b.device_key && (b.at_secs - a.at_secs) < window {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intents() -> Vec<DeviceIntent> {
        vec![
            DeviceIntent::new("", "persona-desktop", IntensityLevel::Medium),
            DeviceIntent::new("phone-key", "persona-phone", IntensityLevel::Low),
        ]
    }

    #[test]
    fn next_arrival_refuses_non_progress_draws() {
        // Regression: a uniform draw of exactly 0.0 (StandardUniform is [0, 1))
        // gives a zero delay, and a near-zero draw gives a sub-ULP delay that
        // f64 cannot represent at the window start; both must return None so the
        // window-fill loop terminates instead of spinning forever.
        let rate = 1e-4; // a realistic low household rate (events/second)
        let t = ACTIVE_WINDOW_START_SECS as f64;
        assert_eq!(next_arrival(t, 0.0, rate), None);
        assert_eq!(next_arrival(t, 1e-300, rate), None);
        // A normal draw advances t strictly.
        assert!(matches!(next_arrival(t, 0.5, rate), Some(next) if next > t));
    }

    #[test]
    fn intensity_ladder_is_frozen() {
        assert_eq!(IntensityLevel::Low.actions_per_hour(), 12);
        assert_eq!(IntensityLevel::Medium.actions_per_hour(), 60);
        assert_eq!(IntensityLevel::High.actions_per_hour(), 200);
        assert_eq!(IntensityLevel::Extreme.actions_per_hour(), 500);
    }

    #[test]
    fn active_window_is_circadian() {
        assert!(!is_active_window(6 * 3_600)); // 06:00 quiet
        assert!(is_active_window(7 * 3_600)); // 07:00 active
        assert!(is_active_window(22 * 3_600 + 3_599)); // 22:59 active
        assert!(!is_active_window(23 * 3_600)); // 23:00 quiet
        assert!(!is_active_window(2 * 3_600)); // 02:00 quiet
    }

    #[test]
    fn empty_intents_degrade_to_no_actions() -> Result<()> {
        let plan =
            plan_household_day(&[], PlanConfig::new(CoordinationMode::CoherentHousehold), 1)?;
        assert!(plan.is_empty());
        Ok(())
    }

    #[test]
    fn offline_peer_degrades_to_local_only() -> Result<()> {
        // Only the local device is present (the phone went offline).
        let local_only = vec![DeviceIntent::new(
            "",
            "persona-desktop",
            IntensityLevel::Medium,
        )];
        let plan = plan_household_day(
            &local_only,
            PlanConfig::new(CoordinationMode::CoherentHousehold),
            7,
        )?;
        assert!(!plan.is_empty());
        assert!(plan.iter().all(|a| a.device_key.is_empty()));
        Ok(())
    }

    #[test]
    fn plan_is_deterministic_for_a_fixed_seed() -> Result<()> {
        let cfg = PlanConfig::new(CoordinationMode::CoherentHousehold);
        let a = plan_household_day(&intents(), cfg, 42)?;
        let b = plan_household_day(&intents(), cfg, 42)?;
        assert_eq!(a, b);
        // A different seed gives a different plan (overwhelmingly likely).
        let c = plan_household_day(&intents(), cfg, 43)?;
        assert_ne!(a, c);
        Ok(())
    }

    #[test]
    fn all_actions_fall_in_the_active_window() -> Result<()> {
        let cfg = PlanConfig::new(CoordinationMode::CoherentHousehold);
        let plan = plan_household_day(&intents(), cfg, 99)?;
        assert!(!plan.is_empty());
        for action in &plan {
            assert!(
                is_active_window(action.at_secs),
                "action at {} is outside the active window",
                action.at_secs
            );
        }
        Ok(())
    }

    #[test]
    fn actions_are_sorted_ascending() -> Result<()> {
        let cfg = PlanConfig::new(CoordinationMode::CoherentHousehold);
        let plan = plan_household_day(&intents(), cfg, 5)?;
        for w in plan.windows(2) {
            assert!(w[0].at_secs <= w[1].at_secs);
        }
        Ok(())
    }

    #[test]
    fn fragmentation_has_no_cross_device_collision() -> Result<()> {
        let cfg = PlanConfig::new(CoordinationMode::Fragmentation).with_collision_window_secs(3);
        // Push both devices high so collisions are likely without the rule.
        let busy = vec![
            DeviceIntent::new("", "p-desktop", IntensityLevel::Extreme),
            DeviceIntent::new("phone", "p-phone", IntensityLevel::Extreme),
        ];
        let plan = plan_household_day(&busy, cfg, 11)?;
        assert!(!plan.is_empty());
        assert!(
            !has_cross_device_collision(&plan, cfg.collision_window_secs),
            "fragmentation plan must not place two devices within the collision window"
        );
        Ok(())
    }

    #[test]
    fn coherent_allows_overlap_but_never_same_second() -> Result<()> {
        let cfg =
            PlanConfig::new(CoordinationMode::CoherentHousehold).with_collision_window_secs(2);
        let busy = vec![
            DeviceIntent::new("", "p-desktop", IntensityLevel::High),
            DeviceIntent::new("phone", "p-phone", IntensityLevel::High),
        ];
        let plan = plan_household_day(&busy, cfg, 13)?;
        // Coherent allows plausible overlap (gaps may be smaller than the
        // fragmentation window), but never two household actions in the same
        // second.
        for w in plan.windows(2) {
            assert!(
                w[1].at_secs - w[0].at_secs >= COHERENT_MIN_GAP_SECS,
                "coherent must not place two actions in the same second"
            );
        }
        // And specifically no two DISTINCT devices fire in the same second.
        assert!(!has_cross_device_collision(&plan, COHERENT_MIN_GAP_SECS));
        Ok(())
    }

    #[test]
    fn anti_collision_never_spills_into_the_quiet_window() -> Result<()> {
        // EXTREME on two distinct devices with a wide collision window maximally
        // stresses the forward offset; it must drop the overflow tail rather
        // than march actions past 23:00 into the quiet window.
        let cfg = PlanConfig::new(CoordinationMode::Fragmentation).with_collision_window_secs(10);
        let busy = vec![
            DeviceIntent::new("", "p-desktop", IntensityLevel::Extreme),
            DeviceIntent::new("phone", "p-phone", IntensityLevel::Extreme),
        ];
        let plan = plan_household_day(&busy, cfg, 7)?;
        assert!(
            plan.iter().all(
                |a| a.at_secs >= ACTIVE_WINDOW_START_SECS && a.at_secs < ACTIVE_WINDOW_END_SECS
            ),
            "every scheduled action must stay inside the 07:00-23:00 active window"
        );
        Ok(())
    }

    #[test]
    fn aggregate_rate_stays_human_not_n_independent_streams() -> Result<()> {
        // The combined count should track the SUM of the per-device hourly
        // rates over the 16-hour active window, within Poisson noise: it must
        // not balloon (which is what N naive independent streams summing
        // wrongly would do) nor collapse.
        let cfg = PlanConfig::new(CoordinationMode::Fragmentation);
        // Medium (60/h) + Low (12/h) = 72/h over 16 active hours = 1152 expected.
        let plan = plan_household_day(&intents(), cfg, 2024)?;
        let n = plan.len() as f64;
        let expected = 72.0 * 16.0;
        // Generous Poisson band (within ~25%); deterministic seed keeps it stable.
        assert!(
            (expected * 0.75..expected * 1.25).contains(&n),
            "household produced {n} actions, expected near {expected}"
        );
        // Both devices appear in the attribution.
        assert!(plan.iter().any(|a| a.device_key.is_empty()));
        assert!(plan.iter().any(|a| a.device_key == "phone-key"));
        Ok(())
    }
}
