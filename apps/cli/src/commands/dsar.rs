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

//! `fauxx-cli dsar ...`: GDPR/CCPA Data Subject Access Request letters (C3 #16).
//!
//! Thin shims over the core DSAR API. Letters are GENERATED and TRACKED here;
//! nothing is ever auto-sent (the core renders text for manual sending).

use anyhow::bail;
use fauxx_core::{Config, Controller, Core, SubjectDetails};

use crate::cli::{DsarCommand, DsarRequestArgs};

/// Dispatch a `dsar` subcommand against a freshly opened core.
pub async fn run(config: Config, command: DsarCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        DsarCommand::Generate(args) => generate(&core, args).await,
        DsarCommand::Record(args) => record(&core, args).await,
        DsarCommand::List { persona, json } => list(&core, persona.as_deref(), json).await,
        DsarCommand::Export {
            request_id,
            name,
            contact,
        } => export(&core, &request_id, &name, &contact).await,
        DsarCommand::Overdue { json } => overdue(&core, json).await,
        DsarCommand::Sent { request_id } => mark_sent(&core, &request_id).await,
    }
}

/// Resolve the controller from the request args: a known broker, or an arbitrary
/// named controller. Exactly one must be supplied.
fn resolve_controller(args: &DsarRequestArgs) -> anyhow::Result<Controller> {
    match (&args.broker, &args.controller_name) {
        (Some(broker_id), None) => Ok(Controller::resolve_broker(broker_id)?),
        (None, Some(name)) => Ok(Controller::arbitrary(
            name.clone(),
            args.controller_contact.clone(),
        )),
        (None, None) => bail!("supply either --broker or --controller-name"),
        (Some(_), Some(_)) => bail!("--broker and --controller-name are mutually exclusive"),
    }
}

/// Generate (without recording) a request and print its drafted record.
async fn generate(core: &Core, args: DsarRequestArgs) -> anyhow::Result<()> {
    let controller = resolve_controller(&args)?;
    let request = core
        .generate_dsar_request(args.kind.into(), &args.persona_id, controller)
        .await?;
    println!("{}", serde_json::to_string_pretty(&request)?);
    Ok(())
}

/// Generate AND record a drafted request.
async fn record(core: &Core, args: DsarRequestArgs) -> anyhow::Result<()> {
    let controller = resolve_controller(&args)?;
    let request = core
        .record_dsar_request(args.kind.into(), &args.persona_id, controller)
        .await?;
    println!(
        "recorded dsar {} ({}) controller={} status={}",
        request.id,
        request.kind.label(),
        request.controller.name,
        request.status.as_str()
    );
    Ok(())
}

/// List recorded requests, optionally scoped to a persona.
async fn list(core: &Core, persona: Option<&str>, json: bool) -> anyhow::Result<()> {
    let requests = core.list_dsar_requests(persona).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&requests)?);
        return Ok(());
    }
    if requests.is_empty() {
        println!("(no dsar requests)");
        return Ok(());
    }
    for r in &requests {
        println!(
            "{}  {}  controller={}  status={}  persona={}",
            r.id,
            r.kind.as_str(),
            r.controller.name,
            r.status.as_str(),
            r.persona_id
        );
    }
    Ok(())
}

/// Render a recorded request's letter text for manual sending.
async fn export(core: &Core, request_id: &str, name: &str, contact: &str) -> anyhow::Result<()> {
    let subject = SubjectDetails::new(name).with_reply_to(contact);
    let letter = core.export_dsar_letter(request_id, &subject).await?;
    println!("Subject: {}", letter.subject);
    println!();
    println!("{}", letter.body);
    Ok(())
}

/// List the requests that are overdue as of now.
async fn overdue(core: &Core, json: bool) -> anyhow::Result<()> {
    let overdue = core.overdue_dsar_requests(now_millis()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&overdue)?);
        return Ok(());
    }
    if overdue.is_empty() {
        println!("(nothing overdue)");
        return Ok(());
    }
    for r in &overdue {
        println!(
            "{}  {}  controller={}  deadline={:?}",
            r.id,
            r.kind.as_str(),
            r.controller.name,
            r.deadline
        );
    }
    Ok(())
}

/// Mark a recorded request as sent NOW, which starts its statutory deadline
/// clock (until then, no deadline is tracked). The core stamps `sent_at` and
/// advances the lifecycle status.
async fn mark_sent(core: &Core, request_id: &str) -> anyhow::Result<()> {
    let request = core.mark_dsar_sent(request_id, now_millis()).await?;
    println!(
        "marked dsar {} sent at {} (status={})",
        request.id,
        request.sent_at.unwrap_or(0),
        request.status.as_str()
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
