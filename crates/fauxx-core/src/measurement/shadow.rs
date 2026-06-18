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

//! Shadow profiles and the control-profile A/B comparison (C4 #21, A2).
//!
//! A [`ShadowProfile`] is one experimental arm: a persona plus an [`Arm`] tag
//! marking it as TREATED (noised) or an untreated CONTROL. Multiple profiles run
//! independently, each with its OWN persona and its own drift metric tracked
//! separately. Profile definitions persist in the new `shadow_profiles` table
//! (see `store/schema.rs`), keyed by id, and round-trip as their exact JSON.
//!
//! The comparison reuses the A1 KL/drift metric (each profile contributes a
//! SAMPLE of drift values, one per snapshot), so the A/B numbers are directly
//! comparable to the dashboard. [`compare_cohorts`] computes, across the treated
//! and control cohorts:
//!
//! - an EFFECT SIZE (Cohen's `d` on the drift metric), and
//! - a SIGNIFICANCE measure (a two-sample t-test p-value),
//!
//! then renders a [`CohortComparison`] whose human-facing fields a
//! non-statistician can read: which arm drifted more, by how much, and how
//! confident we are.

use serde::{Deserialize, Serialize};

use crate::measurement::stats::{
    cohens_d_from_stats, two_sample_t_test_from_stats, SampleStats, TTestKind, TTestResult,
};
use crate::persona::SyntheticPersona;

/// Which experimental arm a shadow profile belongs to (C4 #21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    /// A TREATED profile: noise/decoy traffic is applied to it.
    Treated,
    /// An untreated CONTROL profile: the do-nothing baseline to compare against.
    Control,
}

impl Arm {
    /// The stored/display string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Arm::Treated => "treated",
            Arm::Control => "control",
        }
    }
}

/// A persisted shadow-profile definition (C4 #21): one experimental arm with its
/// own persona. Round-trips through the encrypted store as its exact JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowProfile {
    /// Stable id (UUID v4 string).
    pub id: String,
    /// Human-readable label (e.g. "Treated A", "Do-nothing control").
    pub label: String,
    /// Whether this profile is treated (noised) or an untreated control.
    pub arm: Arm,
    /// The persona id this profile drives (its own independent persona).
    pub persona_id: String,
    /// Epoch milliseconds when the profile was defined.
    pub created_at: i64,
}

impl ShadowProfile {
    /// Build a shadow-profile definition.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        arm: Arm,
        persona_id: impl Into<String>,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            arm,
            persona_id: persona_id.into(),
            created_at,
        }
    }

    /// Convenience: a treated profile for `persona`.
    pub fn treated(
        id: impl Into<String>,
        label: impl Into<String>,
        persona: &SyntheticPersona,
        created_at: i64,
    ) -> Self {
        Self::new(id, label, Arm::Treated, persona.id.clone(), created_at)
    }

    /// Convenience: an untreated control profile for `persona`.
    pub fn control(
        id: impl Into<String>,
        label: impl Into<String>,
        persona: &SyntheticPersona,
        created_at: i64,
    ) -> Self {
        Self::new(id, label, Arm::Control, persona.id.clone(), created_at)
    }
}

/// The plainly-readable result of a treated-vs-control comparison (C4 #21).
///
/// Carries both the raw numbers (for the chart) AND human-facing summary fields
/// so a non-statistician can read the result: which arm drifted more, the size
/// of the difference in plain words, and how confident we are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CohortComparison {
    /// Summary statistics of the TREATED cohort's drift sample.
    pub treated: SampleStats,
    /// Summary statistics of the CONTROL cohort's drift sample.
    pub control: SampleStats,
    /// Cohen's `d` effect size, treated minus control. Positive means the
    /// treated arm drifted MORE on average. `0.0` when undefined/degenerate.
    pub effect_size: f64,
    /// The significance test (statistic, df, two-sided p-value, well-defined).
    pub significance: TTestResult,
    /// A one-line, non-statistician-readable verdict, e.g. "The treated profile
    /// drifted noticeably more than the control (large effect), and the
    /// difference is statistically significant (p = 0.012)."
    pub summary: String,
    /// The plain-words effect magnitude bucket: "negligible" / "small" /
    /// "medium" / "large".
    pub effect_magnitude: String,
    /// The plain-words direction: which arm drifted more.
    pub direction: String,
    /// The plain-words confidence statement derived from the p-value.
    pub confidence: String,
}

/// Compare a TREATED cohort's drift sample against a CONTROL cohort's drift
/// sample (C4 #21), computing the effect size, the significance, and a
/// plainly-readable summary.
///
/// Inputs are samples of the SAME A1 drift metric (one value per snapshot), so
/// the comparison is in the dashboard's own units. The `kind` selects Welch
/// (default, unequal-variance) or pooled significance.
///
/// Degenerate samples are guarded by the underlying [`cohens_d_from_stats`] and
/// [`two_sample_t_test_from_stats`] (n < 2, zero variance): they never panic;
/// the effect size falls back to `0.0` and the significance to the conservative
/// `p = 1` "no evidence" result, and the summary says so plainly.
pub fn compare_cohorts(
    treated_drift: &[f64],
    control_drift: &[f64],
    kind: TTestKind,
) -> CohortComparison {
    let treated = SampleStats::of(treated_drift);
    let control = SampleStats::of(control_drift);
    let effect_size = cohens_d_from_stats(&treated, &control);
    let significance = two_sample_t_test_from_stats(&treated, &control, kind);

    let effect_magnitude = magnitude_label(effect_size).to_string();
    let direction = direction_label(&treated, &control).to_string();
    let confidence = confidence_label(&significance).to_string();
    let summary = build_summary(
        &treated,
        &control,
        effect_size,
        &significance,
        &effect_magnitude,
        &direction,
        &confidence,
    );

    CohortComparison {
        treated,
        control,
        effect_size,
        significance,
        summary,
        effect_magnitude,
        direction,
        confidence,
    }
}

/// The conventional Cohen's `d` magnitude bucket, by absolute value.
fn magnitude_label(d: f64) -> &'static str {
    let a = d.abs();
    if a < 0.2 {
        "negligible"
    } else if a < 0.5 {
        "small"
    } else if a < 0.8 {
        "medium"
    } else {
        "large"
    }
}

/// Which arm drifted more, in plain words.
fn direction_label(treated: &SampleStats, control: &SampleStats) -> &'static str {
    // Within a tiny tolerance the arms are "about the same".
    let diff = treated.mean - control.mean;
    if diff.abs() < 1e-12 {
        "about the same drift in both profiles"
    } else if diff > 0.0 {
        "the treated profile drifted more than the control"
    } else {
        "the control profile drifted more than the treated profile"
    }
}

/// A plain-words confidence statement from the significance result.
fn confidence_label(sig: &TTestResult) -> &'static str {
    if !sig.well_defined {
        return "not enough data to judge significance";
    }
    let p = sig.p_value;
    if p < 0.01 {
        "the difference is statistically significant (strong evidence)"
    } else if p < 0.05 {
        "the difference is statistically significant"
    } else if p < 0.1 {
        "the difference is borderline; more data would help"
    } else {
        "the difference is not statistically significant"
    }
}

/// Assemble the one-line human summary from the parts.
fn build_summary(
    treated: &SampleStats,
    control: &SampleStats,
    effect_size: f64,
    sig: &TTestResult,
    magnitude: &str,
    direction: &str,
    confidence: &str,
) -> String {
    if treated.n < 2 || control.n < 2 {
        return format!(
            "Not enough data to compare yet: treated has {} drift reading(s) and \
             control has {} (need at least 2 each).",
            treated.n, control.n
        );
    }
    if sig.well_defined {
        format!(
            "On the drift metric, {direction} ({magnitude} effect, Cohen's d = {:.2}); \
             {confidence} (p = {:.3}).",
            effect_size, sig.p_value
        )
    } else {
        format!(
            "On the drift metric, {direction} ({magnitude} effect, Cohen's d = {:.2}); \
             {confidence}.",
            effect_size
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn persona(id: &str) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            "Shadow".to_string(),
            AgeRange::AGE_25_34.as_name().to_string(),
            Profession::STUDENT.as_name().to_string(),
            Region::UK.as_name().to_string(),
            vec![
                CategoryPool::MUSIC.as_name().to_string(),
                CategoryPool::GAMING.as_name().to_string(),
                CategoryPool::TECHNOLOGY.as_name().to_string(),
            ],
            10,
            20,
        )
    }

    #[test]
    fn profile_constructors_set_arm() {
        let p = persona("abc");
        let t = ShadowProfile::treated("t1", "Treated", &p, 100);
        assert_eq!(t.arm, Arm::Treated);
        assert_eq!(t.persona_id, "abc");
        let c = ShadowProfile::control("c1", "Control", &p, 100);
        assert_eq!(c.arm, Arm::Control);
        assert_eq!(Arm::Treated.as_str(), "treated");
        assert_eq!(Arm::Control.as_str(), "control");
    }

    #[test]
    fn profile_json_round_trips() -> crate::Result<()> {
        let p = persona("abc");
        let profile = ShadowProfile::treated("t1", "Treated A", &p, 100);
        let json = serde_json::to_string(&profile)?;
        let back: ShadowProfile = serde_json::from_str(&json)?;
        assert_eq!(back, profile);
        // The arm serializes lowercase.
        assert!(json.contains("\"treated\""));
        Ok(())
    }

    #[test]
    fn comparison_well_separated_is_significant() {
        // Treated drifts a lot, control barely moves: large effect, significant.
        let treated = [0.8, 0.85, 0.9, 0.82, 0.88, 0.86];
        let control = [0.05, 0.06, 0.04, 0.055, 0.045, 0.05];
        let cmp = compare_cohorts(&treated, &control, TTestKind::Welch);
        assert!(cmp.significance.well_defined);
        assert!(
            cmp.effect_size > 0.8,
            "expected large effect, got {}",
            cmp.effect_size
        );
        assert_eq!(cmp.effect_magnitude, "large");
        assert!(cmp.direction.contains("treated profile drifted more"));
        assert!(
            cmp.significance.p_value < 0.05,
            "p = {}",
            cmp.significance.p_value
        );
        assert!(cmp.summary.contains("Cohen's d"));
    }

    #[test]
    fn comparison_identical_is_not_significant() {
        let treated = [0.5, 0.5, 0.5, 0.5];
        let control = [0.5, 0.5, 0.5, 0.5];
        let cmp = compare_cohorts(&treated, &control, TTestKind::Welch);
        // Both zero-variance: guarded, not significant, effect 0.
        assert_eq!(cmp.effect_size, 0.0);
        assert!(!cmp.significance.well_defined);
        assert!(cmp.confidence.contains("not enough data to judge"));
    }

    #[test]
    fn comparison_guards_tiny_samples() {
        let cmp = compare_cohorts(&[0.5], &[0.1, 0.2, 0.3], TTestKind::Welch);
        assert_eq!(cmp.effect_size, 0.0);
        assert!(!cmp.significance.well_defined);
        assert!(cmp.summary.contains("Not enough data"));
    }

    #[test]
    fn comparison_empty_cohorts_no_panic() {
        let cmp = compare_cohorts(&[], &[], TTestKind::Welch);
        assert_eq!(cmp.effect_size, 0.0);
        assert!(!cmp.significance.well_defined);
        assert_eq!(cmp.treated.n, 0);
        assert_eq!(cmp.control.n, 0);
    }

    #[test]
    fn magnitude_buckets() {
        assert_eq!(magnitude_label(0.1), "negligible");
        assert_eq!(magnitude_label(0.3), "small");
        assert_eq!(magnitude_label(0.6), "medium");
        assert_eq!(magnitude_label(1.2), "large");
        // Sign does not affect the bucket.
        assert_eq!(magnitude_label(-1.2), "large");
    }
}
