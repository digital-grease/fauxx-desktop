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

//! Desktop-side QUERY PLAN generation (C6 #28, H1).
//!
//! [`generate_query_plan`] produces a category-targeted, timed schedule of query
//! INTENTS the phone replays, the desktop-side analogue of the phone's on-device
//! generation. It reuses the two FROZEN models the studio week-simulator and the
//! C1 household scheduler already use, so a desktop-generated plan matches what
//! the phone would have produced for itself:
//!
//! - TIMING: the circadian Poisson model from
//!   [`crate::orchestration::scheduler`] (activity only inside the 07:00-23:00
//!   active window; Poisson inter-arrival delays `-ln(1 - u) / rate`).
//! - CATEGORY SELECTION: the persona-following weight BLEND from
//!   [`crate::constants`] (the Android `computeWeights`: each category's weight is
//!   `0.85 * w + 0.15 * 0.6`, with `w = 2.0` for an interest else `0.3`), BIASED
//!   by the signed [`WeightMap`] from the
//!   adversarial allocator (the plan multiplies the persona blend by the weight
//!   map so the allocator's protected-interest emphasis carries into the plan).
//!
//! ## Honesty: intents, not full query strings
//!
//! This emits category-targeted INTENTS (category + time + intensity, with an
//! OPTIONAL generated query string left `None`). It does NOT port the Android
//! `GrammarQueryGenerator` query banks + grammar, so it makes NO claim of full
//! E6 fidelity: the phone EXPANDS each intent into a concrete query with its own
//! on-device generator on replay. Porting the E6 query banks and grammar to
//! desktop (to fill in [`QueryIntent::query`]) is a focused FOLLOW-UP; see
//! `crates/fauxx-core/docs/C6_GENERATE.md`.
//!
//! ## Determinism
//!
//! All randomness flows through a seedable [`StdRng`] (mixed with a stable hash
//! of the persona id, exactly as the week simulator does), so the same
//! `(persona, weight_map, intensity, seed)` yields an identical plan and a new
//! seed re-rolls it.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::constants::{
    ALIGNED_WEIGHT, MISALIGNED_WEIGHT, NEUTRAL_WEIGHT, PERSONA_FOLLOW_FRACTION,
    UNIFORM_BASELINE_WEIGHT,
};
use crate::generate::allocator::{WeightMap, EPS};
use crate::orchestration::scheduler::{
    IntensityLevel, ACTIVE_WINDOW_END_SECS, ACTIVE_WINDOW_START_SECS,
};
use crate::persona::{CategoryPool, SyntheticPersona};

/// One scheduled query INTENT: a category to target, when to fire it (seconds
/// past local midnight, inside the active window), the intensity context, and an
/// OPTIONAL generated query string.
///
/// `query` is `None` here (desktop emits the category target; the phone expands
/// it with its own `E6` generator). The field exists so a future desktop-side
/// `GrammarQueryGenerator` port can fill it in without a schema change; it is
/// omitted from JSON when `None` so the phone's lenient reader never sees a key
/// it does not know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryIntent {
    /// Offset from local midnight, in seconds (always inside the active window).
    pub at_secs: i64,
    /// The targeted interest category (a [`CategoryPool`] enum name).
    pub category: String,
    /// The intensity context this intent was planned under.
    pub intensity: IntensityLevel,
    /// An OPTIONAL desktop-generated query string. `None` today (the phone
    /// expands the category target on replay); omitted from JSON when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// A timed, category-targeted plan of query intents for one persona over one
/// active day. The phone replays it, falling back to its own on-device
/// generation when no fresh, valid desktop plan is present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPlan {
    /// The persona id this plan was generated for.
    pub persona_id: String,
    /// The intensity used (parity with execution).
    pub intensity: IntensityLevel,
    /// The seed that produced this plan (re-rollable with a new seed).
    pub seed: u64,
    /// The query intents, in time order.
    pub intents: Vec<QueryIntent>,
}

impl QueryPlan {
    /// Total number of intents in the plan.
    pub fn len(&self) -> usize {
        self.intents.len()
    }

    /// Whether the plan carries no intents.
    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    /// How many intents target `category` (a [`CategoryPool`] enum name).
    pub fn category_count(&self, category: &str) -> usize {
        self.intents
            .iter()
            .filter(|i| i.category == category)
            .count()
    }
}

/// A small, stable (build-independent) FNV-1a hash of a string into a `u64`, used
/// to mix the persona id into the RNG seed so two personas under one seed get
/// distinct (but each reproducible) plans. Matches the week simulator's seeding.
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

/// The persona-following blended weight for one category, the same blend the
/// week simulator and execution path use: `follow * w + (1 - follow) * baseline`,
/// with `w = ALIGNED_WEIGHT` for an interest else `MISALIGNED_WEIGHT`, degrading
/// to `NEUTRAL_WEIGHT` when the persona declares no known interests.
fn persona_blend_weight(c: CategoryPool, aligned: &[CategoryPool]) -> f64 {
    let baseline = UNIFORM_BASELINE_WEIGHT * (1.0 - PERSONA_FOLLOW_FRACTION);
    if aligned.is_empty() {
        NEUTRAL_WEIGHT
    } else if aligned.contains(&c) {
        ALIGNED_WEIGHT * PERSONA_FOLLOW_FRACTION + baseline
    } else {
        MISALIGNED_WEIGHT * PERSONA_FOLLOW_FRACTION + baseline
    }
}

/// Generate a deterministic, category-targeted, timed [`QueryPlan`] for `persona`
/// at `intensity`, seeded by `seed`, with category selection BIASED by the signed
/// `weight_map` from the adversarial allocator.
///
/// Timing follows the circadian Poisson model (active window only); category
/// selection multiplies the persona-following blend by the weight map, so the
/// allocator's protected-interest emphasis carries into the plan. The emitted
/// intents are category targets with `query = None` (the phone expands them).
///
/// Deterministic: the same `(persona, weight_map, intensity, seed)` yields an
/// identical plan. Performs NO real browsing or network access.
pub fn generate_query_plan(
    persona: &SyntheticPersona,
    weight_map: &WeightMap,
    intensity: IntensityLevel,
    seed: u64,
) -> QueryPlan {
    // Mix the persona id into the seed for per-persona divergence under one seed.
    let mut rng = StdRng::seed_from_u64(seed ^ stable_hash(&persona.id));

    // The aligned set: the persona's known interests.
    let aligned: Vec<CategoryPool> = persona
        .interests
        .iter()
        .filter_map(|i| CategoryPool::from_name(i))
        .collect();

    // Precompute the per-category SELECTION weight: persona blend * weight-map
    // bias. The weight map biases the persona-following blend so the allocator's
    // emphasis (toward the protected interests, within the KL budget) shapes the
    // plan's category mix. A category absent from the map falls back to a neutral
    // 1.0 bias (the map covers every category, so this is just defensive).
    let all = CategoryPool::all();
    let weights: Vec<f64> = all
        .iter()
        .map(|c| {
            let bias = weight_map.get(c.as_name()).copied().unwrap_or(1.0).max(EPS);
            (persona_blend_weight(*c, &aligned) * bias).max(EPS)
        })
        .collect();
    let total: f64 = weights.iter().sum();

    let rate = intensity.rate_per_second();
    let mut intents = Vec::new();

    // Walk the active window sampling Poisson inter-arrival delays, exactly as
    // the household scheduler does, so plan timing matches execution.
    let mut t = ACTIVE_WINDOW_START_SECS as f64;
    loop {
        let u: f64 = rng.random::<f64>();
        let delay = -(1.0 - u).ln() / rate;
        t += delay;
        if t >= ACTIVE_WINDOW_END_SECS as f64 {
            break;
        }
        // One weighted draw over all categories picks this intent's target.
        let mut pick = rng.random::<f64>() * total;
        let mut chosen = all[all.len() - 1];
        for (c, w) in all.iter().zip(weights.iter()) {
            pick -= w;
            if pick < 0.0 {
                chosen = *c;
                break;
            }
        }
        intents.push(QueryIntent {
            at_secs: t.floor() as i64,
            category: chosen.as_name().to_string(),
            intensity,
            // Desktop emits the category target only; the phone expands it.
            query: None,
        });
    }

    QueryPlan {
        persona_id: persona.id.clone(),
        intensity,
        seed,
        intents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::allocator::{allocate, weight_map_from, KL_BUDGET};
    use crate::orchestration::scheduler::is_active_window;
    use crate::persona::{AgeRange, Profession, Region};

    fn persona(interests: &[CategoryPool]) -> SyntheticPersona {
        SyntheticPersona::new(
            "plan-test-0000-4000-8000-000000000001".to_string(),
            "Plan Test".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    fn interests() -> Vec<CategoryPool> {
        vec![
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::GAMING,
        ]
    }

    /// A weight map from the allocator, biased toward the persona's interests.
    fn allocated_map(interests: &[CategoryPool]) -> WeightMap {
        let blend = weight_map_from(|c| if interests.contains(&c) { 1.79 } else { 0.345 });
        allocate(&blend, interests, KL_BUDGET)
    }

    #[test]
    fn same_inputs_yield_identical_plan() {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let a = generate_query_plan(&p, &map, IntensityLevel::Medium, 42);
        let b = generate_query_plan(&p, &map, IntensityLevel::Medium, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn new_seed_re_rolls_the_plan() {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let a = generate_query_plan(&p, &map, IntensityLevel::Medium, 42);
        let c = generate_query_plan(&p, &map, IntensityLevel::Medium, 43);
        assert_ne!(a, c);
    }

    #[test]
    fn every_intent_targets_a_valid_category() {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let plan = generate_query_plan(&p, &map, IntensityLevel::High, 7);
        assert!(!plan.is_empty());
        for intent in &plan.intents {
            assert!(
                CategoryPool::from_name(&intent.category).is_some(),
                "intent targets unknown category {}",
                intent.category
            );
        }
    }

    #[test]
    fn all_intents_stay_inside_the_active_window() {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let plan = generate_query_plan(&p, &map, IntensityLevel::High, 99);
        assert!(!plan.is_empty());
        for intent in &plan.intents {
            assert!(
                is_active_window(intent.at_secs),
                "intent at {} is outside the active window",
                intent.at_secs
            );
            assert!(intent.at_secs >= ACTIVE_WINDOW_START_SECS);
            assert!(intent.at_secs < ACTIVE_WINDOW_END_SECS);
        }
    }

    #[test]
    fn intents_are_time_ordered() {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let plan = generate_query_plan(&p, &map, IntensityLevel::Medium, 5);
        for w in plan.intents.windows(2) {
            assert!(w[0].at_secs <= w[1].at_secs);
        }
    }

    #[test]
    fn plan_biases_toward_persona_and_weight_map_categories() {
        // With the allocator's weight map (biased toward the interests) layered on
        // the persona blend, the plan must lean well above the uniform share
        // toward the persona/weight-map interest categories.
        let ints = interests();
        let p = persona(&ints);
        let map = allocated_map(&ints);
        let plan = generate_query_plan(&p, &map, IntensityLevel::High, 2024);
        let total = plan.len();
        assert!(total > 100, "need a meaningful sample, got {total}");

        let interest_hits: usize = ints.iter().map(|c| plan.category_count(c.as_name())).sum();
        let interest_share = interest_hits as f64 / total as f64;
        let uniform_share = ints.len() as f64 / CategoryPool::all().len() as f64;
        assert!(
            interest_share > uniform_share * 2.0,
            "interest share {interest_share} should lean well above uniform {uniform_share}"
        );
    }

    #[test]
    fn query_string_is_none_intents_only() -> crate::Result<()> {
        // HONESTY check: desktop emits category targets, NOT full query strings.
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let plan = generate_query_plan(&p, &map, IntensityLevel::Low, 1);
        assert!(plan.intents.iter().all(|i| i.query.is_none()));
        // And the JSON omits the `query` key entirely when unset.
        let json = serde_json::to_string(&plan)?;
        assert!(!json.contains("\"query\""));
        Ok(())
    }

    #[test]
    fn plan_serializes_and_round_trips() -> crate::Result<()> {
        let p = persona(&interests());
        let map = allocated_map(&interests());
        let plan = generate_query_plan(&p, &map, IntensityLevel::Low, 1);
        let json = serde_json::to_string(&plan)?;
        assert!(json.contains("\"personaId\""));
        assert!(json.contains("\"intents\""));
        let back: QueryPlan = serde_json::from_str(&json)?;
        assert_eq!(back, plan);
        Ok(())
    }
}
