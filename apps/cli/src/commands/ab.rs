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

//! `fauxx ab ...`: control-profile A/B shadow profiles and cohort comparison
//! (C4 #21). Thin shims over the core measurement API; all statistics live in
//! the core (effect size, t-test, plain-words summary).

use fauxx_core::{Baseline, Config, Core, ShadowProfile, TTestKind};

use crate::cli::{AbCommand, ArmArg, PlatformArg};

/// Dispatch an `ab` subcommand against a freshly opened core.
pub async fn run(config: Config, command: AbCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        AbCommand::Define {
            label,
            persona_id,
            arm,
            id,
        } => define(&core, &label, &persona_id, arm, id).await,
        AbCommand::List { json } => list(&core, json).await,
        AbCommand::Compare {
            persona_id,
            platform,
            json,
        } => compare(&core, &persona_id, platform, json).await,
    }
}

/// Define (insert or replace) a shadow profile.
async fn define(
    core: &Core,
    label: &str,
    persona_id: &str,
    arm: ArmArg,
    id: Option<String>,
) -> anyhow::Result<()> {
    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let profile = ShadowProfile::new(id, label, arm.into(), persona_id, now_millis());
    core.save_shadow_profile(&profile).await?;
    println!(
        "defined shadow profile {} ({}) arm={}",
        profile.id,
        profile.label,
        profile.arm.as_str()
    );
    Ok(())
}

/// List the defined shadow profiles.
async fn list(core: &Core, json: bool) -> anyhow::Result<()> {
    let profiles = core.list_shadow_profiles().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
        return Ok(());
    }
    if profiles.is_empty() {
        println!("(no shadow profiles)");
        return Ok(());
    }
    for p in &profiles {
        println!(
            "{}  {}  arm={}  persona={}",
            p.id,
            p.label,
            p.arm.as_str(),
            p.persona_id
        );
    }
    Ok(())
}

/// Compare the treated and control cohorts on a platform.
async fn compare(
    core: &Core,
    persona_id: &str,
    platform: PlatformArg,
    json: bool,
) -> anyhow::Result<()> {
    // The persona-intent baseline measures drift from the persona's declared
    // goal, so fetch the persona to build it (errors if the id is unknown).
    let persona = core.get_persona(persona_id).await?;
    let baseline = Baseline::from_persona(&persona);
    let comparison = core
        .compare_shadow_cohorts(platform.into(), &baseline, TTestKind::Welch)
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
        return Ok(());
    }
    println!("{}", comparison.summary);
    println!(
        "  effect_size={:.4} ({}), direction={}, {}",
        comparison.effect_size,
        comparison.effect_magnitude,
        comparison.direction,
        comparison.confidence
    );
    Ok(())
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
