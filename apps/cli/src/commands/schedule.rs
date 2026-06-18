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

//! `fauxx-cli schedule`: preview the household action timeline (C1 #10, O4).
//!
//! A thin shim that assembles one [`DeviceIntent`] per device (the local device
//! plus every currently paired peer), asks the core to plan the household day
//! via [`Core::plan_household`], and prints a summary (total actions and the
//! active-window span) followed by the first N scheduled actions. This is what
//! exercises O4 headlessly: the planning, aggregation, and anti-collision all
//! run in the core; the CLI only renders the result.

use fauxx_core::{Config, Core, DeviceIntent, IntensityLevel, ScheduledAction};

/// Collision window (seconds) for the preview: two devices firing within this
/// span would read as one coordinated burst, so the planner offsets them.
const COLLISION_WINDOW_SECS: i64 = 2;

/// Default per-device intensity for the preview. Medium keeps the printed plan
/// compact while still demonstrating cross-device aggregation.
const PREVIEW_INTENSITY: IntensityLevel = IntensityLevel::Medium;

/// Build the household plan for `seed` over the local device plus paired peers
/// and print a summary plus the first `limit` scheduled actions.
pub async fn run(config: Config, seed: u64, limit: usize) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    let intents = device_intents(&core).await?;
    let plan = core
        .plan_household(&intents, COLLISION_WINDOW_SECS, seed)
        .await?;

    print_summary(&plan, intents.len());
    for action in plan.iter().take(limit) {
        println!(
            "  {}  device={}  persona={}",
            format_time_of_day(action.at_secs),
            device_label(&action.device_key),
            action.persona_id,
        );
    }
    Ok(())
}

/// Assemble one intent per present device: the local device (empty key) plus
/// every paired peer (its base64url public key). Each device drives its
/// assigned persona if one is set, else a per-device placeholder id so the
/// preview is still meaningful before any assignment exists.
///
/// A device whose persona has a RUNNING campaign previews at the campaign's
/// gap-to-goal intensity (C8 #33 closed loop), so the planned household rate
/// reflects what the campaign is currently driving; otherwise it uses the
/// default preview intensity.
async fn device_intents(core: &Core) -> anyhow::Result<Vec<DeviceIntent>> {
    let mut intents = Vec::new();

    // The local device: empty key per the core's self-device convention.
    let local_persona = persona_for(core, "").await?;
    let local_intensity = campaign_intensity(core, &local_persona).await;
    intents.push(DeviceIntent::new("", local_persona, local_intensity));

    // Every paired peer contributes its own intent.
    for peer in core.paired_peers().await? {
        let persona = persona_for(core, &peer.public_key).await?;
        let intensity = campaign_intensity(core, &persona).await;
        intents.push(DeviceIntent::new(peer.public_key, persona, intensity));
    }
    Ok(intents)
}

/// The intensity a device should preview at: a running campaign's directive
/// intensity for the assigned persona (gap-to-goal driving the household rate),
/// else [`PREVIEW_INTENSITY`]. A placeholder/unassigned persona has no campaign,
/// so it falls back to the default.
async fn campaign_intensity(core: &Core, persona_id: &str) -> IntensityLevel {
    core.campaign_directive_for_persona(persona_id)
        .await
        .ok()
        .and_then(|directive| directive.intensity)
        .unwrap_or(PREVIEW_INTENSITY)
}

/// The persona id assigned to a device, or a stable placeholder derived from
/// the device key when nothing is assigned yet.
async fn persona_for(core: &Core, device_key: &str) -> anyhow::Result<String> {
    Ok(core
        .assigned_persona(device_key)
        .await?
        .unwrap_or_else(|| placeholder_persona(device_key)))
}

/// A readable placeholder persona id for a device with no assignment.
fn placeholder_persona(device_key: &str) -> String {
    if device_key.is_empty() {
        "(local-unassigned)".to_string()
    } else {
        format!("(unassigned:{})", short_key(device_key))
    }
}

/// Print the plan summary: the total action count and the active-window span.
fn print_summary(plan: &[ScheduledAction], device_count: usize) {
    match (plan.first(), plan.last()) {
        (Some(first), Some(last)) => println!(
            "household plan: {} actions across {} device(s), {}-{}",
            plan.len(),
            device_count,
            format_time_of_day(first.at_secs),
            format_time_of_day(last.at_secs),
        ),
        _ => {
            println!("household plan: 0 actions across {device_count} device(s) (no active window)")
        }
    }
}

/// A human label for a device key: `local` for the empty self key, else a short
/// prefix of the base64url public key.
fn device_label(device_key: &str) -> String {
    if device_key.is_empty() {
        "local".to_string()
    } else {
        short_key(device_key)
    }
}

/// The first eight characters of a base64url public key, for compact display.
fn short_key(device_key: &str) -> String {
    device_key.chars().take(8).collect()
}

/// Format a second-of-day offset as `HH:MM:SS` local wall-clock time.
fn format_time_of_day(secs_into_day: i64) -> String {
    let s = secs_into_day.rem_euclid(86_400);
    let (h, m, sec) = (s / 3_600, (s % 3_600) / 60, s % 60);
    format!("{h:02}:{m:02}:{sec:02}")
}
