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

//! Category distributions and the KL-divergence drift metric (C4 #20, A1).
//!
//! A [`CategoryDistribution`] is a tally of how often each category was observed
//! at one point in time (one Topics read-back, or one broker scan). It is the
//! atomic unit the dashboard's drift series is built from.
//!
//! ## The drift metric: KL(baseline || observed)
//!
//! Profile drift is measured as the Kullback-Leibler divergence FROM the
//! baseline distribution `p` TO the observed distribution `q`:
//!
//! ```text
//! D_KL(p || q) = sum over categories c of  p(c) * ln( p(c) / q(c) )
//! ```
//!
//! It is `0` exactly when `p == q` (no drift) and grows as the observed picture
//! diverges from the baseline. We measure `KL(baseline || observed)` so the
//! baseline is the reference the observed profile is scored against: each term
//! is weighted by the baseline mass `p(c)`, i.e. "how surprised is the baseline
//! model to see the observed profile". The sign convention and direction are
//! frozen here so every platform series and every A/B cohort is comparable.
//!
//! ## Smoothing (no infinities, no NaN)
//!
//! Raw KL divergence is undefined when a category has zero probability under one
//! distribution but not the other (`ln(p/0)` is `+inf`; `0 * ln(0/q)` needs the
//! `0 * ln 0 = 0` convention). Observed profiles routinely have zero mass on
//! categories the baseline cares about (and vice versa), so we apply additive
//! (Laplace) smoothing with a small [`epsilon`](Smoothing::epsilon) pseudo-count
//! to BOTH distributions over the UNION of their categories before normalizing.
//! With a positive epsilon every category has positive probability under both
//! `p` and `q`, so every term is finite and the sum is a well-defined,
//! non-negative real number. The [`0 * ln(0/q) = 0`] edge is also handled
//! explicitly as a defensive belt-and-braces guard.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The default additive-smoothing pseudo-count. Small enough not to wash out the
/// signal on realistic tallies, large enough that a single zero-probability
/// category never produces an infinity. Documented and frozen so every series is
/// computed identically.
pub const DEFAULT_EPSILON: f64 = 0.5;

/// Additive (Laplace) smoothing applied to category distributions before the KL
/// divergence is taken, so a zero-probability category cannot yield `inf`/`NaN`.
///
/// A pseudo-count of `epsilon` is added to EVERY category in the union of the
/// two distributions' supports, under both distributions, before normalizing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Smoothing {
    /// The additive pseudo-count. Must be strictly positive to guarantee finite
    /// divergences; a non-positive value is clamped to a tiny floor by
    /// [`Smoothing::effective_epsilon`].
    pub epsilon: f64,
}

impl Smoothing {
    /// Smoothing with the [`DEFAULT_EPSILON`] pseudo-count.
    pub const fn new() -> Self {
        Self {
            epsilon: DEFAULT_EPSILON,
        }
    }

    /// Smoothing with an explicit pseudo-count.
    pub const fn with_epsilon(epsilon: f64) -> Self {
        Self { epsilon }
    }

    /// The epsilon actually used: the configured value, floored to a tiny
    /// strictly-positive constant so a zero/negative/`NaN` epsilon can never
    /// reintroduce an infinity. This keeps the metric total even on misuse.
    pub fn effective_epsilon(&self) -> f64 {
        // `f64::MIN_POSITIVE` is too small to be numerically useful; use a tiny
        // but representable floor instead.
        const FLOOR: f64 = 1e-9;
        if self.epsilon.is_finite() && self.epsilon > FLOOR {
            self.epsilon
        } else {
            FLOOR
        }
    }
}

impl Default for Smoothing {
    fn default() -> Self {
        Self::new()
    }
}

/// A category tally at one observation: how many times each category label was
/// seen. Category labels are opaque strings (a Topics taxonomy name or numeric
/// id, a broker category, etc.), so the same machinery serves every platform.
///
/// Counts are `f64` so weighted observations (e.g. a persona's intended category
/// WEIGHTS as the baseline) and integer tallies share one type. An EMPTY
/// distribution is well-formed: it normalizes to nothing and is handled without
/// panicking or dividing by zero everywhere it is consumed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CategoryDistribution {
    /// Per-category counts, keyed by the opaque category label. `BTreeMap` keeps
    /// the support in deterministic (sorted) order for stable output.
    counts: BTreeMap<String, f64>,
}

impl CategoryDistribution {
    /// An empty distribution (no observations).
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a distribution from `(category, count)` pairs. Repeated categories
    /// accumulate; negative or non-finite counts are dropped (a tally is never
    /// negative, and a `NaN`/`inf` count would poison the metric).
    pub fn from_counts<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (S, f64)>,
        S: Into<String>,
    {
        let mut dist = Self::new();
        for (category, count) in pairs {
            dist.add(category, count);
        }
        dist
    }

    /// Add `count` observations of `category`. Non-finite or negative counts are
    /// ignored so the distribution stays a valid tally.
    pub fn add(&mut self, category: impl Into<String>, count: f64) {
        if !count.is_finite() || count <= 0.0 {
            return;
        }
        *self.counts.entry(category.into()).or_insert(0.0) += count;
    }

    /// Add one observation of `category` (the common tally-by-one case).
    pub fn observe(&mut self, category: impl Into<String>) {
        self.add(category, 1.0);
    }

    /// Whether the distribution has no observations.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The number of distinct categories observed.
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// The total count across all categories (the normalizing denominator).
    pub fn total(&self) -> f64 {
        self.counts.values().sum()
    }

    /// The raw count for one category (0 if unobserved).
    pub fn count(&self, category: &str) -> f64 {
        self.counts.get(category).copied().unwrap_or(0.0)
    }

    /// The category labels observed, in sorted order.
    pub fn categories(&self) -> impl Iterator<Item = &str> {
        self.counts.keys().map(String::as_str)
    }

    /// Iterate `(category, count)` pairs in sorted category order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, f64)> {
        self.counts.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// One category's contribution to the total KL divergence, for the per-category
/// drift heatmap (C4 #20). The `contribution` values across a [`DriftBreakdown`]
/// sum (up to floating-point rounding) to its [`total`](DriftBreakdown::total).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryContribution {
    /// The category label.
    pub category: String,
    /// The smoothed baseline probability `p(c)`.
    pub baseline_p: f64,
    /// The smoothed observed probability `q(c)`.
    pub observed_q: f64,
    /// This category's term of the divergence: `p(c) * ln(p(c) / q(c))`. May be
    /// negative for an individual category (when `q(c) > p(c)`); the SUM over all
    /// categories is the non-negative total divergence.
    pub contribution: f64,
}

/// The full per-category breakdown of one KL divergence: the total plus each
/// category's contribution, in sorted category order. Powers both the scalar
/// drift timeline ([`total`](Self::total)) and the per-category heatmap
/// ([`contributions`](Self::contributions)).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftBreakdown {
    /// The total KL divergence `D_KL(baseline || observed)`. Always finite and
    /// `>= 0` (clamped at 0 to absorb tiny negative floating-point rounding).
    pub total: f64,
    /// Each category's contribution, sorted by category label. Summing the
    /// `contribution` fields reproduces [`total`](Self::total).
    pub contributions: Vec<CategoryContribution>,
}

impl DriftBreakdown {
    /// The divergence breakdown of an empty observation against an empty
    /// baseline: zero drift, no contributions.
    fn empty() -> Self {
        Self {
            total: 0.0,
            contributions: Vec::new(),
        }
    }
}

/// Compute `D_KL(baseline || observed)` with additive smoothing, returning the
/// scalar total only. See [`kl_divergence_breakdown`] for the per-category
/// detail; this is the convenience wrapper the scalar timeline uses.
pub fn kl_divergence(
    baseline: &CategoryDistribution,
    observed: &CategoryDistribution,
    smoothing: Smoothing,
) -> f64 {
    kl_divergence_breakdown(baseline, observed, smoothing).total
}

/// Compute the smoothed `D_KL(baseline || observed)` AND its per-category
/// breakdown in one pass (C4 #20).
///
/// Algorithm:
///
/// 1. Take the UNION of the two distributions' category supports.
/// 2. Add `epsilon` to every category's count under both distributions, then
///    normalize each to a probability vector (`p`, `q`). With a positive epsilon
///    every category has strictly positive probability under both, so no term is
///    infinite.
/// 3. Each category contributes `p(c) * ln(p(c) / q(c))`; the total is their sum.
///
/// Degenerate inputs are handled WITHOUT panicking, dividing by zero, or
/// producing `NaN`/`inf`:
///
/// - If BOTH distributions are empty, the result is the zero-drift
///   [`DriftBreakdown::empty`].
/// - If only ONE is empty, the non-empty side defines the support; smoothing
///   makes the empty side uniform over that support, yielding a finite, positive
///   divergence rather than an infinity.
/// - The `0 * ln(0/q) = 0` convention is applied explicitly as a guard, even
///   though smoothing already prevents a true zero `p(c)`.
pub fn kl_divergence_breakdown(
    baseline: &CategoryDistribution,
    observed: &CategoryDistribution,
    smoothing: Smoothing,
) -> DriftBreakdown {
    // The union of supports, in deterministic sorted order.
    let mut categories: Vec<&str> = Vec::new();
    for c in baseline.categories() {
        categories.push(c);
    }
    for c in observed.categories() {
        if !categories.contains(&c) {
            categories.push(c);
        }
    }
    categories.sort_unstable();
    categories.dedup();

    if categories.is_empty() {
        return DriftBreakdown::empty();
    }

    let epsilon = smoothing.effective_epsilon();
    let k = categories.len() as f64;

    // Smoothed totals: raw total + epsilon for each category in the union.
    let baseline_total = baseline.total() + epsilon * k;
    let observed_total = observed.total() + epsilon * k;

    // Both denominators are >= epsilon * k > 0, so the divisions are always safe.
    let mut total = 0.0;
    let mut contributions = Vec::with_capacity(categories.len());
    for category in categories {
        let p = (baseline.count(category) + epsilon) / baseline_total;
        let q = (observed.count(category) + epsilon) / observed_total;

        // p and q are both strictly positive after smoothing; the explicit
        // `p == 0` guard is defensive belt-and-braces for the `0 ln 0 = 0`
        // convention so this stays total even if a caller forces epsilon to 0.
        let contribution = if p > 0.0 { p * (p / q).ln() } else { 0.0 };

        total += contribution;
        contributions.push(CategoryContribution {
            category: category.to_string(),
            baseline_p: p,
            observed_q: q,
            contribution,
        });
    }

    // KL divergence is mathematically >= 0; clamp tiny negative rounding to 0 so
    // the scalar timeline never shows a spurious sub-zero blip.
    if total < 0.0 && total > -1e-12 {
        total = 0.0;
    }

    DriftBreakdown {
        total,
        contributions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(pairs: &[(&str, f64)]) -> CategoryDistribution {
        CategoryDistribution::from_counts(pairs.iter().map(|(c, n)| (*c, *n)))
    }

    #[test]
    fn identical_distributions_have_zero_divergence() {
        let p = dist(&[("a", 3.0), ("b", 1.0), ("c", 6.0)]);
        let q = p.clone();
        let kl = kl_divergence(&p, &q, Smoothing::new());
        assert!(kl.abs() < 1e-12, "expected ~0 divergence, got {kl}");
    }

    #[test]
    fn hand_computed_example_matches() {
        // Two categories, no zeros, so with epsilon -> 0 the smoothed result
        // approaches the textbook value. Baseline p = (0.5, 0.5); observed
        // q = (0.25, 0.75).
        //   D = 0.5*ln(0.5/0.25) + 0.5*ln(0.5/0.75)
        //     = 0.5*ln(2) + 0.5*ln(2/3)
        //     = 0.5*0.6931471805599453 + 0.5*(-0.4054651081095832)
        //     = 0.34657359027997264 - 0.2027325540547916
        //     = 0.14384103622518104
        let p = dist(&[("a", 50.0), ("b", 50.0)]);
        let q = dist(&[("a", 25.0), ("b", 75.0)]);
        // Tiny epsilon so smoothing barely perturbs the textbook value.
        let kl = kl_divergence(&p, &q, Smoothing::with_epsilon(1e-6));
        let expected = 0.14384103622518104;
        assert!(
            (kl - expected).abs() < 1e-4,
            "expected ~{expected}, got {kl}"
        );
    }

    #[test]
    fn smoothing_prevents_infinity_on_zero_probability_category() {
        // Observed has zero mass on "b" that the baseline cares about, and a
        // brand-new category "c" the baseline never saw. Without smoothing this
        // would be ln(p/0) = +inf; with smoothing it must be finite.
        let p = dist(&[("a", 1.0), ("b", 1.0)]);
        let q = dist(&[("a", 2.0), ("c", 2.0)]);
        let kl = kl_divergence(&p, &q, Smoothing::new());
        assert!(kl.is_finite(), "divergence must be finite, got {kl}");
        assert!(!kl.is_nan(), "divergence must not be NaN");
        assert!(kl > 0.0, "diverging profiles must have positive drift");
    }

    #[test]
    fn per_category_contributions_sum_to_total() {
        let p = dist(&[("a", 5.0), ("b", 3.0), ("c", 2.0)]);
        let q = dist(&[("a", 1.0), ("b", 4.0), ("d", 5.0)]);
        let breakdown = kl_divergence_breakdown(&p, &q, Smoothing::new());
        let summed: f64 = breakdown.contributions.iter().map(|c| c.contribution).sum();
        assert!(
            (summed - breakdown.total).abs() < 1e-12,
            "contributions {summed} must sum to total {}",
            breakdown.total
        );
        // The union support is {a, b, c, d}; every category appears once.
        assert_eq!(breakdown.contributions.len(), 4);
        let cats: Vec<&str> = breakdown
            .contributions
            .iter()
            .map(|c| c.category.as_str())
            .collect();
        assert_eq!(cats, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn both_empty_yields_zero_no_nan() {
        let breakdown = kl_divergence_breakdown(
            &CategoryDistribution::new(),
            &CategoryDistribution::new(),
            Smoothing::new(),
        );
        assert_eq!(breakdown.total, 0.0);
        assert!(breakdown.contributions.is_empty());
    }

    #[test]
    fn one_side_empty_is_finite_positive() {
        // Empty observed against a non-empty baseline: smoothing makes the
        // observed uniform over the baseline's support, a finite positive drift.
        let p = dist(&[("a", 10.0), ("b", 1.0)]);
        let q = CategoryDistribution::new();
        let kl = kl_divergence(&p, &q, Smoothing::new());
        assert!(kl.is_finite() && !kl.is_nan());
        assert!(kl > 0.0);
    }

    #[test]
    fn non_finite_and_negative_counts_are_dropped() {
        let mut d = CategoryDistribution::new();
        d.add("a", f64::NAN);
        d.add("b", f64::INFINITY);
        d.add("c", -3.0);
        d.add("d", 2.0);
        assert_eq!(d.len(), 1);
        assert_eq!(d.count("d"), 2.0);
        assert_eq!(d.total(), 2.0);
    }

    #[test]
    fn zero_epsilon_is_floored_and_stays_finite() {
        // A misused zero epsilon must not reintroduce an infinity: the floor
        // keeps the metric total even with a disjoint support.
        let p = dist(&[("a", 1.0)]);
        let q = dist(&[("b", 1.0)]);
        let kl = kl_divergence(&p, &q, Smoothing::with_epsilon(0.0));
        assert!(kl.is_finite() && !kl.is_nan(), "got {kl}");
    }
}
