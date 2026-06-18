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

//! Harmful-query blocklist for the search-decoy path (C6 H1), a faithful port of
//! the Android `QueryBlocklist`. It blocks query CONTENT (distinct from the URL
//! destination blocklist) to defend against two risks the desktop's synthetic
//! search traffic could otherwise create for the user:
//!
//! - **Class A (law-enforcement / criminal-attention)**: queries whose mere
//!   appearance in a broker / ISP / search-engine profile could plausibly draw an
//!   investigation, watchlist placement, or ban (CSAM, weapon/drug synthesis,
//!   terrorism, trafficking, cybercrime tooling, ...). "Safe for a human to run"
//!   is no defense: the harm is the profile entry, not the result.
//! - **Class B (self-signal)**: queries that are benign (often lifesaving) for a
//!   real searcher but create a false first-person distress signal when injected
//!   as synthetic noise (988 / crisis lines, DV hotlines, asylum, bankruptcy,
//!   eviction, ...), which can trigger wellness checks, insurance denial, or
//!   watchlists.
//!
//! The corpus and matching rules are vendored verbatim from the Android app (see
//! `scripts/vendor-query-data.py`) so the desktop never emits a query the phone
//! would refuse. The terms + patterns live in [`BUNDLED_HARMFUL_QUERIES`],
//! embedded at compile time.
//!
//! **Fail closed.** If the corpus fails to parse, is empty, or contains a regex
//! that does not compile, [`QueryBlocklist::is_blocked`] returns `true` for EVERY
//! input (the search-decoy path then emits nothing) rather than run with a
//! degraded guard. The bundled-corpus test asserts this never happens in
//! practice (it loads and every pattern compiles).

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

/// The harmful-query corpus, embedded at compile time. Vendored from the Android
/// app's `assets/harmful_queries.json` (single source of truth; re-sync with
/// `scripts/vendor-query-data.py`).
pub const BUNDLED_HARMFUL_QUERIES: &str = include_str!("harmful_queries.json");

/// JSON shape of the harmful-query corpus. A leading `_readme` field (contributor
/// rules) is present in the file and ignored on load.
#[derive(Debug, Deserialize)]
struct HarmfulQueriesJson {
    #[serde(default)]
    class_a_terms: Vec<String>,
    #[serde(default)]
    self_signal_terms: Vec<String>,
    #[serde(default)]
    regex_patterns: Vec<String>,
}

/// A loaded harmful-query blocklist: normalized phrase terms (substring match)
/// plus compiled regex patterns. Build once and reuse.
#[derive(Debug)]
pub struct QueryBlocklist {
    /// NFKC-normalized phrase terms; a query is blocked if it CONTAINS one.
    phrase_terms: Vec<String>,
    /// Compiled regex patterns; a query is blocked if any matches.
    regexes: Vec<Regex>,
    /// When set, [`is_blocked`](Self::is_blocked) returns `true` for everything.
    load_failed: bool,
}

impl QueryBlocklist {
    /// Load the compile-time bundled corpus ([`BUNDLED_HARMFUL_QUERIES`]).
    pub fn bundled() -> Self {
        Self::from_json(BUNDLED_HARMFUL_QUERIES)
    }

    /// Parse and compile a blocklist from JSON. Fails CLOSED (every query blocked)
    /// on a parse error, an all-empty corpus, or a regex that does not compile.
    pub fn from_json(json: &str) -> Self {
        let parsed: HarmfulQueriesJson = match serde_json::from_str(json) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    target: "fauxx_core::querybank",
                    error = %e,
                    "harmful-query corpus failed to parse; failing closed (all queries blocked)"
                );
                return Self::failed();
            }
        };

        if parsed.class_a_terms.is_empty()
            && parsed.self_signal_terms.is_empty()
            && parsed.regex_patterns.is_empty()
        {
            tracing::error!(
                target: "fauxx_core::querybank",
                "harmful-query corpus is empty; failing closed (all queries blocked)"
            );
            return Self::failed();
        }

        let phrase_terms: Vec<String> = parsed
            .class_a_terms
            .iter()
            .chain(parsed.self_signal_terms.iter())
            .map(|t| normalize_for_match(t))
            .filter(|t| !t.is_empty())
            .collect();

        // Compile every regex; a single failure fails the whole guard CLOSED
        // rather than silently dropping one safety pattern. `regex` is Unicode-
        // aware by default (the `unicode` feature), matching the Android `(?U)`
        // flag; input is already lowercased by `normalize_for_match`, so
        // case-insensitivity is belt-and-suspenders.
        let mut regexes = Vec::with_capacity(parsed.regex_patterns.len());
        for pat in &parsed.regex_patterns {
            match RegexBuilder::new(pat).case_insensitive(true).build() {
                Ok(re) => regexes.push(re),
                Err(e) => {
                    tracing::error!(
                        target: "fauxx_core::querybank",
                        pattern = %pat,
                        error = %e,
                        "harmful-query regex failed to compile; failing closed"
                    );
                    return Self::failed();
                }
            }
        }

        Self {
            phrase_terms,
            regexes,
            load_failed: false,
        }
    }

    /// `true` if `query` matches any harmful phrase (substring) or regex. Returns
    /// `true` for every input when the corpus failed to load.
    pub fn is_blocked(&self, query: &str) -> bool {
        if self.load_failed {
            return true;
        }
        let normalized = normalize_for_match(query);
        if self.phrase_terms.iter().any(|t| normalized.contains(t)) {
            return true;
        }
        self.regexes.iter().any(|re| re.is_match(&normalized))
    }

    /// Whether the corpus failed to load (and so everything is being blocked).
    pub fn load_failed(&self) -> bool {
        self.load_failed
    }

    fn failed() -> Self {
        Self {
            phrase_terms: Vec::new(),
            regexes: Vec::new(),
            load_failed: true,
        }
    }
}

/// Canonicalize text before matching so common evasion tricks cannot smuggle a
/// harmful phrase past the substring/regex guard. Applied to BOTH the query and
/// the stored terms so the two sides canonicalize identically:
///
/// - NFKC folds compatibility forms (fullwidth letters/digits, ligatures) to
///   their canonical equivalents, so `"ｂｏｍｂ"` / `"９８８"` match `"bomb"` / `"988"`.
/// - Zero-width and soft-hyphen characters (often injected mid-word) are stripped.
/// - Lowercased for case-insensitive substring matching.
/// - Internal whitespace runs (spaces, tabs, newlines, Unicode spaces) are
///   collapsed to a single ASCII space, and the ends trimmed. Stored phrase terms
///   use single spaces, so WITHOUT this a multi-word term could be evaded by
///   separating its words with a tab, double-space, or newline.
///
/// Cross-script homoglyph confusables (e.g. Cyrillic "а" for Latin "a") are NOT
/// folded here; that needs a full Unicode confusables table and is a follow-up.
fn normalize_for_match(text: &str) -> String {
    let folded: String = text
        .nfkc()
        .filter(|c| !is_zero_width(*c))
        .collect::<String>()
        .to_lowercase();
    // `split_whitespace` splits on any Unicode whitespace and drops empty runs, so
    // this collapses internal runs AND trims the ends in one pass.
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Zero-width / format characters stripped before matching: ZWSP..ZWJ
/// (U+200B-U+200D), word joiner (U+2060), BOM/ZWNBSP (U+FEFF), soft hyphen
/// (U+00AD). Injecting these mid-phrase is a common way to break substring match.
fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_corpus_loads_and_every_pattern_compiles() {
        // The real vendored corpus must load cleanly (not fail closed): this
        // proves all regex_patterns compile under Rust's `regex` and the lists
        // are non-empty. If it ever fails closed, the search-decoy path goes dark.
        let bl = QueryBlocklist::bundled();
        assert!(
            !bl.load_failed(),
            "bundled harmful-query corpus must load (all patterns compile, lists non-empty)"
        );
        assert!(!bl.phrase_terms.is_empty());
        assert!(!bl.regexes.is_empty());
        // A plainly-benign commercial query is not blocked.
        assert!(!bl.is_blocked("best running shoes for beginners"));
    }

    #[test]
    fn matches_phrase_terms_and_is_normalization_resistant() {
        let bl = QueryBlocklist::from_json(
            r#"{"class_a_terms":["forbidden phrase"],"self_signal_terms":[],"regex_patterns":[]}"#,
        );
        assert!(!bl.load_failed());
        assert!(bl.is_blocked("some forbidden phrase here"));
        assert!(!bl.is_blocked("a perfectly fine query"));
        // Case-insensitive.
        assert!(bl.is_blocked("FORBIDDEN PHRASE"));
        // Zero-width injection mid-phrase does not evade the guard.
        assert!(bl.is_blocked("forbidden\u{200B} phrase"));
        // Fullwidth characters fold to ASCII via NFKC.
        assert!(bl.is_blocked("ｆｏｒｂｉｄｄｅｎ phrase"));
        // Altered inter-word whitespace (double-space, tab, newline) does not evade
        // a multi-word term: normalization collapses internal whitespace runs.
        assert!(bl.is_blocked("a forbidden  phrase"));
        assert!(bl.is_blocked("a forbidden\tphrase"));
        assert!(bl.is_blocked("a forbidden\nphrase"));
    }

    #[test]
    fn matches_regex_patterns() {
        let bl = QueryBlocklist::from_json(
            r#"{"class_a_terms":[],"self_signal_terms":[],"regex_patterns":["\\b988\\b"]}"#,
        );
        assert!(!bl.load_failed());
        assert!(bl.is_blocked("call 988 now"));
        assert!(!bl.is_blocked("i scored 9889 points"));
    }

    #[test]
    fn fails_closed_on_bad_input() {
        // Unparseable JSON, an all-empty corpus, and an uncompilable regex each
        // block EVERY query.
        for bad in [
            "not json",
            r#"{"class_a_terms":[],"self_signal_terms":[],"regex_patterns":[]}"#,
            r#"{"class_a_terms":[],"self_signal_terms":[],"regex_patterns":["("]}"#,
        ] {
            let bl = QueryBlocklist::from_json(bad);
            assert!(bl.load_failed(), "should fail closed for: {bad}");
            assert!(bl.is_blocked("anything at all"));
        }
    }
}
