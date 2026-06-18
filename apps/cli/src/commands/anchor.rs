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

//! `fauxx-cli anchor ...`: the account-anchor identity-linkage inventory (C3 #19).
//!
//! Thin shims over the core anchor API. This is a READ-ONLY analysis inventory:
//! it records what the user types and scores it; it NEVER scrapes or automates
//! against a real account. Scoring and recommendations live in the core.

use fauxx_core::{Config, Core, IdentitySignal};

use crate::cli::{AnchorCommand, SignalArg};

/// Dispatch an `anchor` subcommand against a freshly opened core.
pub async fn run(config: Config, command: AnchorCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        AnchorCommand::Record {
            label,
            site,
            signal,
            shared_contact_key,
        } => record(&core, &label, &site, &signal, shared_contact_key).await,
        AnchorCommand::List { json } => list(&core, json).await,
        AnchorCommand::Score { json } => score(&core, json).await,
        AnchorCommand::Recommendations { json } => recommendations(&core, json).await,
    }
}

/// Record a curated account anchor.
async fn record(
    core: &Core,
    label: &str,
    site: &str,
    signals: &[SignalArg],
    shared_contact_key: Option<String>,
) -> anyhow::Result<()> {
    let signals: Vec<IdentitySignal> = signals.iter().map(|s| (*s).into()).collect();
    let anchor = core
        .record_account_anchor(label, site, signals, shared_contact_key)
        .await?;
    println!(
        "recorded anchor {} ({}) signals={}",
        anchor.id,
        anchor.label,
        anchor.signals.len()
    );
    Ok(())
}

/// List the anchor inventory.
async fn list(core: &Core, json: bool) -> anyhow::Result<()> {
    let anchors = core.list_account_anchors().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&anchors)?);
        return Ok(());
    }
    if anchors.is_empty() {
        println!("(no anchors)");
        return Ok(());
    }
    for a in &anchors {
        let signals: Vec<&str> = a.signals.iter().map(|s| s.as_str()).collect();
        println!(
            "{}  {}  site={}  signals=[{}]",
            a.id,
            a.label,
            a.site,
            signals.join(",")
        );
    }
    Ok(())
}

/// Score the inventory, strongest first.
async fn score(core: &Core, json: bool) -> anyhow::Result<()> {
    let scores = core.score_account_anchors().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&scores)?);
        return Ok(());
    }
    if scores.is_empty() {
        println!("(no anchors to score)");
        return Ok(());
    }
    for s in &scores {
        println!(
            "{}  {}  score={}  strength={}  linked={}",
            s.anchor_id, s.label, s.score, s.strength, s.linked_accounts
        );
    }
    Ok(())
}

/// Produce prioritized partitioning recommendations.
async fn recommendations(core: &Core, json: bool) -> anyhow::Result<()> {
    let recs = core.account_anchor_recommendations().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&recs)?);
        return Ok(());
    }
    if recs.is_empty() {
        println!("(no recommendations)");
        return Ok(());
    }
    for r in &recs {
        println!(
            "{}  {}  {}  score={}\n    {}",
            r.anchor_id,
            r.label,
            r.kind.as_str(),
            r.score,
            r.rationale
        );
    }
    Ok(())
}
