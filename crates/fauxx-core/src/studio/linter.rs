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

//! Persona coherence linter (C5 #25, P2).
//!
//! [`lint_persona`] takes a persona and returns a list of [`Finding`]s WITHOUT
//! mutating it (the function takes `&SyntheticPersona`). A clean, coherent
//! persona returns an empty list.
//!
//! Two tiers of rule:
//!
//! - [`Severity::HardImplausible`]: a combination that essentially cannot exist.
//!   These port the Android `PersonaConsistencyRules` hard incompatible-trait
//!   pairs (seeded by the existing [`SyntheticPersona::validate`]
//!   completeness/known-value checks), e.g. AGE_65_PLUS + ACADEMIC with fewer
//!   than three interests, AGE_18_24 + RETIREMENT without FINANCE or REAL_ESTATE,
//!   PARENTING + AGE_18_24 with a single interest.
//! - [`Severity::Warning`]: a combination that is unlikely but not impossible,
//!   driven by the bundled category-affinity prior (`category_cooccurrence.json`,
//!   VENDORED from the phone's `ad_category_cooccurrence.json` via
//!   `scripts/vendor-cooccurrence.py`): a persona-interest PAIR whose affinity is
//!   BELOW `min_affinity` (an uncommon combination the population sampler would
//!   rarely produce) is flagged.
//!
//! ## Recompute on the P1 change event
//!
//! Linting is a pure function of a persona, so a subscriber drives it: take a
//! [`crate::studio::PersonaChanged`] event off the
//! [`Core::subscribe_persona_changes`](crate::Core::subscribe_persona_changes)
//! broadcast, load the changed persona
//! ([`Core::get_persona`](crate::Core::get_persona)), call
//! [`Core::lint_persona`](crate::Core::lint_persona), and re-render. The GUI
//! subscription itself is a later batch; this module is the headless computation
//! it will call.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::persona::{
    AgeRange, CategoryPool, PersonaIssue, Profession, Region, SyntheticPersona, INTEREST_COUNT,
};

/// The bundled category-AFFINITY prior, embedded at compile time. VENDORED from
/// the phone's `ad_category_cooccurrence.json` (single source of truth; re-sync
/// with `scripts/vendor-cooccurrence.py`).
const COOCCURRENCE_JSON: &str = include_str!("category_cooccurrence.json");

/// How severe a [`Finding`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// Unlikely but not impossible: surfaced so an author notices, not blocked.
    Warning,
    /// Essentially cannot exist: an incoherent profile a real person would not
    /// hold.
    HardImplausible,
}

/// One problem the linter found in a persona. Carries the [`Severity`], a
/// human-readable reason, and the affected field identifier(s) (the persona's
/// own field vocabulary, e.g. `"ageRange"`, `"interests"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// How severe this finding is.
    pub severity: Severity,
    /// A human-readable explanation suitable for showing to the author.
    pub reason: String,
    /// The persona field identifier(s) this finding concerns, in a stable order.
    pub fields: Vec<String>,
}

impl Finding {
    /// Build a [`Severity::HardImplausible`] finding.
    fn hard(reason: impl Into<String>, fields: &[&str]) -> Self {
        Self {
            severity: Severity::HardImplausible,
            reason: reason.into(),
            fields: fields.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// Build a [`Severity::Warning`] finding.
    fn warning(reason: impl Into<String>, fields: &[&str]) -> Self {
        Self {
            severity: Severity::Warning,
            reason: reason.into(),
            fields: fields.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// Whether this finding is hard-implausible.
    pub fn is_hard(&self) -> bool {
        self.severity == Severity::HardImplausible
    }
}

/// One affinity-table entry: an unordered category pair and its affinity `w`
/// (in `[0, 1]`; higher = the two interests cluster together more).
#[derive(Debug, Clone, Deserialize)]
struct CooccurrencePair {
    a: String,
    b: String,
    w: f64,
}

/// The parsed category-affinity prior.
#[derive(Debug, Clone, Deserialize)]
struct CooccurrenceTable {
    /// A persona-interest pair with affinity BELOW this is an uncommon
    /// combination and is warned (absent from the table = affinity 0).
    min_affinity: f64,
    affinities: Vec<CooccurrencePair>,
}

/// An unordered category-pair key (alphabetical so `(a, b)` and `(b, a)` map to
/// the same key).
type PairKey = (String, String);

/// The parsed affinity prior: the `min_affinity` threshold plus a map from each
/// pair key to its affinity weight.
type ParsedCooccurrence = (f64, BTreeMap<PairKey, f64>);

/// The unordered key for a category pair (alphabetical so `(a, b)` and `(b, a)`
/// map to the same key).
fn pair_key(a: &str, b: &str) -> PairKey {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Parsed, validated, cached view of the co-occurrence table. Parsed once.
fn cooccurrence() -> &'static ParsedCooccurrence {
    static TABLE: OnceLock<ParsedCooccurrence> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The table is a compile-time-embedded const we author and a unit test
        // validates, so a parse failure here is a build-time authoring bug, not a
        // runtime input. Fall back to a permissive empty table (no warnings)
        // rather than panicking so the no-unwrap rule holds.
        // A parse failure falls back to `min_affinity = 0.0` so NOTHING warns
        // (affinity is never below 0): a permissive empty prior, not a panic.
        let parsed: CooccurrenceTable =
            serde_json::from_str(COOCCURRENCE_JSON).unwrap_or(CooccurrenceTable {
                min_affinity: 0.0,
                affinities: Vec::new(),
            });
        let mut map = BTreeMap::new();
        for p in parsed.affinities {
            map.insert(pair_key(&p.a, &p.b), p.w);
        }
        (parsed.min_affinity, map)
    })
}

/// Lint a persona for coherence, returning every [`Finding`] (empty when clean).
///
/// NON-DESTRUCTIVE: takes the persona by shared reference and never mutates it.
/// Findings are returned hard-implausible first, then warnings, each group in a
/// deterministic order.
pub fn lint_persona(persona: &SyntheticPersona) -> Vec<Finding> {
    let mut hard = Vec::new();
    let mut warnings = Vec::new();

    completeness_findings(persona, &mut hard);
    incompatible_trait_findings(persona, &mut hard);
    cooccurrence_findings(persona, &mut warnings);

    let mut out = hard;
    out.extend(warnings);
    out
}

/// Required-field completeness and known-value checks. Seeds off the existing
/// [`SyntheticPersona::validate`] [`PersonaIssue`] checks, mapping each to a
/// [`Severity::HardImplausible`] finding (an unknown enum value or an out-of-band
/// interest count is not a real, coherent persona). Also flags an empty name.
fn completeness_findings(persona: &SyntheticPersona, out: &mut Vec<Finding>) {
    if persona.name.trim().is_empty() {
        out.push(Finding::hard("persona has no name", &["name"]));
    }
    for issue in persona.validate() {
        match issue {
            PersonaIssue::UnknownAgeRange(v) => out.push(Finding::hard(
                format!("ageRange {v:?} is not a known age bracket"),
                &["ageRange"],
            )),
            PersonaIssue::UnknownProfession(v) => out.push(Finding::hard(
                format!("profession {v:?} is not a known profession"),
                &["profession"],
            )),
            PersonaIssue::UnknownRegion(v) => out.push(Finding::hard(
                format!("region {v:?} is not a known region"),
                &["region"],
            )),
            PersonaIssue::UnknownInterest(v) => out.push(Finding::hard(
                format!("interest {v:?} is not a known category"),
                &["interests"],
            )),
            PersonaIssue::InterestCount(n) => out.push(Finding::hard(
                format!(
                    "a persona must carry {}-{} interests, found {n}",
                    INTEREST_COUNT.start(),
                    INTEREST_COUNT.end()
                ),
                &["interests"],
            )),
        }
    }
}

/// The hard incompatible-trait pairs ported from the Android
/// `PersonaConsistencyRules`. Each maps to a [`Severity::HardImplausible`]
/// finding.
fn incompatible_trait_findings(persona: &SyntheticPersona, out: &mut Vec<Finding>) {
    let age = AgeRange::from_name(&persona.age_range);
    let has = |c: CategoryPool| persona.interests.iter().any(|i| i == c.as_name());
    let interest_count = persona.interests.len();

    // AGE_65_PLUS + ACADEMIC with fewer than 3 interests: a retiree-aged
    // academic with a near-empty interest set is implausibly thin.
    if age == Some(AgeRange::AGE_65_PLUS) && has(CategoryPool::ACADEMIC) && interest_count < 3 {
        out.push(Finding::hard(
            "AGE_65_PLUS with ACADEMIC must carry at least 3 interests to be plausible",
            &["ageRange", "interests"],
        ));
    }

    // AGE_18_24 + RETIREMENT without FINANCE or REAL_ESTATE: a young adult with a
    // RETIREMENT interest is only plausible alongside a finance/real-estate angle.
    if age == Some(AgeRange::AGE_18_24)
        && has(CategoryPool::RETIREMENT)
        && !(has(CategoryPool::FINANCE) || has(CategoryPool::REAL_ESTATE))
    {
        out.push(Finding::hard(
            "AGE_18_24 with RETIREMENT needs FINANCE or REAL_ESTATE to be plausible",
            &["ageRange", "interests"],
        ));
    }

    // PARENTING + AGE_18_24 with a single interest: a young parent profile with
    // only one interest is implausibly thin.
    if age == Some(AgeRange::AGE_18_24) && has(CategoryPool::PARENTING) && interest_count <= 1 {
        out.push(Finding::hard(
            "AGE_18_24 with PARENTING must carry more than one interest to be plausible",
            &["ageRange", "interests"],
        ));
    }

    // Belt-and-braces consistency the Android rules also encode: a retiree
    // profession on a young adult is incoherent.
    if age == Some(AgeRange::AGE_18_24)
        && Profession::from_name(&persona.profession) == Some(Profession::RETIRED)
    {
        out.push(Finding::hard(
            "AGE_18_24 with the RETIRED profession is incompatible",
            &["ageRange", "profession"],
        ));
    }

    // A region we cannot interpret cannot anchor a coherent profile; this is
    // already covered by completeness, so it is not duplicated here. The
    // `Region` import is used by the cooccurrence pathway's documentation only;
    // reference it to keep the trait checks self-contained.
    let _ = Region::from_name(&persona.region);
}

/// Co-occurrence warnings from the bundled real-distribution table. Each persona
/// interest PAIR whose stored co-occurrence rate is at or below the table's
/// `warn_at_or_below` threshold becomes one [`Severity::Warning`] finding.
fn cooccurrence_findings(persona: &SyntheticPersona, out: &mut Vec<Finding>) {
    let (min_affinity, table) = cooccurrence();
    // Consider only known categories; an unknown interest is already a hard
    // completeness finding and cannot key the affinity prior.
    let known: Vec<&String> = persona
        .interests
        .iter()
        .filter(|i| CategoryPool::from_name(i).is_some())
        .collect();
    for i in 0..known.len() {
        for j in (i + 1)..known.len() {
            let key = pair_key(known[i], known[j]);
            // Absent from the prior = no semantic affinity (0). A pair below the
            // threshold is an uncommon combination the population sampler would
            // rarely produce; flag it as a non-fatal Warning.
            let affinity = table.get(&key).copied().unwrap_or(0.0);
            if affinity < *min_affinity {
                out.push(Finding::warning(
                    format!(
                        "interests {} and {} are an uncommon combination (low population affinity)",
                        key.0, key.1
                    ),
                    &["interests"],
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn persona(
        age: AgeRange,
        profession: Profession,
        interests: &[CategoryPool],
    ) -> SyntheticPersona {
        SyntheticPersona::new(
            "lint-test-0000-4000-8000-000000000000".to_string(),
            "Lint Test".to_string(),
            age.as_name().to_string(),
            profession.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    fn coherent() -> SyntheticPersona {
        // A mutually-affiliated interest triple in the vendored prior (every pair
        // is listed), so a clean persona draws no co-occurrence warning.
        persona(
            AgeRange::AGE_35_44,
            Profession::ENGINEER,
            &[
                CategoryPool::ACADEMIC,
                CategoryPool::ENVIRONMENT,
                CategoryPool::HISTORY,
            ],
        )
    }

    #[test]
    fn coocurrence_table_parses_and_keys_are_known_categories() {
        let (threshold, table) = cooccurrence();
        assert!(*threshold > 0.0);
        assert!(!table.is_empty());
        for (a, b) in table.keys() {
            assert!(
                CategoryPool::from_name(a).is_some(),
                "co-occurrence key {a} is not a known category"
            );
            assert!(
                CategoryPool::from_name(b).is_some(),
                "co-occurrence key {b} is not a known category"
            );
        }
    }

    #[test]
    fn clean_persona_returns_no_findings() {
        assert!(lint_persona(&coherent()).is_empty());
    }

    #[test]
    fn unaffiliated_interest_pair_is_flagged_as_advisory() {
        // TECHNOLOGY+SCIENCE are affiliated in the prior, but TRAVEL has no
        // affinity to either: the uncommon combination is flagged as a non-fatal
        // Warning (the #25 P2 rare-co-occurrence advisory), not a hard finding.
        let p = persona(
            AgeRange::AGE_35_44,
            Profession::ENGINEER,
            &[
                CategoryPool::TECHNOLOGY,
                CategoryPool::SCIENCE,
                CategoryPool::TRAVEL,
            ],
        );
        let findings = lint_persona(&p);
        assert!(
            findings.iter().any(|f| !f.is_hard()
                && f.reason.contains("uncommon combination")
                && f.reason.contains("TRAVEL")),
            "an unaffiliated interest pair must warn"
        );
        assert!(
            findings.iter().all(|f| !f.is_hard()),
            "this persona has no hard-implausible findings, only advisories"
        );
    }

    #[test]
    fn lint_never_mutates_the_persona() {
        let p = coherent();
        let before = p.clone();
        let _ = lint_persona(&p);
        assert_eq!(p, before);
    }

    #[test]
    fn flags_age_65_plus_academic_with_too_few_interests() {
        // AGE_65_PLUS + ACADEMIC with only 2 interests: hard implausible. (Two
        // interests also trips the completeness rule; we assert the trait rule is
        // present specifically.)
        let p = persona(
            AgeRange::AGE_65_PLUS,
            Profession::RETIRED,
            &[CategoryPool::ACADEMIC, CategoryPool::HISTORY],
        );
        let findings = lint_persona(&p);
        assert!(findings
            .iter()
            .any(|f| f.is_hard() && f.reason.contains("AGE_65_PLUS")));
    }

    #[test]
    fn flags_age_18_24_retirement_without_finance_or_real_estate() {
        let p = persona(
            AgeRange::AGE_18_24,
            Profession::STUDENT,
            &[
                CategoryPool::RETIREMENT,
                CategoryPool::GAMING,
                CategoryPool::MUSIC,
            ],
        );
        let findings = lint_persona(&p);
        assert!(findings
            .iter()
            .any(|f| f.is_hard() && f.reason.contains("RETIREMENT")));
        // Adding FINANCE clears that specific hard finding.
        let ok = persona(
            AgeRange::AGE_18_24,
            Profession::STUDENT,
            &[
                CategoryPool::RETIREMENT,
                CategoryPool::FINANCE,
                CategoryPool::MUSIC,
            ],
        );
        assert!(!lint_persona(&ok)
            .iter()
            .any(|f| f.is_hard() && f.reason.contains("RETIREMENT")));
    }

    #[test]
    fn flags_parenting_age_18_24_with_single_interest() {
        // A single-interest persona also trips completeness (needs 3..=5), but
        // the PARENTING + AGE_18_24 trait rule must specifically fire.
        let p = persona(
            AgeRange::AGE_18_24,
            Profession::HOMEMAKER,
            &[CategoryPool::PARENTING],
        );
        let findings = lint_persona(&p);
        assert!(findings
            .iter()
            .any(|f| f.is_hard() && f.reason.contains("PARENTING")));
    }

    #[test]
    fn flags_never_cooccur_interest_pair_as_warning() {
        // RETIREMENT + GAMING is in the bundled table below the threshold. Keep
        // the persona otherwise coherent (AGE_45_54 so no age trait rule fires)
        // and pad to a valid interest count so the only finding is the warning.
        let p = persona(
            AgeRange::AGE_45_54,
            Profession::OTHER,
            &[
                CategoryPool::RETIREMENT,
                CategoryPool::GAMING,
                CategoryPool::TECHNOLOGY,
            ],
        );
        let findings = lint_persona(&p);
        assert!(findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.fields == vec!["interests".to_string()]));
        // No hard findings for this otherwise-coherent persona.
        assert!(!findings.iter().any(Finding::is_hard));
    }

    #[test]
    fn unknown_enum_values_are_hard_completeness_findings() {
        let mut p = coherent();
        p.age_range = "AGE_999".to_string();
        let findings = lint_persona(&p);
        assert!(findings
            .iter()
            .any(|f| f.is_hard() && f.fields.contains(&"ageRange".to_string())));
    }

    #[test]
    fn hard_findings_sort_before_warnings() {
        // A persona that trips both a hard trait rule and a co-occurrence warning
        // returns the hard finding(s) first.
        let p = persona(
            AgeRange::AGE_18_24,
            Profession::STUDENT,
            &[
                CategoryPool::RETIREMENT,
                CategoryPool::GAMING,
                CategoryPool::MUSIC,
            ],
        );
        let findings = lint_persona(&p);
        let first_warning = findings
            .iter()
            .position(|f| f.severity == Severity::Warning);
        let last_hard = findings.iter().rposition(Finding::is_hard);
        if let (Some(fw), Some(lh)) = (first_warning, last_hard) {
            assert!(lh < fw, "all hard findings must precede warnings");
        }
    }
}
