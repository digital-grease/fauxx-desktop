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

//! Adversarial-allocation surrogate (the on-phone `AdversarialAllocator.allocate`
//! contract, E4, ported FAITHFULLY).
//!
//! [`allocate`] takes a per-category weight vector (the persona-following BLEND,
//! one weight per [`CategoryPool`]), a set of PROTECTED interests the user has
//! pinned, and a [`KL_BUDGET`] (0.15 nats). It runs COORDINATE DESCENT over the
//! categories, trying each multiplicative [`FACTOR`](FACTORS) on each
//! perturbable category and keeping a move only while the result's KL divergence
//! from the input baseline stays within the budget. The optimization objective
//! is the adversary's loss surrogate: spread mass toward the protected interests
//! (so the persona keeps serving the user's declared interests) while the KL
//! budget caps how far the distribution may drift from the baseline (so the
//! perturbation stays plausible and bounded).
//!
//! ## Frozen contract
//!
//! - [`PASSES`] = 10 full sweeps of coordinate descent.
//! - [`FACTORS`] = `[0.4, 0.6, 0.8, 1.25, 1.7, 2.5]`, the multiplicative trial
//!   steps tried on each category, in order.
//! - [`EPS`] = `1e-6`, the numeric floor used to keep logs/divisions finite.
//! - The [`SensitiveAttributes`] denylist categories are NEVER perturbed by the
//!   descent. The final uniform Pass-2 rescale preserves their pairwise ratios
//!   (their share among themselves holds whenever each one's normalized share is
//!   at or above [`MIN_WEIGHT`], the realistic case), so a sensitive topic the
//!   user might genuinely care about is neither amplified nor suppressed by the
//!   adversarial surrogate.
//! - After descent, [`WeightNormalizer::normalize`] applies the MIN_WEIGHT
//!   ([`MIN_WEIGHT`]) two-pass clamp-and-divide so the result sums to ~1.0 with
//!   the floor preserved and no category is ever truly zero.
//! - The KL budget is enforced on the pre-clamp allocation during descent. The
//!   final MIN_WEIGHT floor can add a tiny, bounded slack (it only raises
//!   near-zero entries to `MIN_WEIGHT`), so the final distribution's divergence
//!   from the baseline is within [`KL_BUDGET`] plus that small floor slack.
//!
//! ## Determinism
//!
//! Coordinate descent here is fully DETERMINISTIC: it visits categories in the
//! fixed [`CategoryPool::all`] order, tries factors in their fixed order, and
//! takes the first improving in-budget move. No randomness is involved, so a
//! known input yields one fixed allocation (the tests pin this).

use std::collections::BTreeMap;

use crate::persona::CategoryPool;

/// The KL-divergence budget, in nats. The descent never accepts a perturbation
/// whose pre-clamp KL from the input baseline exceeds this; the final MIN_WEIGHT
/// floor may add a small bounded slack on top (see the module docs). The
/// on-phone allocator uses the same 0.15-nat cap.
pub const KL_BUDGET: f64 = 0.15;

/// Full coordinate-descent sweeps over the category set.
pub const PASSES: usize = 10;

/// The multiplicative trial steps tried on each perturbable category, in order.
/// Mirrors the on-phone allocator's `FACTORS` exactly.
pub const FACTORS: [f64; 6] = [0.4, 0.6, 0.8, 1.25, 1.7, 2.5];

/// Numeric floor used to keep logs and divisions finite during descent (a
/// weight is never driven below this while optimizing).
pub const EPS: f64 = 1e-6;

/// The minimum normalized weight any category may carry after normalization. The
/// two-pass clamp-and-divide guarantees no category is ever truly zero. Mirrors
/// the on-phone `WeightNormalizer.MIN_WEIGHT`.
pub const MIN_WEIGHT: f64 = 0.001;

/// The frozen SENSITIVE-ATTRIBUTE denylist (the on-phone `SensitiveAttributes`):
/// categories the adversarial allocator must NEVER perturb. Their relative mass
/// (their share among themselves) is preserved exactly so the surrogate neither
/// amplifies nor suppresses a topic the user might genuinely, sensitively care
/// about. Every entry is a [`CategoryPool`] variant, so the set stays in lockstep
/// with the frozen 32-value pool.
pub const SENSITIVE_ATTRIBUTES: [CategoryPool; 5] = [
    CategoryPool::MEDICAL,
    CategoryPool::LEGAL,
    CategoryPool::POLITICS,
    CategoryPool::RELATIONSHIPS_DATING,
    CategoryPool::WELLNESS_ALTERNATIVE,
];

/// Whether `category` is on the [`SENSITIVE_ATTRIBUTES`] denylist (never
/// perturbed by the adversarial allocator).
pub fn is_sensitive(category: CategoryPool) -> bool {
    SENSITIVE_ATTRIBUTES.contains(&category)
}

/// A distribution over EVERY [`CategoryPool`], keyed by the frozen Android enum
/// name (e.g. `"FINANCE"`). The allocator's input and output both use this shape
/// so it round-trips to the phone losslessly. Stored in a [`BTreeMap`] so the
/// JSON key order is stable (which keeps a signed artifact's canonical bytes
/// deterministic).
pub type WeightMap = BTreeMap<String, f64>;

/// Build a full-coverage [`WeightMap`] from a closure that scores each category.
/// Every [`CategoryPool`] gets an entry; a non-finite or negative score is
/// floored to [`EPS`] so the map never carries an invalid weight.
pub fn weight_map_from<F: Fn(CategoryPool) -> f64>(score: F) -> WeightMap {
    let mut map = WeightMap::new();
    for c in CategoryPool::all() {
        let w = score(*c);
        let w = if w.is_finite() && w > 0.0 { w } else { EPS };
        map.insert(c.as_name().to_string(), w);
    }
    map
}

/// Normalize a [`WeightMap`] in place to a probability distribution (sums to
/// ~1.0). Returns the dense weight vector in [`CategoryPool::all`] order. Used
/// internally and by tests.
fn dense(map: &WeightMap) -> Vec<f64> {
    CategoryPool::all()
        .iter()
        .map(|c| map.get(c.as_name()).copied().unwrap_or(EPS).max(EPS))
        .collect()
}

/// Turn a dense weight vector back into a [`WeightMap`] in [`CategoryPool::all`]
/// order.
fn to_map(dense: &[f64]) -> WeightMap {
    let mut map = WeightMap::new();
    for (c, w) in CategoryPool::all().iter().zip(dense.iter()) {
        map.insert(c.as_name().to_string(), *w);
    }
    map
}

/// Normalize a dense vector to a probability distribution (sum 1.0), flooring at
/// [`EPS`] first so no entry is zero and the sum is positive.
fn normalized_probs(v: &[f64]) -> Vec<f64> {
    let floored: Vec<f64> = v.iter().map(|x| x.max(EPS)).collect();
    let total: f64 = floored.iter().sum();
    if total <= 0.0 {
        // Degenerate (cannot happen after the EPS floor), but fail safe to
        // uniform rather than dividing by zero.
        let n = floored.len().max(1) as f64;
        return vec![1.0 / n; floored.len()];
    }
    floored.iter().map(|x| x / total).collect()
}

/// KL divergence `D(p || q)` in nats between two distributions of equal length,
/// each first renormalized to sum to 1.0 (with an [`EPS`] floor so the log terms
/// stay finite). `p` is the candidate/result, `q` is the baseline.
pub fn kl_divergence(p: &[f64], q: &[f64]) -> f64 {
    if p.len() != q.len() {
        return f64::INFINITY;
    }
    let pn = normalized_probs(p);
    let qn = normalized_probs(q);
    let mut kl = 0.0;
    for (pi, qi) in pn.iter().zip(qn.iter()) {
        let pi = pi.max(EPS);
        let qi = qi.max(EPS);
        kl += pi * (pi / qi).ln();
    }
    // Numeric noise can push a near-zero KL slightly negative; clamp at 0.
    kl.max(0.0)
}

/// The KL divergence (nats) between two [`WeightMap`]s, treating each as a
/// distribution over the frozen [`CategoryPool`]. Convenience over
/// [`kl_divergence`] for callers and tests holding maps.
pub fn weight_map_kl(result: &WeightMap, baseline: &WeightMap) -> f64 {
    kl_divergence(&dense(result), &dense(baseline))
}

/// The adversary's loss SURROGATE for one candidate distribution.
///
/// Lower is better. The surrogate is the negative log-mass the candidate places
/// on the PROTECTED interest categories: minimizing it spreads probability toward
/// the user's pinned interests (so the persona keeps serving them), while the KL
/// budget (checked separately) bounds how far the candidate may drift from the
/// baseline. With no protected interests there is nothing to optimize toward, so
/// the surrogate is flat and descent makes no move (the baseline survives,
/// then gets normalized).
fn loss(dense: &[f64], protected_idx: &[usize]) -> f64 {
    if protected_idx.is_empty() {
        return 0.0;
    }
    let probs = normalized_probs(dense);
    let protected_mass: f64 = protected_idx.iter().map(|&i| probs[i]).sum();
    -(protected_mass.max(EPS)).ln()
}

/// The two-pass MIN_WEIGHT clamp-and-divide normalizer (the on-phone
/// `WeightNormalizer`).
///
/// Pass 1 normalizes the raw weights to sum to 1.0, then CLAMPS every entry up to
/// [`MIN_WEIGHT`] so no category is ever truly zero. Pass 2 divides the clamped
/// vector by its sum so the result sums back to ~1.0. This is a UNIFORM scaling,
/// so it preserves the relative mass (the pairwise ratios) of every entry that
/// was not raised by the clamp, which is why the never-perturbed sensitive
/// categories keep their relative mass through normalization.
///
/// The post-divide value of an entry that was clamped UP to the floor is
/// `MIN_WEIGHT / total`; because clamping can only raise `total` to at most
/// slightly above 1 (the input summed to 1 and at most `n` floors were added,
/// `n * MIN_WEIGHT = 0.032` for the 32-category pool), a clamped entry can dip a
/// hair below `MIN_WEIGHT` but stays STRICTLY POSITIVE: the contract is that no
/// category is ever truly zero, with the floor preserved up to the final
/// uniform rescale.
pub struct WeightNormalizer;

impl WeightNormalizer {
    /// Normalize a dense weight vector with the two-pass clamp-and-divide. The
    /// result sums to ~1.0, every entry is strictly positive (never zero), and
    /// the relative mass of the unclamped entries is preserved (uniform scaling).
    pub fn normalize_dense(weights: &[f64]) -> Vec<f64> {
        // Pass 1: normalize, then clamp up to the floor (no entry ever zero).
        let p1 = normalized_probs(weights);
        let clamped: Vec<f64> = p1.iter().map(|x| x.max(MIN_WEIGHT)).collect();
        // Pass 2: divide by the clamped sum. Uniform scaling preserves ratios.
        let total: f64 = clamped.iter().sum();
        if total <= 0.0 {
            let n = clamped.len().max(1) as f64;
            return vec![1.0 / n; clamped.len()];
        }
        clamped.iter().map(|x| x / total).collect()
    }

    /// Normalize a [`WeightMap`] with the two-pass clamp-and-divide, preserving
    /// the [`CategoryPool`] coverage and key order.
    pub fn normalize(map: &WeightMap) -> WeightMap {
        to_map(&Self::normalize_dense(&dense(map)))
    }
}

/// Run the adversarial-allocation surrogate over `combined` (the per-category
/// weight blend), favoring the `protected_interests`, under `kl_budget` (nats).
///
/// FAITHFUL port of the on-phone `AdversarialAllocator.allocate`:
/// - coordinate descent for [`PASSES`] sweeps;
/// - on each perturbable category, try each [`FACTORS`] step and take the first
///   one that strictly lowers the [`loss`] surrogate AND keeps the candidate's
///   [`kl_divergence`] from the baseline within `kl_budget`;
/// - NEVER perturb a [`SENSITIVE_ATTRIBUTES`] category (their relative mass is
///   preserved exactly);
/// - finally apply [`WeightNormalizer::normalize`] (the MIN_WEIGHT two-pass
///   clamp-and-divide) so the result sums to ~1.0 with the floor preserved.
///
/// The returned [`WeightMap`] covers EVERY [`CategoryPool`], sums to ~1.0, never
/// zeroes a category, and is within `kl_budget` of the input baseline.
pub fn allocate(
    combined: &WeightMap,
    protected_interests: &[CategoryPool],
    kl_budget: f64,
) -> WeightMap {
    let all = CategoryPool::all();
    // The baseline distribution the KL budget is measured against (the input,
    // normalized). Frozen for the whole descent.
    let baseline = normalized_probs(&dense(combined));

    // Working dense vector we perturb in place (raw, unnormalized weights; the
    // loss/KL functions normalize internally, so absolute scale is irrelevant).
    let mut work = dense(combined);

    // Indices of the protected interests (the descent target) and the sensitive
    // set (never perturbed). Both map into the frozen `all` order.
    let protected_idx: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, c)| protected_interests.contains(c))
        .map(|(i, _)| i)
        .collect();

    // With no protected interests the loss surrogate is constant 0 and the
    // descent can never accept a move; skip the 10x32 no-op sweep and go
    // straight to the normalized baseline.
    if protected_idx.is_empty() {
        return WeightNormalizer::normalize(&to_map(&work));
    }

    // Coordinate descent: PASSES sweeps, visiting categories in the fixed order.
    for _ in 0..PASSES {
        for (i, c) in all.iter().enumerate() {
            // Sensitive categories are NEVER perturbed: skip them entirely. The
            // uniform Pass-2 rescale then preserves their pairwise ratios.
            if is_sensitive(*c) {
                continue;
            }
            let current = work[i].max(EPS);
            let base_loss = loss(&work, &protected_idx);
            // Try each multiplicative factor in order; take the FIRST that both
            // lowers the loss and stays within the KL budget.
            for &f in FACTORS.iter() {
                let candidate = (current * f).max(EPS);
                let prev = work[i];
                work[i] = candidate;
                let cand_loss = loss(&work, &protected_idx);
                let cand_kl = kl_divergence(&work, &baseline);
                if cand_loss + EPS < base_loss && cand_kl <= kl_budget {
                    // Accept the move and stop trying factors for this category.
                    break;
                }
                // Reject: restore and try the next factor.
                work[i] = prev;
            }
        }
    }

    // Final MIN_WEIGHT two-pass clamp-and-divide: sums to ~1.0, floor preserved,
    // no category ever zero.
    WeightNormalizer::normalize(&to_map(&work))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A baseline blend: interests weighted high, everything else low. Mirrors the
    /// persona-following blend's two-tier shape (an interest ~5x a non-interest)
    /// without depending on the studio simulator.
    fn baseline_blend(interests: &[CategoryPool]) -> WeightMap {
        weight_map_from(|c| if interests.contains(&c) { 1.79 } else { 0.345 })
    }

    fn protected() -> Vec<CategoryPool> {
        vec![
            CategoryPool::FINANCE,
            CategoryPool::TECHNOLOGY,
            CategoryPool::TRAVEL,
        ]
    }

    fn sum(map: &WeightMap) -> f64 {
        map.values().sum()
    }

    #[test]
    fn output_covers_every_category() {
        let out = allocate(&baseline_blend(&protected()), &protected(), KL_BUDGET);
        assert_eq!(out.len(), CategoryPool::all().len());
        for c in CategoryPool::all() {
            assert!(out.contains_key(c.as_name()), "missing {}", c.as_name());
        }
    }

    #[test]
    fn output_sums_to_one() {
        let out = allocate(&baseline_blend(&protected()), &protected(), KL_BUDGET);
        let s = sum(&out);
        assert!((s - 1.0).abs() < 1e-9, "sum {s} is not ~1.0");
    }

    #[test]
    fn no_category_is_ever_zero_min_weight_floor_holds() {
        let out = allocate(&baseline_blend(&protected()), &protected(), KL_BUDGET);
        // The clamp-and-divide guarantees no category is ever truly zero, with the
        // floor preserved up to the final uniform rescale. For the 32-category
        // pool the rescale divisor is at most ~1.032 (1 + 32 * MIN_WEIGHT), so a
        // clamped entry stays within that factor of the floor and well above zero.
        let min_after_rescale = MIN_WEIGHT / (1.0 + CategoryPool::all().len() as f64 * MIN_WEIGHT);
        for (name, w) in &out {
            assert!(*w > 0.0, "category {name} is zero");
            assert!(
                *w >= min_after_rescale - 1e-12,
                "category {name} weight {w} fell below the rescaled floor {min_after_rescale}"
            );
        }
    }

    #[test]
    fn kl_divergence_stays_within_budget() {
        let baseline = baseline_blend(&protected());
        let out = allocate(&baseline, &protected(), KL_BUDGET);
        // Compare against the SAME baseline the allocator measured against; the
        // post-normalize KL must not exceed the budget (within a tiny numeric
        // tolerance for the final clamp-and-divide).
        let kl = weight_map_kl(&out, &baseline);
        assert!(
            kl <= KL_BUDGET + 1e-6,
            "KL {kl} exceeds the {KL_BUDGET}-nat budget"
        );
    }

    #[test]
    fn sensitive_categories_relative_mass_is_unperturbed() {
        // The relative mass AMONG the sensitive categories must be identical in
        // the input baseline and the output: the allocator never touches them, so
        // only the global renormalization scales them uniformly, leaving their
        // ratios intact.
        let mut blend = baseline_blend(&protected());
        // Give the sensitive categories distinct, non-uniform input mass so the
        // ratio test is meaningful.
        blend.insert(CategoryPool::MEDICAL.as_name().to_string(), 0.9);
        blend.insert(CategoryPool::LEGAL.as_name().to_string(), 0.3);
        blend.insert(CategoryPool::POLITICS.as_name().to_string(), 0.6);
        blend.insert(
            CategoryPool::RELATIONSHIPS_DATING.as_name().to_string(),
            0.15,
        );
        blend.insert(
            CategoryPool::WELLNESS_ALTERNATIVE.as_name().to_string(),
            0.45,
        );

        let out = allocate(&blend, &protected(), KL_BUDGET);

        // For each pair of sensitive categories, the output ratio must equal the
        // input ratio (uniform scaling preserves ratios).
        let names: Vec<String> = SENSITIVE_ATTRIBUTES
            .iter()
            .map(|c| c.as_name().to_string())
            .collect();
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                let in_ratio = blend[&names[i]] / blend[&names[j]];
                let out_ratio = out[&names[i]] / out[&names[j]];
                assert!(
                    (in_ratio - out_ratio).abs() < 1e-6,
                    "sensitive pair {}/{} ratio changed: in {in_ratio}, out {out_ratio}",
                    names[i],
                    names[j]
                );
            }
        }
    }

    #[test]
    fn known_input_is_deterministic() {
        let baseline = baseline_blend(&protected());
        let a = allocate(&baseline, &protected(), KL_BUDGET);
        let b = allocate(&baseline, &protected(), KL_BUDGET);
        assert_eq!(a, b, "allocation must be deterministic for a fixed input");
    }

    #[test]
    fn allocation_shifts_mass_toward_protected_interests() {
        // Start from a near-flat baseline so the only structure the allocator can
        // add is toward the protected interests.
        let baseline = weight_map_from(|_| 1.0);
        let prot = vec![CategoryPool::FINANCE, CategoryPool::TECHNOLOGY];
        let out = allocate(&baseline, &prot, KL_BUDGET);

        let protected_mass: f64 = prot.iter().map(|c| out[c.as_name()]).sum();
        let uniform_share = prot.len() as f64 / CategoryPool::all().len() as f64;
        assert!(
            protected_mass > uniform_share,
            "protected mass {protected_mass} should exceed the uniform share {uniform_share}"
        );
        // ...but the budget still bounds it (it does not collapse to the
        // interests alone).
        assert!(weight_map_kl(&out, &baseline) <= KL_BUDGET + 1e-6);
    }

    #[test]
    fn empty_protected_set_yields_normalized_baseline() {
        // With no protected interests there is nothing to optimize toward, so
        // descent makes no move; the output is just the normalized baseline.
        let baseline = baseline_blend(&protected());
        let out = allocate(&baseline, &[], KL_BUDGET);
        let expected = WeightNormalizer::normalize(&baseline);
        for c in CategoryPool::all() {
            let got = out[c.as_name()];
            let exp = expected[c.as_name()];
            assert!((got - exp).abs() < 1e-9, "{}: {got} vs {exp}", c.as_name());
        }
    }

    #[test]
    fn normalizer_two_pass_floor_and_sum() {
        // A vector with a hard zero must come out summing to ~1.0 with every entry
        // strictly positive (never zero) and the zero entry raised to the floor
        // up to the final uniform rescale.
        let n = CategoryPool::all().len();
        let mut v = vec![1.0; n];
        v[0] = 0.0;
        let out = WeightNormalizer::normalize_dense(&v);
        let s: f64 = out.iter().sum();
        assert!((s - 1.0).abs() < 1e-9, "sum {s}");
        // The clamped (was-zero) entry stays within the rescale factor of the
        // floor and is strictly positive.
        let min_after_rescale = MIN_WEIGHT / (1.0 + n as f64 * MIN_WEIGHT);
        for x in &out {
            assert!(*x > 0.0, "entry {x} is zero");
            assert!(
                *x >= min_after_rescale - 1e-12,
                "entry {x} below rescaled floor"
            );
        }
        // The was-zero entry is at the (rescaled) floor; the rest sit above it.
        assert!(out[0] < out[1], "the clamped entry should be the smallest");
    }

    #[test]
    fn kl_of_identical_distributions_is_zero() {
        let v = baseline_blend(&protected());
        assert!(weight_map_kl(&v, &v) < 1e-9);
    }

    #[test]
    fn sensitive_set_is_within_the_frozen_pool() {
        for c in SENSITIVE_ATTRIBUTES {
            assert!(
                CategoryPool::all().contains(&c),
                "{} is not a frozen CategoryPool",
                c.as_name()
            );
            assert!(is_sensitive(c));
        }
        assert!(!is_sensitive(CategoryPool::FINANCE));
    }
}
