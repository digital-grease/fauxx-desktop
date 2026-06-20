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

//! Category-targetable decoy history generation (C2 #12, R2).
//!
//! A curated, BUNDLED mapping from each frozen [`CategoryPool`] interest category
//! to a small set of representative, reputable HTTPS sites. Given a persona's (or
//! a campaign's) category selection, the decoy browser visits those sites with
//! the persona's paced cadence, building real history in the isolated decoy
//! profile so the Privacy Sandbox Topics API can later attribute topics to it.
//!
//! The mapping is embedded at compile time from `category_sites.json` via
//! `include_str!` + `serde`, so it ships in the binary with no runtime file
//! dependency and is validated once (and cached) on first use. The site list is
//! deliberately MODEST and tasteful: a few mainstream, category-representative
//! homepages per category, all plain HTTPS, none requiring auth and none on the
//! R3 sign-in blocklist (a unit test enforces both invariants).
//!
//! All visits go through the guarded
//! [`DecoyPage::navigate`](crate::browser::DecoyPage::navigate), so the R3
//! navigation blocklist applies to every seeded URL, and the persona-paced
//! [`BrowsingCadence`](crate::browser::BrowsingCadence) gives each visit a
//! human-plausible dwell/scroll.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::browser::isolation;
use crate::browser::DecoyBrowser;
use crate::error::{CoreError, Result};
use crate::persona::{CategoryPool, SyntheticPersona};

/// The bundled category->site table, embedded at compile time. Keys are
/// [`CategoryPool`] enum names; values are representative reputable HTTPS sites.
const CATEGORY_SITES_JSON: &str = include_str!("category_sites.json");

/// Parsed, validated, cached view of [`CATEGORY_SITES_JSON`]. Parsed once on
/// first access. A `BTreeMap` keeps iteration order deterministic.
fn site_table() -> &'static BTreeMap<String, Vec<String>> {
    static TABLE: OnceLock<BTreeMap<String, Vec<String>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The table is a compile-time-embedded const we author and a unit test
        // validates (every category present, every URL HTTPS and not on the
        // blocklist), so a parse failure here is a build-time authoring bug, not
        // a runtime input. Fall back to an empty map rather than panicking so
        // the no-unwrap rule holds; `category_sites` then surfaces a typed error.
        serde_json::from_str(CATEGORY_SITES_JSON).unwrap_or_default()
    })
}

/// The representative HTTPS sites for one interest [`CategoryPool`] category.
///
/// Returns [`CoreError::Browser`] if the category somehow has no bundled sites
/// (which the completeness unit test prevents for every frozen category).
pub fn category_sites(category: CategoryPool) -> Result<&'static [String]> {
    let sites = site_table()
        .get(category.as_name())
        .map(Vec::as_slice)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CoreError::Browser(format!(
                "no bundled decoy sites for category {}",
                category.as_name()
            ))
        })?;
    Ok(sites)
}

/// Resolve a persona's interest names into the ordered, de-duplicated list of
/// HTTPS sites to seed.
///
/// Unknown interest strings (a legacy/future phone value not in
/// [`CategoryPool`]) are simply skipped: they cannot be mapped to a curated
/// site set, so they contribute no visits rather than erroring. The result
/// preserves first-seen order and drops duplicate URLs shared across categories.
pub fn sites_for_persona(persona: &SyntheticPersona) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for interest in &persona.interests {
        let Some(category) = CategoryPool::from_name(interest) else {
            continue;
        };
        if let Ok(sites) = category_sites(category) {
            for site in sites {
                if seen.insert(site.clone()) {
                    out.push(site.clone());
                }
            }
        }
    }
    out
}

/// Resolve an explicit category selection (a campaign's targeted categories)
/// into the ordered, de-duplicated list of HTTPS sites to seed.
pub fn sites_for_categories(categories: &[CategoryPool]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for category in categories {
        if let Ok(sites) = category_sites(*category) {
            for site in sites {
                if seen.insert(site.clone()) {
                    out.push(site.clone());
                }
            }
        }
    }
    out
}

/// Outcome of seeding decoy history for one persona/campaign run: which URLs
/// were visited successfully and which were skipped (with the reason), so a
/// headless run is observable and the closed loop can record what happened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedOutcome {
    /// URLs that loaded successfully (guarded navigation + paced browse).
    pub visited: Vec<String>,
    /// URLs that were skipped, each with a short local reason (navigation error
    /// or a guardrail refusal). Skips do not abort the run; the decoy keeps
    /// seeding the rest so a single unreachable site does not lose the session.
    pub skipped: Vec<(String, String)>,
}

impl SeedOutcome {
    /// Number of URLs that loaded successfully.
    pub fn visited_count(&self) -> usize {
        self.visited.len()
    }

    /// Whether at least one site was visited.
    pub fn any_visited(&self) -> bool {
        !self.visited.is_empty()
    }
}

/// Drive `browser` to visit `urls` in order with the persona's paced cadence,
/// building real history in the decoy profile.
///
/// Every URL goes through the guarded
/// [`DecoyPage::navigate`](crate::browser::DecoyPage::navigate) (so the R3
/// navigation blocklist applies) and then a persona-paced
/// [`BrowsingCadence`](crate::browser::BrowsingCadence) browse (scroll + dwell).
/// A URL that fails to load, or is refused by the guardrail, is recorded in
/// [`SeedOutcome::skipped`] and the run continues with the next URL; the whole
/// seed never aborts on one bad site. `seed` mixes into the cadence so a fixed
/// `(persona, seed)` pair is reproducible.
///
/// Each URL is visited on its OWN fresh page so the decoy's history accrues one
/// top-level navigation per site (mirroring real browsing), rather than a single
/// page repeatedly redirected.
pub async fn seed_history_for_persona(
    browser: &DecoyBrowser,
    persona: &SyntheticPersona,
    urls: &[String],
    seed: u64,
) -> Result<SeedOutcome> {
    let mut outcome = SeedOutcome::default();
    for url in urls {
        // Pre-check the guardrail so a blocked URL is a recorded skip rather than
        // a hard error that aborts the whole seed (the navigate call also checks).
        if let Err(e) = isolation::ensure_navigation_allowed(url) {
            outcome.skipped.push((url.clone(), e.to_string()));
            continue;
        }
        let page = match browser.new_page().await {
            Ok(page) => page,
            Err(e) => {
                outcome.skipped.push((url.clone(), e.to_string()));
                continue;
            }
        };
        match page.navigate(url).await {
            Ok(()) => {
                // Persona-paced browse builds plausible engagement on the page.
                // A browse failure (e.g. the tab closed) is non-fatal: the visit
                // still counts, since the navigation itself created history.
                let _ = page.browse_with_persona(persona, seed).await;
                outcome.visited.push(url.clone());
            }
            Err(e) => {
                outcome.skipped.push((url.clone(), e.to_string()));
            }
        }
    }
    tracing::info!(
        target: "fauxx_core::browser::categories",
        persona_id = %persona.id,
        visited = outcome.visited.len(),
        skipped = outcome.skipped.len(),
        "seeded decoy history for persona category interests"
    );
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_is_complete_for_every_frozen_category() -> Result<()> {
        // The embedded JSON must parse and cover EVERY frozen CategoryPool name
        // with a non-empty site list. This is the completeness invariant.
        for category in CategoryPool::all() {
            let sites = category_sites(*category)?;
            assert!(
                !sites.is_empty(),
                "category {} must have at least one bundled site",
                category.as_name()
            );
        }
        // No EXTRA keys the enum does not know about (keeps the table in lockstep
        // with the frozen category set).
        for key in site_table().keys() {
            assert!(
                CategoryPool::from_name(key).is_some(),
                "site table has unknown category key {key}"
            );
        }
        Ok(())
    }

    #[test]
    fn every_site_is_https_and_not_on_the_auth_blocklist() {
        for sites in site_table().values() {
            for url in sites {
                assert!(
                    url.starts_with("https://"),
                    "decoy site must be HTTPS (Topics needs a secure context): {url}"
                );
                assert!(
                    !isolation::is_blocked_auth_flow(url),
                    "decoy site must not be on the R3 auth-flow blocklist: {url}"
                );
            }
        }
    }

    #[test]
    fn no_duplicate_urls_within_a_category() {
        for (cat, sites) in site_table() {
            let mut seen = std::collections::HashSet::new();
            for url in sites {
                assert!(
                    seen.insert(url),
                    "duplicate site {url} within category {cat}"
                );
            }
        }
    }

    fn persona_with(interests: &[CategoryPool]) -> SyntheticPersona {
        use crate::persona::{AgeRange, Profession, Region};
        SyntheticPersona::new(
            "seed-test-0000-4000-8000-000000000000".to_string(),
            "Seed Test".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            interests.iter().map(|c| c.as_name().to_string()).collect(),
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    #[test]
    fn sites_for_persona_maps_interests_and_dedupes() -> Result<()> {
        let persona = persona_with(&[
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::TRAVEL,
        ]);
        let sites = sites_for_persona(&persona);
        assert!(!sites.is_empty());
        // Every persona-derived URL is one of the three categories' sites.
        let mut expected = std::collections::HashSet::new();
        for c in [
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::TRAVEL,
        ] {
            for s in category_sites(c)? {
                expected.insert(s.clone());
            }
        }
        for s in &sites {
            assert!(expected.contains(s), "unexpected seeded URL {s}");
        }
        // De-duplicated: no URL appears twice.
        let unique: std::collections::HashSet<_> = sites.iter().collect();
        assert_eq!(unique.len(), sites.len());
        Ok(())
    }

    #[test]
    fn sites_for_persona_skips_unknown_interests() {
        // A legacy/future phone interest that is not a known category yields no
        // sites for that entry (and does not error).
        let mut persona = persona_with(&[CategoryPool::TECHNOLOGY]);
        persona.interests.push("SPACE_LEGACY".to_string());
        let sites = sites_for_persona(&persona);
        // Still just the TECHNOLOGY sites; the unknown interest contributed none.
        assert!(!sites.is_empty());
    }

    #[test]
    fn sites_for_categories_matches_explicit_selection() -> Result<()> {
        let sites = sites_for_categories(&[CategoryPool::FINANCE, CategoryPool::FINANCE]);
        // Duplicate category collapses to the single category's de-duped sites.
        assert_eq!(sites.len(), category_sites(CategoryPool::FINANCE)?.len());
        Ok(())
    }

    #[test]
    fn seed_outcome_helpers() {
        let mut outcome = SeedOutcome::default();
        assert!(!outcome.any_visited());
        assert_eq!(outcome.visited_count(), 0);
        outcome.visited.push("https://example.com/".to_string());
        assert!(outcome.any_visited());
        assert_eq!(outcome.visited_count(), 1);
    }
}
