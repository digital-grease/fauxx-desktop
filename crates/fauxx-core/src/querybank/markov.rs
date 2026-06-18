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

//! Bigram Markov query head generator (C6 H1 phase 2), a faithful port of the
//! Android `MarkovQueryGenerator`.
//!
//! v1 heads are raw corpus picks, so the set of possible head queries is exactly
//! the shipped corpus, a fleet-wide signature a broker could fingerprint. This
//! model trains a per-category bigram (2-gram) chain on the corpus and generates
//! NOVEL but natural-looking heads by seeding with a real corpus phrase and
//! extending word-by-word through the chain. Combined with the per-install
//! refinement style, two installs' query distributions diverge in BOTH the head
//! set and the refinement shape.
//!
//! Safety is NOT this model's job: bigram chaining can assemble a harmful phrase
//! from individually safe sources ("how to" + "hang" + ...), so every output is
//! re-gated by [`QueryBlocklist`](super::QueryBlocklist) in the caller, which
//! resamples and then falls back to a (corpus-pick, already-filtered) head.

use std::collections::HashMap;

use rand::RngExt;

/// Minimum words a chained query must have to be emitted; below this we fall back
/// to the raw seed phrase from the corpus (always a natural corpus line).
const MIN_PLAUSIBLE_WORDS: usize = 3;

/// A per-category bigram model: for each category, `word -> words that follow it`
/// in the training corpus. Trained once at construction from the (already
/// blocklist-filtered) banks.
#[derive(Debug)]
pub(super) struct MarkovModel {
    /// category name -> (lowercased word -> following words, verbatim).
    bigrams: HashMap<String, HashMap<String, Vec<String>>>,
}

impl MarkovModel {
    /// Train per-category bigram chains over the banks (keyed by category name).
    pub(super) fn train(banks: &HashMap<String, Vec<String>>) -> Self {
        let mut bigrams = HashMap::with_capacity(banks.len());
        for (category, queries) in banks {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for query in queries {
                let words: Vec<&str> = query.split_whitespace().collect();
                for pair in words.windows(2) {
                    map.entry(pair[0].to_lowercase())
                        .or_default()
                        .push(pair[1].to_string());
                }
            }
            bigrams.insert(category.clone(), map);
        }
        Self { bigrams }
    }

    /// Generate ONE bigram-chained query for `category`, seeded from a random
    /// phrase in `bank`, extended to `target_len` words. May return a blocked or
    /// short output; the caller re-gates and may resample. Returns `None` only
    /// when the category/bank is empty (nothing to seed from).
    ///
    /// Seeds with the FULL corpus phrase (not a single word) so the output keeps a
    /// plausible structure; if extension stalls below [`MIN_PLAUSIBLE_WORDS`] it
    /// returns the seed phrase verbatim (a natural corpus line).
    pub(super) fn generate(
        &self,
        category: &str,
        bank: &[String],
        target_len: usize,
        rng: &mut impl RngExt,
    ) -> Option<String> {
        if bank.is_empty() {
            return None;
        }
        let map = self.bigrams.get(category)?;
        let seed = &bank[rng.random_range(0..bank.len())];
        let mut result: Vec<String> = seed.split_whitespace().map(str::to_string).collect();
        if result.is_empty() {
            return None;
        }
        while result.len() < target_len {
            let last = result[result.len() - 1].to_lowercase();
            match map.get(&last) {
                Some(nexts) if !nexts.is_empty() => {
                    result.push(nexts[rng.random_range(0..nexts.len())].clone());
                }
                // No outgoing bigram for the last word: stop extending.
                _ => break,
            }
        }
        if result.len() < MIN_PLAUSIBLE_WORDS {
            return Some(seed.clone());
        }
        Some(result.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn banks() -> HashMap<String, Vec<String>> {
        let mut m = HashMap::new();
        m.insert(
            "TECHNOLOGY".to_string(),
            vec![
                "best laptop for students".to_string(),
                "best smartphone camera comparison".to_string(),
                "laptop battery life tips".to_string(),
            ],
        );
        m
    }

    #[test]
    fn generates_multiword_queries_from_the_chain() {
        let model = MarkovModel::train(&banks());
        let mut rng = StdRng::seed_from_u64(7);
        let q = match model.generate("TECHNOLOGY", &banks()["TECHNOLOGY"], 6, &mut rng) {
            Some(q) => q,
            None => panic!("a trained, non-empty category should yield a query"),
        };
        assert!(q.split_whitespace().count() >= MIN_PLAUSIBLE_WORDS);
        // Every word came from the corpus vocabulary (no foreign tokens).
        let vocab: std::collections::HashSet<String> = banks()["TECHNOLOGY"]
            .iter()
            .flat_map(|s| s.split_whitespace().map(str::to_lowercase))
            .collect();
        for w in q.split_whitespace() {
            assert!(vocab.contains(&w.to_lowercase()), "foreign token: {w}");
        }
    }

    #[test]
    fn empty_or_unknown_category_yields_none() {
        let model = MarkovModel::train(&banks());
        let mut rng = StdRng::seed_from_u64(1);
        assert!(model.generate("TECHNOLOGY", &[], 6, &mut rng).is_none());
        assert!(model
            .generate("UNKNOWN", &["x y z".to_string()], 6, &mut rng)
            .is_none());
    }

    #[test]
    fn is_deterministic_for_a_fixed_seed() {
        let model = MarkovModel::train(&banks());
        let b = banks();
        let q1 = model.generate(
            "TECHNOLOGY",
            &b["TECHNOLOGY"],
            6,
            &mut StdRng::seed_from_u64(3),
        );
        let q2 = model.generate(
            "TECHNOLOGY",
            &b["TECHNOLOGY"],
            6,
            &mut StdRng::seed_from_u64(3),
        );
        assert_eq!(q1, q2);
    }
}
