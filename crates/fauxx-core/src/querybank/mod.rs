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

//! Search-decoy query generation (C6 H1).
//!
//! When a persona drives the desktop decoy browser to poison SEARCH engines (not
//! just visit category sites), this module produces the query strings: it samples
//! from per-category corpora vendored from the Android app, applies per-INSTALL
//! refinement styling so a fleet of installs does not emit one shared,
//! fingerprintable query distribution, and gates every candidate through the
//! [`QueryBlocklist`] so a harmful or self-signalling query is never dispatched.
//!
//! The corpora and the safety blocklist are vendored verbatim from the Fauxx
//! Android app (`scripts/vendor-query-data.py` is the single source of truth) so
//! the desktop never emits a query the phone would refuse. US/English only by
//! design, matching the desktop's ACS-PUMS persona model.
//!
//! Each goal query is a head plus an optional per-install-styled `refinements`
//! wrap, blocklist-gated. The head is either a raw corpus pick or a NOVEL
//! `markov`-chained query (per-install mix), so a fleet does not share one
//! literal-corpus head set. Multi-query refinement CHAIN sessions live in the
//! caller ([`crate::browser::search`]). Still deferred: organic SERP-link
//! following (see the plan).

mod blocklist;
mod markov;
mod refinements;

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::persona::CategoryPool;

pub use blocklist::{QueryBlocklist, BUNDLED_HARMFUL_QUERIES};
pub use refinements::QueryLocale;

/// The per-category query corpora, embedded at compile time. Vendored from the
/// Android app's EN `assets/query_banks/` (single source of truth; re-sync with
/// `scripts/vendor-query-data.py`). One JSON object keyed by the SCREAMING_SNAKE
/// [`CategoryPool`] name.
pub const BUNDLED_QUERY_BANKS: &str = include_str!("query_banks_en.json");

/// Neutral commercial lean when a persona has no interests to read.
const NEUTRAL_LEAN: f32 = 0.5;
/// Ceiling on the per-query refinement rate (mirrors the Android grammar model).
const MAX_REFINE_RATE: f32 = 0.95;
/// Markov-head resample attempts before falling back to a corpus pick.
const MAX_RESAMPLE_ATTEMPTS: usize = 5;

/// Generates blocklist-safe, per-install-styled search queries for the decoy
/// browser. Build once (it loads + filters the corpora) and reuse.
#[derive(Debug)]
pub struct QueryGenerator {
    /// Per-category corpora, keyed by [`CategoryPool::as_name`], pre-filtered
    /// through the blocklist at load (defense in depth).
    banks: HashMap<String, Vec<String>>,
    /// The harmful-query guard, re-checked on every emitted query.
    blocklist: QueryBlocklist,
    /// This install's refinement style, so two installs querying the same
    /// category diverge in how they refine, defeating the fleet signature.
    style: InstallStyle,
    /// Per-category bigram chain for NOVEL heads beyond the literal corpus.
    markov: markov::MarkovModel,
}

impl QueryGenerator {
    /// Build a generator from the bundled corpora + blocklist. `install_seed`
    /// fixes this install's generation style deterministically (derive it from a
    /// stable per-device value, e.g. the sync device key).
    pub fn new(install_seed: u64) -> Self {
        let blocklist = QueryBlocklist::bundled();
        let banks = load_banks(&blocklist);
        let markov = markov::MarkovModel::train(&banks);
        Self {
            banks,
            blocklist,
            style: InstallStyle::from_seed(install_seed),
            markov,
        }
    }

    /// Generate ONE blocklist-safe goal query for `category`.
    ///
    /// `commercial_lean` (0.0 informational .. 1.0 commercial; 0.5 neutral, from
    /// [`commercial_lean`]) nudges the per-install refinement rate. Returns `None`
    /// if the category bank is unknown/empty, or the drawn candidate is blocked,
    /// so the caller simply suppresses that dispatch (a missing action is cheaper
    /// than an unsafe one).
    pub fn generate(
        &self,
        category: CategoryPool,
        commercial_lean: f32,
        rng: &mut impl RngExt,
    ) -> Option<String> {
        let bank = self.banks.get(category.as_name())?;
        if bank.is_empty() {
            return None;
        }
        // Head: a Markov-chained NOVEL query (with this install's probability) or a
        // raw corpus pick. The per-install head-source mix + refine style make two
        // installs' query distributions diverge. The Markov path is gated +
        // resampled by `markov_head`; the corpus path is already filtered at load.
        // The final re-gate is belt-and-suspenders.
        let target_len = rng.random_range(3..=8);
        let head = if rng.random::<f32>() < self.style.markov_head_probability {
            self.markov_head(category.as_name(), bank, target_len, rng)
        } else {
            Some(bank[rng.random_range(0..bank.len())].clone())
        };
        let head = match head {
            Some(h) if !h.is_empty() && !self.blocklist.is_blocked(&h) => h,
            _ => return None,
        };

        // Per-install + persona-nudged refine rate. The per-install component is
        // what makes two installs' query distributions visibly different.
        let refine_rate = (self.style.refine_probability
            + commercial_lean * self.style.persona_refine_weight)
            .clamp(0.0, MAX_REFINE_RATE);
        if rng.random::<f32>() >= refine_rate {
            return Some(head);
        }

        // One refinement; a refinement is freshly COMPOSED text, so it MUST pass
        // the guard. If it is blocked (or empty), fall back to the safe head.
        match refinements::refine(&head, QueryLocale::En, 1, rng)
            .into_iter()
            .next()
        {
            Some(refined) if !refined.is_empty() && !self.blocklist.is_blocked(&refined) => {
                Some(refined)
            }
            _ => Some(head),
        }
    }

    /// A blocklist-safe Markov-chained head: resample up to
    /// [`MAX_RESAMPLE_ATTEMPTS`] times (bigram chaining can compose a blocked
    /// phrase from safe sources), then fall back to a corpus pick (already
    /// load-filtered, so safe and on-topic).
    fn markov_head(
        &self,
        category: &str,
        bank: &[String],
        target_len: usize,
        rng: &mut impl RngExt,
    ) -> Option<String> {
        for _ in 0..MAX_RESAMPLE_ATTEMPTS {
            if let Some(q) = self.markov.generate(category, bank, target_len, rng) {
                if !q.is_empty() && !self.blocklist.is_blocked(&q) {
                    return Some(q);
                }
            }
        }
        // Chaining kept producing blocked output: fall back to a safe corpus pick.
        Some(bank[rng.random_range(0..bank.len())].clone())
    }

    /// Build up to `count` in-topic REFINEMENTS of `goal` for an intent-chain
    /// search session (C6 H1 phase 2): the way a real searcher narrows a subject
    /// (`"trail running shoes"` -> `"best trail running shoes"` -> `"trail running
    /// shoes reviews"`). Each refinement is freshly composed text, so every one is
    /// re-gated through the blocklist; a blocked refinement is dropped (its goal
    /// already passed), so the returned list may be shorter than `count`.
    pub fn refine_goal(&self, goal: &str, count: usize, rng: &mut impl RngExt) -> Vec<String> {
        refinements::refine(goal, QueryLocale::En, count, rng)
            .into_iter()
            .filter(|r| !r.is_empty() && !self.blocklist.is_blocked(r))
            .collect()
    }

    /// Whether the safety blocklist failed to load (and so everything is blocked).
    /// A health signal the caller can surface; when set, [`generate`](Self::generate)
    /// returns `None` for every category.
    pub fn blocklist_load_failed(&self) -> bool {
        self.blocklist.load_failed()
    }
}

/// Persona commercial lean in `[0, 1]`: the fraction of the persona's interests
/// that are commercial-intent categories; `NEUTRAL_LEAN` when there are none.
/// Commercial personas refine slightly more (buy/compare/price intent).
pub fn commercial_lean(interests: &[CategoryPool]) -> f32 {
    if interests.is_empty() {
        return NEUTRAL_LEAN;
    }
    let commercial = interests.iter().filter(|c| is_commercial(**c)).count();
    commercial as f32 / interests.len() as f32
}

/// Categories with buy/compare/price intent (mirrors the Android set).
fn is_commercial(c: CategoryPool) -> bool {
    matches!(
        c,
        CategoryPool::FINANCE
            | CategoryPool::REAL_ESTATE
            | CategoryPool::AUTOMOTIVE
            | CategoryPool::FASHION
            | CategoryPool::BEAUTY
            | CategoryPool::TRAVEL
            | CategoryPool::HOME_IMPROVEMENT
            | CategoryPool::BUSINESS
            | CategoryPool::RETIREMENT
            | CategoryPool::TECHNOLOGY
            | CategoryPool::FITNESS
            | CategoryPool::PETS
            | CategoryPool::GAMING
    )
}

/// Per-install refinement style, derived deterministically from the device seed.
/// Ranges are deliberately wide so two installs' query distributions differ even
/// with the same corpus and persona.
#[derive(Debug)]
struct InstallStyle {
    /// Base chance a head is refined (0.20 .. 0.70).
    refine_probability: f32,
    /// How much the persona's commercial lean shifts the refine rate (0.10 .. 0.40).
    persona_refine_weight: f32,
    /// Chance a head is Markov-chained (novel) vs. a raw corpus pick (0.40 .. 0.90).
    /// Leans high so most heads escape the literal-corpus fingerprint, but every
    /// install still differs.
    markov_head_probability: f32,
}

impl InstallStyle {
    fn from_seed(seed: u64) -> Self {
        let mut r = StdRng::seed_from_u64(seed);
        Self {
            refine_probability: 0.20 + r.random::<f32>() * 0.50,
            persona_refine_weight: 0.10 + r.random::<f32>() * 0.30,
            markov_head_probability: 0.40 + r.random::<f32>() * 0.50,
        }
    }
}

/// Parse + blocklist-filter the bundled corpora into per-category banks keyed by
/// [`CategoryPool::as_name`]. A parse failure logs and yields empty banks (so
/// generation simply produces nothing); an unknown category key is skipped.
fn load_banks(blocklist: &QueryBlocklist) -> HashMap<String, Vec<String>> {
    let raw: HashMap<String, Vec<String>> = match serde_json::from_str(BUNDLED_QUERY_BANKS) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(
                target: "fauxx_core::querybank",
                error = %e,
                "query banks failed to parse; search-decoy generation will be empty"
            );
            return HashMap::new();
        }
    };
    let mut banks = HashMap::with_capacity(raw.len());
    for (name, queries) in raw {
        if CategoryPool::from_name(&name).is_none() {
            tracing::warn!(
                target: "fauxx_core::querybank",
                category = %name,
                "query-bank category is not a known CategoryPool; skipping"
            );
            continue;
        }
        let filtered: Vec<String> = queries
            .into_iter()
            .filter(|q| !blocklist.is_blocked(q))
            .collect();
        banks.insert(name, filtered);
    }
    banks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_has_a_nonempty_bank() {
        // Proves the vendored bank keys all map to a desktop CategoryPool variant
        // (i.e. the Android query-bank names match the frozen 32-value enum) and
        // that the blocklist did not empty any bank.
        let gen = QueryGenerator::new(1);
        for c in CategoryPool::all() {
            let bank = gen.banks.get(c.as_name());
            assert!(
                bank.is_some_and(|b| !b.is_empty()),
                "category {} should have a non-empty query bank",
                c.as_name()
            );
        }
        assert!(!gen.blocklist_load_failed());
    }

    #[test]
    fn generates_safe_queries_for_every_category() {
        let gen = QueryGenerator::new(42);
        let mut rng = StdRng::seed_from_u64(7);
        for c in CategoryPool::all() {
            // Draw several so both the refine and no-refine branches are exercised.
            for _ in 0..8 {
                let q = match gen.generate(*c, commercial_lean(&[*c]), &mut rng) {
                    Some(q) => q,
                    None => panic!("a known category should yield a query: {}", c.as_name()),
                };
                assert!(!q.is_empty());
                // Every emitted query is blocklist-clean (the core safety contract).
                assert!(
                    !QueryBlocklist::bundled().is_blocked(&q),
                    "emitted query must be blocklist-safe: {q}"
                );
            }
        }
    }

    #[test]
    fn generation_is_deterministic_for_a_fixed_seed_and_rng() {
        let gen = QueryGenerator::new(99);
        let mut a = StdRng::seed_from_u64(5);
        let mut b = StdRng::seed_from_u64(5);
        let qa = gen.generate(CategoryPool::TECHNOLOGY, 0.5, &mut a);
        let qb = gen.generate(CategoryPool::TECHNOLOGY, 0.5, &mut b);
        assert_eq!(qa, qb);
    }

    #[test]
    fn install_style_varies_with_the_seed() {
        let a = InstallStyle::from_seed(1);
        let b = InstallStyle::from_seed(2);
        // Overwhelmingly likely to differ; the point is installs are not identical.
        assert!(
            (a.refine_probability - b.refine_probability).abs() > f32::EPSILON
                || (a.persona_refine_weight - b.persona_refine_weight).abs() > f32::EPSILON
                || (a.markov_head_probability - b.markov_head_probability).abs() > f32::EPSILON
        );
    }

    #[test]
    fn markov_head_stays_safe_and_can_produce_novel_heads() {
        let gen = QueryGenerator::new(42);
        let bank = match gen.banks.get(CategoryPool::TECHNOLOGY.as_name()) {
            Some(b) if !b.is_empty() => b.clone(),
            _ => panic!("TECHNOLOGY bank should be populated"),
        };
        let corpus: std::collections::HashSet<&String> = bank.iter().collect();
        let mut novel = 0usize;
        for s in 0..200u64 {
            let mut rng = StdRng::seed_from_u64(s);
            let target_len = 3 + (s as usize % 6);
            let head = match gen.markov_head(
                CategoryPool::TECHNOLOGY.as_name(),
                &bank,
                target_len,
                &mut rng,
            ) {
                Some(h) => h,
                None => panic!("a populated bank should always yield a head"),
            };
            // The core safety contract: a Markov-chained head is never blocked.
            assert!(!head.is_empty());
            assert!(
                !QueryBlocklist::bundled().is_blocked(&head),
                "markov head must be blocklist-safe: {head}"
            );
            if !corpus.contains(&head) {
                novel += 1;
            }
        }
        // Chaining must escape the literal corpus at least sometimes, else it adds
        // no anti-fingerprint value over a raw corpus pick.
        assert!(
            novel > 0,
            "markov chaining produced only verbatim corpus lines"
        );
    }

    #[test]
    fn commercial_lean_reads_interests() {
        assert_eq!(commercial_lean(&[]), NEUTRAL_LEAN);
        assert_eq!(commercial_lean(&[CategoryPool::FINANCE]), 1.0);
        assert_eq!(commercial_lean(&[CategoryPool::POLITICS]), 0.0);
        assert!(
            (commercial_lean(&[CategoryPool::FINANCE, CategoryPool::POLITICS]) - 0.5).abs() < 1e-6
        );
    }

    #[test]
    fn refine_goal_yields_count_bounded_safe_in_topic_refinements() {
        let gen = QueryGenerator::new(3);
        let mut rng = StdRng::seed_from_u64(11);
        let refs = gen.refine_goal("home espresso machine", 2, &mut rng);
        assert!(refs.len() <= 2);
        for r in &refs {
            assert!(!r.is_empty());
            // Every refinement is blocklist-clean (the chain safety contract).
            assert!(
                !QueryBlocklist::bundled().is_blocked(r),
                "refinement must be blocklist-safe: {r}"
            );
            // ...and stays on the goal's topic.
            assert!(
                r.contains("espresso") || r.contains("machine") || r.contains("home"),
                "refinement should stay on topic: {r}"
            );
        }
    }
}
