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

//! Two-sample statistics for the control-profile A/B comparison (C4 #21, A2).
//!
//! These operate on two samples of the SAME drift metric (the A1 KL divergence,
//! one value per snapshot), so a "treated vs control" comparison is directly in
//! the dashboard's own units.
//!
//! Two quantities are computed, both guarded against tiny/degenerate samples:
//!
//! - [`cohens_d`]: the EFFECT SIZE, a standardized mean difference. Plain `f64`
//!   math. It answers "how big is the difference, in standard deviations".
//! - [`two_sample_t_test`]: the SIGNIFICANCE, a p-value from a two-sample
//!   t-test. The test statistic is plain math; the p-value uses the Student-t
//!   CDF from `statrs`. It answers "how likely is a difference this large under
//!   the null hypothesis of no difference".
//!
//! ## Welch vs pooled
//!
//! The default is WELCH's t-test (unequal-variance), via [`TTestKind::Welch`].
//! Welch does NOT assume the two cohorts share a variance, which is the safer
//! default for a treated-vs-control comparison where the noised profile may well
//! have a different drift variance than the control. The pooled (Student)
//! variant ([`TTestKind::Pooled`]) assumes equal variances and pools them into a
//! single estimate with `n1 + n2 - 2` degrees of freedom; it is offered for
//! callers who have established equal variances, but it is not the default. The
//! Welch-Satterthwaite formula gives Welch's fractional degrees of freedom.

use serde::{Deserialize, Serialize};
use statrs::distribution::{ContinuousCDF, StudentsT};

/// Summary statistics of one sample (count, mean, sample variance). Computed
/// once and reused so the effect size and the t-test agree on the inputs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SampleStats {
    /// Number of observations.
    pub n: usize,
    /// Arithmetic mean (0 for an empty sample).
    pub mean: f64,
    /// UNBIASED sample variance (Bessel's `n - 1` denominator). `0` for a sample
    /// of size `< 2`, where variance is undefined.
    pub variance: f64,
}

impl SampleStats {
    /// Compute the count, mean, and unbiased (`n - 1`) sample variance of
    /// `values`. Non-finite values are ignored so a stray `NaN` cannot poison
    /// the statistics. An empty (or all-non-finite) sample yields
    /// `{ n: 0, mean: 0, variance: 0 }` rather than `NaN`.
    pub fn of(values: &[f64]) -> Self {
        let clean: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        let n = clean.len();
        if n == 0 {
            return Self {
                n: 0,
                mean: 0.0,
                variance: 0.0,
            };
        }
        let mean = clean.iter().sum::<f64>() / n as f64;
        let variance = if n < 2 {
            // Variance is undefined for a single observation; report 0 (and the
            // significance test will guard the degenerate case below).
            0.0
        } else {
            let ss: f64 = clean.iter().map(|v| (v - mean).powi(2)).sum();
            ss / (n as f64 - 1.0)
        };
        Self { n, mean, variance }
    }

    /// The standard deviation (square root of the sample variance).
    pub fn std_dev(&self) -> f64 {
        self.variance.sqrt()
    }
}

/// Which two-sample t-test to run. See the module docs for Welch vs pooled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TTestKind {
    /// Welch's unequal-variance t-test (the default; does not assume equal
    /// variances). Uses the Welch-Satterthwaite fractional degrees of freedom.
    #[default]
    Welch,
    /// The pooled-variance (Student) t-test, assuming the two cohorts share a
    /// variance. Degrees of freedom `n1 + n2 - 2`.
    Pooled,
}

/// The outcome of a two-sample t-test (C4 #21). Carries enough to render a plain
/// summary: the statistic, its degrees of freedom, and the two-sided p-value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TTestResult {
    /// Which test was run.
    pub kind: TTestKind,
    /// The t statistic. `0.0` when the means are equal; sign follows
    /// `mean(a) - mean(b)`.
    pub t_statistic: f64,
    /// The degrees of freedom (fractional for Welch).
    pub degrees_of_freedom: f64,
    /// The TWO-SIDED p-value in `[0, 1]`. `1.0` for identical/degenerate samples
    /// (no evidence of a difference); small for well-separated cohorts.
    pub p_value: f64,
    /// `false` when the samples were too small or too degenerate (both
    /// variances zero) to run a real test; the p-value is then the conservative
    /// `1.0` and the statistic `0.0`. `true` for a genuine test result.
    pub well_defined: bool,
}

impl TTestResult {
    /// The conservative degenerate result: no evidence of a difference. Used when
    /// a sample has `n < 2` or both variances are zero, so a real test cannot be
    /// formed without dividing by zero.
    fn degenerate(kind: TTestKind) -> Self {
        Self {
            kind,
            t_statistic: 0.0,
            degrees_of_freedom: 0.0,
            p_value: 1.0,
            well_defined: false,
        }
    }
}

/// Cohen's `d` effect size between samples `a` and `b` (C4 #21): the difference
/// in means in units of the POOLED standard deviation,
///
/// ```text
/// d = (mean(a) - mean(b)) / s_pooled,
/// s_pooled = sqrt( ((n_a - 1) s_a^2 + (n_b - 1) s_b^2) / (n_a + n_b - 2) )
/// ```
///
/// A positive `d` means sample `a` drifted MORE than sample `b` on average. The
/// magnitude is conventionally read as ~0.2 small, ~0.5 medium, ~0.8 large.
///
/// Degenerate samples are guarded WITHOUT panicking: if either sample has
/// `n < 2`, or the pooled standard deviation is zero (both samples have zero
/// variance, e.g. every value identical), the effect size is reported as `0.0`
/// rather than dividing by zero to `NaN`/`inf`.
pub fn cohens_d(a: &[f64], b: &[f64]) -> f64 {
    let sa = SampleStats::of(a);
    let sb = SampleStats::of(b);
    cohens_d_from_stats(&sa, &sb)
}

/// Cohen's `d` from pre-computed [`SampleStats`] (so callers that already have
/// the summaries do not recompute them). Same degenerate-sample guards as
/// [`cohens_d`].
pub fn cohens_d_from_stats(a: &SampleStats, b: &SampleStats) -> f64 {
    if a.n < 2 || b.n < 2 {
        return 0.0;
    }
    let df = a.n as f64 + b.n as f64 - 2.0;
    if df <= 0.0 {
        return 0.0;
    }
    let pooled_var = ((a.n as f64 - 1.0) * a.variance + (b.n as f64 - 1.0) * b.variance) / df;
    let s_pooled = pooled_var.sqrt();
    if !s_pooled.is_finite() || s_pooled <= 0.0 {
        // Both samples have zero variance: the means may differ but there is no
        // spread to standardize by. Report 0 rather than an infinity.
        return 0.0;
    }
    (a.mean - b.mean) / s_pooled
}

/// Run a two-sample t-test between `a` and `b` (C4 #21), returning the statistic,
/// degrees of freedom, and the two-sided p-value from the Student-t CDF.
///
/// Guards every degenerate case WITHOUT panicking:
///
/// - Either sample with `n < 2`: variance is undefined; returns the conservative
///   degenerate result (`p = 1`, `well_defined = false`).
/// - Both sample variances zero (no spread in either cohort): the denominator
///   would be zero; returns the degenerate result. (If the means also differ,
///   there is still no statistical spread to test against.)
/// - Otherwise computes the requested [`TTestKind`] statistic and degrees of
///   freedom, and reads the two-sided p-value off the Student-t CDF.
///
/// The p-value is `2 * (1 - CDF(|t|; df))`, clamped to `[0, 1]`.
pub fn two_sample_t_test(a: &[f64], b: &[f64], kind: TTestKind) -> TTestResult {
    let sa = SampleStats::of(a);
    let sb = SampleStats::of(b);
    two_sample_t_test_from_stats(&sa, &sb, kind)
}

/// Run a two-sample t-test from pre-computed [`SampleStats`]. Same guards as
/// [`two_sample_t_test`].
pub fn two_sample_t_test_from_stats(
    a: &SampleStats,
    b: &SampleStats,
    kind: TTestKind,
) -> TTestResult {
    if a.n < 2 || b.n < 2 {
        return TTestResult::degenerate(kind);
    }
    let na = a.n as f64;
    let nb = b.n as f64;

    let (t, df) = match kind {
        TTestKind::Welch => {
            // Welch: standard error from each variance separately.
            let se2 = a.variance / na + b.variance / nb;
            if !se2.is_finite() || se2 <= 0.0 {
                return TTestResult::degenerate(kind);
            }
            let se = se2.sqrt();
            let t = (a.mean - b.mean) / se;
            // Welch-Satterthwaite degrees of freedom.
            let va = a.variance / na;
            let vb = b.variance / nb;
            let denom = va * va / (na - 1.0) + vb * vb / (nb - 1.0);
            let df = if denom > 0.0 {
                (se2 * se2) / denom
            } else {
                // Both variances zero already returns above; defensive fallback.
                na + nb - 2.0
            };
            (t, df)
        }
        TTestKind::Pooled => {
            let df = na + nb - 2.0;
            if df <= 0.0 {
                return TTestResult::degenerate(kind);
            }
            let pooled_var = ((na - 1.0) * a.variance + (nb - 1.0) * b.variance) / df;
            let se2 = pooled_var * (1.0 / na + 1.0 / nb);
            if !se2.is_finite() || se2 <= 0.0 {
                return TTestResult::degenerate(kind);
            }
            let t = (a.mean - b.mean) / se2.sqrt();
            (t, df)
        }
    };

    if !t.is_finite() || !df.is_finite() || df <= 0.0 {
        return TTestResult::degenerate(kind);
    }

    // Student-t CDF for the two-sided p-value. `StudentsT::new` needs a positive
    // scale and freedom; on the off chance the constructor rejects the inputs we
    // fall back to the conservative degenerate result rather than panicking.
    let p_value = match StudentsT::new(0.0, 1.0, df) {
        Ok(dist) => {
            // Two-sided: 2 * P(T >= |t|) = 2 * (1 - CDF(|t|)).
            let two_sided = 2.0 * (1.0 - dist.cdf(t.abs()));
            two_sided.clamp(0.0, 1.0)
        }
        Err(_) => {
            return TTestResult::degenerate(kind);
        }
    };

    TTestResult {
        kind,
        t_statistic: t,
        degrees_of_freedom: df,
        p_value,
        well_defined: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_stats_basic() {
        let s = SampleStats::of(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]);
        assert_eq!(s.n, 8);
        assert!((s.mean - 5.0).abs() < 1e-12);
        // Unbiased variance of this classic example is 32/7 ~= 4.5714286.
        assert!(
            (s.variance - (32.0 / 7.0)).abs() < 1e-9,
            "got {}",
            s.variance
        );
    }

    #[test]
    fn sample_stats_ignores_non_finite_and_guards_tiny() {
        let s = SampleStats::of(&[f64::NAN, 3.0, f64::INFINITY]);
        // Only the finite 3.0 survives; variance undefined for n=1 -> 0.
        assert_eq!(s.n, 1);
        assert_eq!(s.mean, 3.0);
        assert_eq!(s.variance, 0.0);

        let empty = SampleStats::of(&[]);
        assert_eq!(empty.n, 0);
        assert_eq!(empty.mean, 0.0);
        assert_eq!(empty.variance, 0.0);
    }

    #[test]
    fn cohens_d_matches_hand_example() {
        // a = [1,2,3,4,5]: mean 3, var 2.5. b = [3,4,5,6,7]: mean 5, var 2.5.
        // pooled var = 2.5, s_pooled = sqrt(2.5) = 1.5811388...
        // d = (3 - 5) / 1.5811388 = -1.264911...
        let a = [1.0, 2.0, 3.0, 4.0, 5.0];
        let b = [3.0, 4.0, 5.0, 6.0, 7.0];
        let d = cohens_d(&a, &b);
        let expected = -2.0 / (2.5_f64).sqrt();
        assert!((d - expected).abs() < 1e-12, "expected {expected}, got {d}");
    }

    #[test]
    fn cohens_d_guards_degenerate_samples() {
        // n < 2 on one side.
        assert_eq!(cohens_d(&[1.0], &[1.0, 2.0, 3.0]), 0.0);
        // Zero variance on both sides (every value identical): no spread.
        assert_eq!(cohens_d(&[5.0, 5.0, 5.0], &[7.0, 7.0, 7.0]), 0.0);
        // Empty.
        assert_eq!(cohens_d(&[], &[]), 0.0);
    }

    #[test]
    fn t_test_identical_cohorts_p_near_one() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0];
        let b = a;
        let res = two_sample_t_test(&a, &b, TTestKind::Welch);
        assert!(res.well_defined);
        assert!(res.t_statistic.abs() < 1e-12);
        assert!(
            (res.p_value - 1.0).abs() < 1e-9,
            "identical cohorts -> p ~= 1, got {}",
            res.p_value
        );
    }

    #[test]
    fn t_test_well_separated_cohorts_small_p() {
        // Two tight, far-apart clusters: a strongly significant difference.
        let a = [10.0, 10.1, 9.9, 10.05, 9.95, 10.0];
        let b = [1.0, 1.1, 0.9, 1.05, 0.95, 1.0];
        let res = two_sample_t_test(&a, &b, TTestKind::Welch);
        assert!(res.well_defined);
        assert!(res.t_statistic.abs() > 5.0, "t = {}", res.t_statistic);
        assert!(
            res.p_value < 0.001,
            "well-separated cohorts -> tiny p, got {}",
            res.p_value
        );
    }

    #[test]
    fn t_test_p_value_in_unit_interval() {
        let a = [1.0, 2.0, 3.0, 4.0];
        let b = [2.0, 3.0, 4.0, 5.0];
        for kind in [TTestKind::Welch, TTestKind::Pooled] {
            let res = two_sample_t_test(&a, &b, kind);
            assert!(res.well_defined);
            assert!(
                (0.0..=1.0).contains(&res.p_value),
                "p out of [0,1]: {}",
                res.p_value
            );
            assert!(res.degrees_of_freedom > 0.0);
        }
    }

    #[test]
    fn t_test_guards_degenerate_samples() {
        // n < 2.
        let res = two_sample_t_test(&[1.0], &[2.0, 3.0], TTestKind::Welch);
        assert!(!res.well_defined);
        assert_eq!(res.p_value, 1.0);
        assert_eq!(res.t_statistic, 0.0);

        // Both zero-variance: no spread, denominator would be zero.
        let res2 = two_sample_t_test(&[5.0, 5.0, 5.0], &[9.0, 9.0, 9.0], TTestKind::Welch);
        assert!(!res2.well_defined);
        assert_eq!(res2.p_value, 1.0);

        // Pooled with both zero-variance likewise degenerate.
        let res3 = two_sample_t_test(&[5.0, 5.0], &[9.0, 9.0], TTestKind::Pooled);
        assert!(!res3.well_defined);
    }

    #[test]
    fn pooled_and_welch_agree_on_equal_n_equal_var() {
        // With equal sizes AND equal variances, Welch and pooled give the same
        // statistic (their formulas coincide there).
        let a = [2.0, 4.0, 6.0, 8.0];
        let b = [3.0, 5.0, 7.0, 9.0];
        let w = two_sample_t_test(&a, &b, TTestKind::Welch);
        let p = two_sample_t_test(&a, &b, TTestKind::Pooled);
        assert!((w.t_statistic - p.t_statistic).abs() < 1e-9);
    }
}
