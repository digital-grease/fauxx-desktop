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

//! Localized search-refinement templates (a faithful port of the Android
//! `SearchRefinements`). A refinement wraps a goal query so it stays topically
//! related but reads like how a real searcher narrows a subject:
//! `"trail running shoes"` -> `"best trail running shoes"` -> `"trail running
//! shoes reviews"`. Every refined string is still run through the
//! [`QueryBlocklist`](super::QueryBlocklist) by the caller before dispatch;
//! safety does not depend on the transformation.
//!
//! US/English only today, matching the desktop's persona model. The locale is
//! threaded through as a parameter ([`QueryLocale`]) so adding `es`/`fr`/`ru`
//! later is just adding their template tables, not reshaping the API.

use rand::RngExt;

/// The query locale. US/English only today; the enum exists so the rest of the
/// generator is already locale-parametric when more ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryLocale {
    #[default]
    En,
}

/// English refinement templates. `{}` is replaced by the (possibly lightly
/// reformulated) goal. Generic, benign, commercial-intent shaped.
const EN_TEMPLATES: &[&str] = &[
    "best {}",
    "{} reviews",
    "{} price",
    "{} near me",
    "how to choose {}",
    "{} for beginners",
    "is {} worth it",
    "{} alternatives",
    "{} comparison",
    "{} buying guide",
    "cheap {}",
    "{} deals",
    "top rated {}",
    "{} recommendations",
    "{} pros and cons",
    "{} tips",
];

/// Chance that a refinement reformulates (drops an edge word of) a 3+-word goal,
/// so a chain whose every query embeds the seed verbatim under a small fixed affix
/// set is not itself a clusterable fingerprint.
const REFORMULATE_FRACTION: f32 = 0.30;

fn templates_for(locale: QueryLocale) -> &'static [&'static str] {
    match locale {
        QueryLocale::En => EN_TEMPLATES,
    }
}

/// Build up to `count` distinct in-topic refinements of `goal` for `locale`. Each
/// output contains the goal, or, ~30% of the time for a 3+-word goal, the goal
/// minus one edge word. Returns fewer than `count` only if the template table is
/// smaller than `count`.
pub fn refine(goal: &str, locale: QueryLocale, count: usize, rng: &mut impl RngExt) -> Vec<String> {
    let templates = templates_for(locale);
    let take = count.min(templates.len());
    // Partial Fisher-Yates over template indices: pick `take` distinct templates.
    let mut idx: Vec<usize> = (0..templates.len()).collect();
    for i in 0..take {
        let j = rng.random_range(i..idx.len());
        idx.swap(i, j);
    }
    idx[..take]
        .iter()
        .map(|&i| {
            let core = refinement_core(goal, rng);
            templates[i].replace("{}", &core)
        })
        .collect()
}

/// The goal verbatim, or (~30% of the time, only for 3+-word goals) the goal with
/// one edge word dropped.
fn refinement_core(goal: &str, rng: &mut impl RngExt) -> String {
    let words: Vec<&str> = goal.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() < 3 || rng.random::<f32>() >= REFORMULATE_FRACTION {
        return goal.to_string();
    }
    if rng.random::<bool>() {
        words[1..].join(" ")
    } else {
        words[..words.len() - 1].join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn refinements_contain_the_goal_and_are_distinct() {
        let mut rng = StdRng::seed_from_u64(7);
        let out = refine("trail running shoes", QueryLocale::En, 4, &mut rng);
        assert_eq!(out.len(), 4);
        // Distinct templates were used.
        let unique: std::collections::HashSet<&String> = out.iter().collect();
        assert_eq!(unique.len(), 4);
        // Each output is non-empty and topically anchored to (a subset of) the goal.
        for r in &out {
            assert!(!r.is_empty());
            assert!(
                r.contains("running") || r.contains("shoes") || r.contains("trail"),
                "refinement should stay on topic: {r}"
            );
        }
    }

    #[test]
    fn count_is_capped_at_template_count_and_deterministic_per_seed() {
        let mut a = StdRng::seed_from_u64(42);
        let mut b = StdRng::seed_from_u64(42);
        assert_eq!(
            refine("home espresso machine", QueryLocale::En, 100, &mut a),
            refine("home espresso machine", QueryLocale::En, 100, &mut b),
        );
        // Capped at the template-table size.
        assert_eq!(
            refine("x", QueryLocale::En, 100, &mut StdRng::seed_from_u64(1)).len(),
            EN_TEMPLATES.len()
        );
    }

    #[test]
    fn short_goals_are_never_reformulated() {
        // A 1-2 word goal is always embedded whole (the reformulate branch needs
        // 3+ words), so the goal word always survives.
        let mut rng = StdRng::seed_from_u64(99);
        for _ in 0..50 {
            let out = refine("espresso", QueryLocale::En, 1, &mut rng);
            assert!(out[0].contains("espresso"));
        }
    }
}
