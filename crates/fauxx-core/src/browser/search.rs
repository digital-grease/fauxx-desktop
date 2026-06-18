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

//! Search-engine decoy poisoning (C6 H1): drive the isolated decoy browser to
//! issue persona-aligned search queries, so a phone-less / homelab desktop
//! pollutes search-engine and broker profiles the way the Android
//! `SearchPoisonModule` does, not only by visiting category sites.
//!
//! Each query string is produced + safety-gated by
//! [`crate::querybank::QueryGenerator`] BEFORE it reaches here; this module only
//! builds a SERP URL and navigates to it through the SAME guarded path as every
//! other decoy visit ([`DecoyPage::navigate`] -> the R3 auth-flow blocklist +
//! HTTPS-only check), then dwells with the persona's cadence. A search is, by
//! construction, a GET to a public SERP with credentials omitted; no form is
//! submitted and no real account is touched.
//!
//! US/English only today (the SERP locale params are hardcoded to `en`/`US`),
//! matching the desktop persona model. Each category runs an intent-chain session:
//! a goal query followed by `0..=`[`MAX_SESSION_REFINEMENTS`] in-topic refinements
//! on the same engine. Still deferred: organic SERP-link following.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use super::{isolation, DecoyBrowser};
use crate::error::Result;
use crate::persona::{CategoryPool, SyntheticPersona};
use crate::querybank::{commercial_lean, QueryGenerator};

/// One search engine plus its SERP-URL builder. The builder takes an
/// already-percent-encoded query and returns a full HTTPS URL. Kept as a data
/// table so adding/removing an engine (or, later, threading a locale) is a single
/// edit.
struct SearchEngine {
    name: &'static str,
    build: fn(&str) -> String,
}

// US/English SERP builders (locale params hardcoded to en/US; widen when more
// locales ship). Engine diversity makes the synthetic traffic harder to
// fingerprint as bot activity than a single SERP would be.
fn google_url(q: &str) -> String {
    format!("https://www.google.com/search?q={q}&hl=en&gl=US")
}
fn bing_url(q: &str) -> String {
    format!("https://www.bing.com/search?q={q}&setmkt=en-US")
}
fn duckduckgo_url(q: &str) -> String {
    format!("https://duckduckgo.com/?q={q}&kl=en-us")
}
fn yahoo_url(q: &str) -> String {
    format!("https://search.yahoo.com/search?p={q}")
}
fn yandex_url(q: &str) -> String {
    format!("https://yandex.com/search/?text={q}&lang=en")
}

const SEARCH_ENGINES: &[SearchEngine] = &[
    SearchEngine {
        name: "google",
        build: google_url,
    },
    SearchEngine {
        name: "bing",
        build: bing_url,
    },
    SearchEngine {
        name: "duckduckgo",
        build: duckduckgo_url,
    },
    SearchEngine {
        name: "yahoo",
        build: yahoo_url,
    },
    SearchEngine {
        name: "yandex",
        build: yandex_url,
    },
];

/// Percent-encode a query for use in a URL query string: every byte outside the
/// RFC 3986 unreserved set becomes `%XX` (spaces included, as `%20`). Conservative
/// on purpose so a query with `&`, `=`, `#`, or non-ASCII cannot break out of the
/// `q=` parameter.
fn percent_encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 3);
    for &b in query.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A dispatched (or skipped) search, recorded for the caller's measurement /
/// activity log. Only the GOAL query + engine are recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchDispatch {
    /// The targeted interest category.
    pub category: String,
    /// The search engine the query was sent to.
    pub engine: String,
    /// The goal query string that was dispatched.
    pub query: String,
}

/// The result of a search session: queries dispatched, and queries/URLs skipped
/// with a local-only reason (an empty/blocked generation, a refused URL, or a
/// failed navigation). Mirrors [`SeedOutcome`](super::SeedOutcome).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SearchOutcome {
    /// Searches that actually loaded a SERP.
    pub dispatched: Vec<SearchDispatch>,
    /// `(query-or-url, reason)` pairs for searches that were not dispatched.
    pub skipped: Vec<(String, String)>,
}

/// Build the SERP URL for `query` on `engine`, percent-encoding the query first.
fn build_search_url(engine: &SearchEngine, query: &str) -> String {
    (engine.build)(&percent_encode_query(query))
}

/// Max refinements appended to a goal in one intent-chain session (C6 H1 phase 2):
/// a goal SERP, then `0..=this` narrowing refinements on the SAME engine, like a
/// human scanning results and re-querying. Kept small so a session does not
/// dominate the dispatch budget.
const MAX_SESSION_REFINEMENTS: usize = 2;

/// Dispatch ONE query to `engine`'s SERP through the guarded path (HTTPS-only +
/// auth-flow blocklist), dwelling with the persona's cadence. Records the outcome
/// (dispatched, or skipped-with-reason) and returns whether it loaded. The query
/// MUST already be blocklist-vetted by the caller.
async fn dispatch_one(
    browser: &DecoyBrowser,
    engine: &SearchEngine,
    query: &str,
    category: CategoryPool,
    persona: &SyntheticPersona,
    seed: u64,
    outcome: &mut SearchOutcome,
) -> bool {
    let url = build_search_url(engine, query);
    // Pre-check the guardrail so a refused URL is a recorded skip, not a hard
    // error (navigate also checks; this keeps the session going).
    if let Err(e) = isolation::ensure_navigation_allowed(&url) {
        outcome.skipped.push((url, e.to_string()));
        return false;
    }
    let page = match browser.new_page().await {
        Ok(page) => page,
        Err(e) => {
            outcome.skipped.push((url, e.to_string()));
            return false;
        }
    };
    match page.navigate(&url).await {
        Ok(()) => {
            // Persona-paced dwell on the SERP; a dwell failure is non-fatal (the
            // navigation itself created the search-history entry).
            let _ = page.browse_with_persona(persona, seed).await;
            outcome.dispatched.push(SearchDispatch {
                category: category.as_name().to_string(),
                engine: engine.name.to_string(),
                query: query.to_string(),
            });
            true
        }
        Err(e) => {
            outcome.skipped.push((url, e.to_string()));
            false
        }
    }
}

/// Run a decoy search session for `persona` over the given `categories`. For each
/// category: generate a safe goal query, pick ONE engine, navigate the goal SERP
/// through the guarded path, then issue `0..=`[`MAX_SESSION_REFINEMENTS`] in-topic
/// refinements on the SAME engine (an intent-chain session, like a human narrowing
/// on one search tab) - C6 H1 phase 2.
///
/// Every query (goal AND each refinement) is blocklist-gated by `generator`; a
/// category that yields no safe goal is a recorded skip, not an error, and the
/// session continues. Each SERP URL is re-checked through the R3 navigation
/// guardrail before loading. `seed` makes the run reproducible.
pub async fn run_search_session(
    browser: &DecoyBrowser,
    persona: &SyntheticPersona,
    generator: &QueryGenerator,
    categories: &[CategoryPool],
    seed: u64,
) -> Result<SearchOutcome> {
    let mut outcome = SearchOutcome::default();
    let mut rng = StdRng::seed_from_u64(seed);
    let lean = commercial_lean(&persona_categories(persona));

    for &category in categories {
        let Some(goal) = generator.generate(category, lean, &mut rng) else {
            outcome.skipped.push((
                category.as_name().to_string(),
                "no safe query generated".to_string(),
            ));
            continue;
        };
        // One engine for the whole session (a human narrows on one SERP).
        let engine = &SEARCH_ENGINES[rng.random_range(0..SEARCH_ENGINES.len())];

        // Goal query. If it does not dispatch, skip the session (no refinements
        // off a goal that never loaded).
        if !dispatch_one(
            browser,
            engine,
            &goal,
            category,
            persona,
            seed,
            &mut outcome,
        )
        .await
        {
            continue;
        }
        // Intent-chain refinements, each freshly blocklist-gated by `refine_goal`.
        let count = rng.random_range(0..=MAX_SESSION_REFINEMENTS);
        for refinement in generator.refine_goal(&goal, count, &mut rng) {
            dispatch_one(
                browser,
                engine,
                &refinement,
                category,
                persona,
                seed,
                &mut outcome,
            )
            .await;
        }
    }

    tracing::info!(
        target: "fauxx_core::browser::search",
        persona_id = %persona.id,
        dispatched = outcome.dispatched.len(),
        skipped = outcome.skipped.len(),
        "ran decoy search session (goal + refinement chains)"
    );
    Ok(outcome)
}

/// Map a persona's interest names to [`CategoryPool`] (skipping any unknown name),
/// for the commercial-lean read.
fn persona_categories(persona: &SyntheticPersona) -> Vec<CategoryPool> {
    persona
        .interests
        .iter()
        .filter_map(|name| CategoryPool::from_name(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_is_conservative() {
        assert_eq!(
            percent_encode_query("best running shoes"),
            "best%20running%20shoes"
        );
        // Reserved characters that could break out of the q= param are encoded.
        assert_eq!(percent_encode_query("a&b=c#d"), "a%26b%3Dc%23d");
        // Unreserved pass through.
        assert_eq!(percent_encode_query("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn every_engine_builds_an_https_serp_url_with_the_query() {
        let encoded = percent_encode_query("trail running shoes");
        for engine in SEARCH_ENGINES {
            let url = (engine.build)(&encoded);
            assert!(
                url.starts_with("https://"),
                "{}: must be HTTPS: {url}",
                engine.name
            );
            assert!(
                url.contains(&encoded),
                "{}: must carry the query: {url}",
                engine.name
            );
        }
    }

    #[test]
    fn every_engine_serp_url_passes_the_navigation_guardrail() {
        // The decoy must not self-block its own search dispatch: every SERP URL it
        // builds has to clear the R3 auth-flow blocklist + HTTPS check. If a new
        // engine's domain were ever blocklisted, this catches it before shipping.
        let url_query = percent_encode_query("home espresso machine reviews");
        for engine in SEARCH_ENGINES {
            let url = (engine.build)(&url_query);
            assert!(
                isolation::ensure_navigation_allowed(&url).is_ok(),
                "{} SERP URL must pass the navigation guardrail: {url}",
                engine.name
            );
        }
    }
}
