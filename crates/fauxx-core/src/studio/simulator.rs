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

//! Deterministic, seedable synthetic-WEEK simulator (C5 #26, P3).
//!
//! [`simulate_week`] previews a persona's week of decoy activity WITHOUT any real
//! browsing or network: it produces a structured [`SimulatedWeek`] timeline of
//! decoy sessions and queries (with times, a category, and an intensity), so the
//! GUI can render the preview later (no rendering here).
//!
//! The preview matches EXECUTION because it reuses the same two frozen models the
//! C1 household scheduler and the browser use:
//!
//! - Category selection uses the persona-following weight BLEND from
//!   [`crate::constants`] (the Android `computeWeights`): each category's weight
//!   is `PERSONA_FOLLOW_FRACTION (0.85) * w + 0.15 * UNIFORM_BASELINE_WEIGHT
//!   (0.6)`, where `w` is [`ALIGNED_WEIGHT`] (2.0) for an interest else
//!   [`MISALIGNED_WEIGHT`] (0.3), giving an interest 1.79 and a non-interest
//!   0.345; one weighted draw over all categories picks each query, so interests
//!   are favored while noise still blurs the fingerprint. With no interests it
//!   degrades to a uniform draw at [`NEUTRAL_WEIGHT`] (1.0). See
//!   [`select_category`].
//! - Timing uses the same circadian Poisson model as the household scheduler:
//!   activity only inside the 07:00-23:00 active window, with Poisson
//!   inter-arrival delays `-ln(1 - u) / rate`.
//!
//! Determinism: all randomness flows through a seedable [`StdRng`] (mixed with a
//! stable hash of the persona id), so the SAME `(persona, seed)` yields an
//! identical week, and a NEW seed re-rolls it.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::constants::{
    ALIGNED_WEIGHT, MISALIGNED_WEIGHT, NEUTRAL_WEIGHT, PERSONA_FOLLOW_FRACTION,
    UNIFORM_BASELINE_WEIGHT,
};
use crate::orchestration::scheduler::{
    IntensityLevel, ACTIVE_WINDOW_END_SECS, ACTIVE_WINDOW_START_SECS,
};
use crate::persona::{CategoryPool, SyntheticPersona};

/// Days simulated in one preview week.
pub const DAYS_PER_WEEK: u32 = 7;

/// The weight assigned to a category for one query, recorded so the preview is
/// explainable (aligned vs misaligned vs neutral vs uniform-baseline noise).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum QueryWeighting {
    /// Drawn from the persona-following component, category aligned with an
    /// interest ([`ALIGNED_WEIGHT`]).
    PersonaAligned,
    /// Drawn from the persona-following component, a neutral (non-interest)
    /// category ([`NEUTRAL_WEIGHT`]).
    PersonaNeutral,
    /// Drawn from the uniform-baseline noise component (the 15% that blurs the
    /// fingerprint, weighted by [`UNIFORM_BASELINE_WEIGHT`]).
    UniformBaseline,
}

/// One simulated decoy query inside a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedQuery {
    /// Offset from local midnight, in seconds (always inside the active window).
    pub at_secs: i64,
    /// The selected interest category (a [`CategoryPool`] enum name).
    pub category: String,
    /// How this category was weighted into the selection.
    pub weighting: QueryWeighting,
}

/// One simulated decoy session: a contiguous run of queries on one day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedSession {
    /// Zero-based day index within the week (0..[`DAYS_PER_WEEK`]).
    pub day: u32,
    /// Offset from local midnight, in seconds, of the session's first query.
    pub start_secs: i64,
    /// The queries in this session, in time order.
    pub queries: Vec<SimulatedQuery>,
}

impl SimulatedSession {
    /// The number of queries in this session.
    pub fn query_count(&self) -> usize {
        self.queries.len()
    }
}

/// A full simulated week of decoy activity for a persona preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedWeek {
    /// The persona id this week was simulated for.
    pub persona_id: String,
    /// The seed that produced this week (re-rollable with a new seed).
    pub seed: u64,
    /// The per-day intensity used (preview parity with execution).
    pub intensity: IntensityLevel,
    /// The simulated sessions, one per active day, day-ascending.
    pub sessions: Vec<SimulatedSession>,
}

impl SimulatedWeek {
    /// Every query across every session, flattened (day-then-time order).
    pub fn all_queries(&self) -> impl Iterator<Item = &SimulatedQuery> {
        self.sessions.iter().flat_map(|s| s.queries.iter())
    }

    /// Total query count across the week.
    pub fn total_queries(&self) -> usize {
        self.sessions
            .iter()
            .map(SimulatedSession::query_count)
            .sum()
    }

    /// How many queries selected `category`.
    pub fn category_count(&self, category: &str) -> usize {
        self.all_queries()
            .filter(|q| q.category == category)
            .count()
    }

    /// The per-category query counts across the whole week, HIGHEST count first
    /// (ties broken by category name for determinism). The #26 category breakdown
    /// the studio preview renders.
    pub fn category_counts(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for query in self.all_queries() {
            *counts.entry(query.category.clone()).or_insert(0) += 1;
        }
        let mut out: Vec<(String, usize)> = counts.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

/// A small, stable (build-independent) hash of a string into a `u64`, used to mix
/// the persona id into the RNG seed so two distinct personas under the same seed
/// get distinct (but each reproducible) weeks. FNV-1a, matching
/// [`crate::browser`]'s cadence seeding.
fn stable_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Simulate one synthetic week of decoy activity for `persona` at `intensity`,
/// seeded by `seed`. Deterministic: the same `(persona, intensity, seed)` yields
/// an identical [`SimulatedWeek`]; a new `seed` re-rolls it. Performs NO real
/// browsing or network access.
pub fn simulate_week(
    persona: &SyntheticPersona,
    intensity: IntensityLevel,
    seed: u64,
) -> SimulatedWeek {
    // Mix the persona id into the seed so distinct personas diverge under one
    // seed, while a fixed (persona, seed) stays reproducible.
    let mut rng = StdRng::seed_from_u64(seed ^ stable_hash(&persona.id));

    // The aligned category set: the persona's known interests.
    let aligned: Vec<CategoryPool> = persona
        .interests
        .iter()
        .filter_map(|i| CategoryPool::from_name(i))
        .collect();

    let rate = intensity.rate_per_second();
    let mut sessions = Vec::with_capacity(DAYS_PER_WEEK as usize);

    for day in 0..DAYS_PER_WEEK {
        let mut queries = Vec::new();
        // Walk the active window sampling Poisson inter-arrival delays, exactly
        // as the household scheduler does, so preview timing matches execution.
        let mut t = ACTIVE_WINDOW_START_SECS as f64;
        loop {
            let u: f64 = rng.random::<f64>();
            // rate > 0 always (every IntensityLevel has a positive rate), so the
            // delay is finite.
            let delay = -(1.0 - u).ln() / rate;
            t += delay;
            if t >= ACTIVE_WINDOW_END_SECS as f64 {
                break;
            }
            let (category, weighting) = select_category(&mut rng, &aligned);
            queries.push(SimulatedQuery {
                at_secs: t.floor() as i64,
                category,
                weighting,
            });
        }
        let start_secs = queries
            .first()
            .map(|q| q.at_secs)
            .unwrap_or(ACTIVE_WINDOW_START_SECS);
        sessions.push(SimulatedSession {
            day,
            start_secs,
            queries,
        });
    }

    SimulatedWeek {
        persona_id: persona.id.clone(),
        seed,
        intensity,
        sessions,
    }
}

/// Select one query's category using the frozen persona-following weight blend,
/// the same one the execution path applies (the Android `computeWeights`):
/// every category gets a single blended weight and ONE weighted draw picks the
/// category, so the preview's category mix matches what execution produces (no
/// separate follow/noise branch that would skew the mix).
///
/// Per category the weight is `follow * w + (1 - follow) * baseline`, where
/// `follow` = [`PERSONA_FOLLOW_FRACTION`] (0.85), `baseline` =
/// [`UNIFORM_BASELINE_WEIGHT`] (0.6), and `w` is [`ALIGNED_WEIGHT`] (2.0) for a
/// persona INTEREST or [`MISALIGNED_WEIGHT`] (0.3) for any other category. So an
/// interest carries `2.0*0.85 + 0.6*0.15 = 1.79` and a non-interest
/// `0.3*0.85 + 0.6*0.15 = 0.345`: each interest is about 5x more likely than
/// each non-interest, while noise still spreads across the whole pool. A persona
/// with NO interests degrades to a uniform draw at [`NEUTRAL_WEIGHT`] (1.0).
///
/// Returns the category name and how it was weighted (PersonaAligned when the
/// chosen category is one of the persona's interests, else UniformBaseline).
fn select_category(rng: &mut StdRng, aligned: &[CategoryPool]) -> (String, QueryWeighting) {
    let all = CategoryPool::all();
    let baseline = UNIFORM_BASELINE_WEIGHT * (1.0 - PERSONA_FOLLOW_FRACTION);
    let aligned_weight = ALIGNED_WEIGHT * PERSONA_FOLLOW_FRACTION + baseline;
    let misaligned_weight = MISALIGNED_WEIGHT * PERSONA_FOLLOW_FRACTION + baseline;

    // Blended weight per category (NEUTRAL uniform when no interests declared).
    let weight_for = |c: &CategoryPool| -> f64 {
        if aligned.is_empty() {
            NEUTRAL_WEIGHT
        } else if aligned.contains(c) {
            aligned_weight
        } else {
            misaligned_weight
        }
    };
    let label_for = |c: &CategoryPool| -> QueryWeighting {
        if aligned.contains(c) {
            QueryWeighting::PersonaAligned
        } else {
            QueryWeighting::UniformBaseline
        }
    };

    let total: f64 = all.iter().map(weight_for).sum();
    let mut pick = rng.random::<f64>() * total;
    for c in all {
        pick -= weight_for(c);
        if pick < 0.0 {
            return (c.as_name().to_string(), label_for(c));
        }
    }
    // Floating-point fallthrough: attribute to the last category.
    let last = &all[all.len() - 1];
    (last.as_name().to_string(), label_for(last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::scheduler::is_active_window;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn persona(interests: &[CategoryPool]) -> SyntheticPersona {
        SyntheticPersona::new(
            "sim-test-0000-4000-8000-000000000001".to_string(),
            "Sim Test".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    fn sample() -> SyntheticPersona {
        persona(&[
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::GAMING,
        ])
    }

    #[test]
    fn same_seed_yields_identical_week() {
        let p = sample();
        let a = simulate_week(&p, IntensityLevel::Medium, 42);
        let b = simulate_week(&p, IntensityLevel::Medium, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn new_seed_re_rolls_the_week() {
        let p = sample();
        let a = simulate_week(&p, IntensityLevel::Medium, 42);
        let c = simulate_week(&p, IntensityLevel::Medium, 43);
        assert_ne!(a, c);
    }

    #[test]
    fn category_counts_aggregate_the_week_highest_first() {
        let week = simulate_week(&sample(), IntensityLevel::Medium, 42);
        let counts = week.category_counts();
        // The breakdown covers every query and agrees with the per-category and
        // total counts.
        let summed: usize = counts.iter().map(|(_, n)| n).sum();
        assert_eq!(summed, week.total_queries());
        for (category, n) in &counts {
            assert_eq!(*n, week.category_count(category));
            assert!(*n > 0, "a listed category must have at least one query");
        }
        // Sorted highest-first (the breakdown leads with the dominant category).
        for pair in counts.windows(2) {
            assert!(pair[0].1 >= pair[1].1, "counts must be descending");
        }
    }

    #[test]
    fn week_has_seven_days() {
        let week = simulate_week(&sample(), IntensityLevel::Medium, 7);
        assert_eq!(week.sessions.len(), DAYS_PER_WEEK as usize);
        for (i, s) in week.sessions.iter().enumerate() {
            assert_eq!(s.day, i as u32);
        }
    }

    #[test]
    fn all_queries_stay_inside_the_active_window() {
        let week = simulate_week(&sample(), IntensityLevel::High, 99);
        assert!(week.total_queries() > 0);
        for q in week.all_queries() {
            assert!(
                is_active_window(q.at_secs),
                "query at {} is outside the active window",
                q.at_secs
            );
            assert!(q.at_secs >= ACTIVE_WINDOW_START_SECS);
            assert!(q.at_secs < ACTIVE_WINDOW_END_SECS);
        }
    }

    #[test]
    fn category_frequencies_lean_toward_persona_interests() {
        // The genuine Android weight blend gives each interest 1.79 and each
        // non-interest 0.345, so the 3 interests take far more than the 3/32 ~=
        // 0.094 a uniform draw would give, and each interest is about 5x more
        // likely than each non-interest.
        let interests = [
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::GAMING,
        ];
        let p = persona(&interests);
        let week = simulate_week(&p, IntensityLevel::High, 2024);
        let total = week.total_queries();
        assert!(total > 100, "need a meaningful sample, got {total}");

        let interest_hits: usize = interests
            .iter()
            .map(|c| week.category_count(c.as_name()))
            .sum();
        let interest_share = interest_hits as f64 / total as f64;
        // 3 interests of 32 categories take ~35% of the mass (3*1.79 / (3*1.79 +
        // 29*0.345)), well above the uniform 0.094.
        assert!(
            interest_share > 0.25,
            "interest share {interest_share:.3} should lean well above the uniform 0.094"
        );

        // Per category, each interest is far more frequent than each non-interest
        // (the weight ratio is 1.79 / 0.345 ~= 5.2).
        let avg_interest = interest_hits as f64 / interests.len() as f64;
        let non_interest_cats = CategoryPool::all().len() - interests.len();
        let avg_non_interest = (total - interest_hits) as f64 / non_interest_cats as f64;
        assert!(
            avg_interest > 2.5 * avg_non_interest,
            "each interest ({avg_interest:.1}) should be far more frequent than each non-interest ({avg_non_interest:.1})"
        );

        // And uniform-baseline (non-interest) queries still exist, proving noise
        // spreads across the wider pool.
        assert!(week
            .all_queries()
            .any(|q| q.weighting == QueryWeighting::UniformBaseline));
    }

    #[test]
    fn interest_less_persona_degrades_to_uniform_noise() {
        // A persona whose only interest is unknown maps to zero aligned
        // categories; the simulator must not panic and produces only
        // uniform-baseline queries.
        let mut p = sample();
        p.interests = vec!["TOTALLY_UNKNOWN".to_string()];
        let week = simulate_week(&p, IntensityLevel::Medium, 5);
        assert!(week.total_queries() > 0);
        assert!(week
            .all_queries()
            .all(|q| q.weighting == QueryWeighting::UniformBaseline));
    }

    #[test]
    fn week_serializes_and_round_trips() -> crate::Result<()> {
        let week = simulate_week(&sample(), IntensityLevel::Low, 1);
        let json = serde_json::to_string(&week)?;
        assert!(json.contains("\"personaId\""));
        assert!(json.contains("\"sessions\""));
        let back: SimulatedWeek = serde_json::from_str(&json)?;
        assert_eq!(back, week);
        Ok(())
    }
}
