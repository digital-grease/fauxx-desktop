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

//! `fauxx-cli search <persona-id>`: a one-off decoy SEARCH session (C6 H1).
//!
//! A thin shim over [`Core::run_persona_search_session_live`]: it generates
//! persona-aligned, safety-gated queries and dispatches them to search engines
//! through the persona's isolated decoy browser. Standalone search-engine
//! poisoning for a phone-less / homelab deployment (no paired phone required).
//! Requires a system Chromium; cron or a timer can drive it periodically.

use fauxx_core::{Config, Core};

/// Run one search session for `persona_id`.
pub async fn run(
    config: Config,
    persona_id: String,
    decoy_id: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    let decoy_id = decoy_id.unwrap_or_else(|| format!("search-{persona_id}"));

    let outcome = core
        .run_persona_search_session_live(&persona_id, &decoy_id)
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        return Ok(());
    }

    println!(
        "search session for {persona_id}: {} dispatched, {} skipped",
        outcome.dispatched.len(),
        outcome.skipped.len()
    );
    for d in &outcome.dispatched {
        println!("  [{}] {} -> {}", d.category, d.query, d.engine);
    }
    for (what, why) in &outcome.skipped {
        println!("  skipped {what}: {why}");
    }
    Ok(())
}
