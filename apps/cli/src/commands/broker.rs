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

//! `fauxx broker ...`: the data-broker opt-out registry and submission tracking
//! (C3 #15). Thin shims over the core broker API; all generation, deadline math,
//! and persistence live in the core.

use std::collections::BTreeMap;

use fauxx_core::{Config, Core};

use crate::cli::BrokerCommand;

/// Dispatch a `broker` subcommand against a freshly opened core.
pub async fn run(config: Config, command: BrokerCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        BrokerCommand::List { json } => list(&core, json),
        BrokerCommand::Generate {
            broker_id,
            persona_id,
            json,
        } => generate(&core, &broker_id, &persona_id, json).await,
        BrokerCommand::Record {
            broker_id,
            persona_id,
        } => record(&core, &broker_id, &persona_id).await,
        BrokerCommand::Submissions { persona, json } => {
            submissions(&core, persona.as_deref(), json).await
        }
        BrokerCommand::DueSoon { json } => due_soon(&core, json).await,
    }
}

/// List the bundled broker registry.
fn list(core: &Core, json: bool) -> anyhow::Result<()> {
    let registry = core.broker_registry();
    if json {
        let entries: BTreeMap<&str, &fauxx_core::BrokerTemplate> =
            registry.iter().map(|(id, t)| (*id, *t)).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if registry.is_empty() {
        println!("(empty registry)");
        return Ok(());
    }
    for (id, template) in &registry {
        println!(
            "{}  {}  method={:?}  fields={}",
            id,
            template.display_name,
            template.method,
            template.required_fields.len()
        );
    }
    Ok(())
}

/// Generate (without recording) a filled opt-out request for review.
async fn generate(
    core: &Core,
    broker_id: &str,
    persona_id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let request = core
        .generate_broker_request(broker_id, persona_id, &BTreeMap::new())
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&request)?);
        return Ok(());
    }
    println!(
        "broker={} persona={} url={} method={:?}",
        request.broker_id, request.persona_id, request.opt_out_url, request.method
    );
    for (key, value) in &request.fields {
        println!("  {key} = {value}");
    }
    if !request.missing_fields.is_empty() {
        println!("  missing: {}", request.missing_fields.join(", "));
    }
    Ok(())
}

/// Generate AND record a drafted submission.
async fn record(core: &Core, broker_id: &str, persona_id: &str) -> anyhow::Result<()> {
    let submission = core.record_broker_submission(broker_id, persona_id).await?;
    println!(
        "recorded submission {} ({}) status={} deadline={}",
        submission.id,
        submission.broker_id,
        submission.status.as_str(),
        submission.deadline
    );
    Ok(())
}

/// List recorded submissions, optionally scoped to a persona.
async fn submissions(core: &Core, persona: Option<&str>, json: bool) -> anyhow::Result<()> {
    let subs = core.list_broker_submissions(persona).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&subs)?);
        return Ok(());
    }
    if subs.is_empty() {
        println!("(no submissions)");
        return Ok(());
    }
    for s in &subs {
        println!(
            "{}  broker={}  persona={}  status={}  deadline={}",
            s.id,
            s.broker_id,
            s.persona_id,
            s.status.as_str(),
            s.deadline
        );
    }
    Ok(())
}

/// List the submissions whose deadline is due or overdue as of now.
async fn due_soon(core: &Core, json: bool) -> anyhow::Result<()> {
    let due = core.due_broker_submissions(now_millis()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&due)?);
        return Ok(());
    }
    if due.is_empty() {
        println!("(nothing due)");
        return Ok(());
    }
    for s in &due {
        println!(
            "{}  broker={}  status={}  deadline={}",
            s.id,
            s.broker_id,
            s.status.as_str(),
            s.deadline
        );
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
