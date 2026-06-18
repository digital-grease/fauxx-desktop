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

//! `fauxx campaign ...`: goal-driven campaigns, the closed loop (C8 #33).
//!
//! Thin shims over the core campaign planner. The goal model, the gap-to-
//! intensity mapping, the dwell/lifecycle, and all persistence live in the core;
//! the CLI only constructs the campaign and renders directives.

use fauxx_core::{Campaign, Comparator, Config, Core, Goal, TargetMetric};

use crate::cli::{CampaignCommand, ComparatorArg};

/// Dispatch a `campaign` subcommand against a freshly opened core.
pub async fn run(config: Config, command: CampaignCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        CampaignCommand::Create {
            label,
            persona_id,
            target_segment,
            comparator,
            threshold,
            id,
        } => {
            create(
                &core,
                &label,
                &persona_id,
                &target_segment,
                comparator,
                threshold,
                id,
            )
            .await
        }
        CampaignCommand::List { persona, json } => list(&core, persona.as_deref(), json).await,
        CampaignCommand::Start { id } => start(&core, &id).await,
        CampaignCommand::Pause { id } => pause(&core, &id).await,
        CampaignCommand::Adjust { id, threshold } => adjust(&core, &id, threshold).await,
        CampaignCommand::Tick { id, json } => tick(&core, &id, json).await,
    }
}

/// Create a goal-driven campaign.
#[allow(clippy::too_many_arguments)]
async fn create(
    core: &Core,
    label: &str,
    persona_id: &str,
    target_segment: &str,
    comparator: ComparatorArg,
    threshold: f64,
    id: Option<String>,
) -> anyhow::Result<()> {
    let comparator: Comparator = comparator.into();
    let goal = Goal::new(TargetMetric::SegmentDrift, comparator, threshold)?;
    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let campaign = Campaign::new(id, label, persona_id, target_segment, goal, now_millis());
    core.save_campaign(&campaign).await?;
    println!(
        "created campaign {} ({}) target={} status={}",
        campaign.id,
        campaign.label,
        campaign.target_segment,
        campaign.status.as_str()
    );
    Ok(())
}

/// List campaigns, optionally scoped to a persona.
async fn list(core: &Core, persona: Option<&str>, json: bool) -> anyhow::Result<()> {
    let campaigns = core.list_campaigns(persona).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&campaigns)?);
        return Ok(());
    }
    if campaigns.is_empty() {
        println!("(no campaigns)");
        return Ok(());
    }
    for c in &campaigns {
        println!(
            "{}  {}  persona={}  target={}  status={}",
            c.id,
            c.label,
            c.persona_id,
            c.target_segment,
            c.status.as_str()
        );
    }
    Ok(())
}

/// Start (or resume) a campaign.
async fn start(core: &Core, id: &str) -> anyhow::Result<()> {
    let campaign = core.start_campaign(id, now_millis()).await?;
    println!("campaign {} -> {}", campaign.id, campaign.status.as_str());
    Ok(())
}

/// Pause a campaign.
async fn pause(core: &Core, id: &str) -> anyhow::Result<()> {
    let campaign = core.pause_campaign(id, now_millis()).await?;
    println!("campaign {} -> {}", campaign.id, campaign.status.as_str());
    Ok(())
}

/// Adjust a campaign's goal threshold.
async fn adjust(core: &Core, id: &str, threshold: f64) -> anyhow::Result<()> {
    let campaign = core
        .adjust_campaign_threshold(id, threshold, now_millis())
        .await?;
    println!(
        "campaign {} threshold -> {}",
        campaign.id, campaign.goal.threshold
    );
    Ok(())
}

/// Advance a campaign's closed loop one tick.
async fn tick(core: &Core, id: &str, json: bool) -> anyhow::Result<()> {
    let directive = core.tick_campaign(id, now_millis()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&directive)?);
        return Ok(());
    }
    match directive.intensity {
        Some(level) => println!(
            "tick: running at {level:?}, bias toward {}",
            directive.target_segment
        ),
        None => println!("tick: idle (not driving activity)"),
    }
    Ok(())
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
