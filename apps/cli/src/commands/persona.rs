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

//! `fauxx-cli persona ...`: list, show, add, and delete personas.
//!
//! These are thin shims over the core persona API. `add` is the minimal write
//! path that makes list/show demonstrable end to end; full persona management
//! lands in C5. Validation here only warns (it never rewrites the wire form),
//! mirroring the core's lossless round-trip rule.

use std::io::Read as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use fauxx_core::{Config, Core, CoreError, SyntheticPersona};

use crate::cli::{PersonaAddArgs, PersonaCommand};

/// Nine days in milliseconds: the default persona active-until window.
const ACTIVE_WINDOW_MS: i64 = 9 * 24 * 60 * 60 * 1000;

/// Dispatch a `persona` subcommand against a freshly opened core.
pub async fn run(config: Config, command: PersonaCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        PersonaCommand::List { json } => list(&core, json).await,
        PersonaCommand::Show { id } => show(&core, &id).await,
        PersonaCommand::Add(args) => add(&core, args).await,
        PersonaCommand::Delete { id } => delete(&core, &id).await,
    }
}

/// Print each persona as a summary line, or the full list as JSON.
async fn list(core: &Core, json: bool) -> anyhow::Result<()> {
    let personas = core.list_personas().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&personas)?);
        return Ok(());
    }
    if personas.is_empty() {
        println!("(no personas)");
        return Ok(());
    }
    for p in &personas {
        println!(
            "{}  {}  region={}  interests={}",
            p.id,
            p.name,
            p.region,
            p.interests.len()
        );
    }
    Ok(())
}

/// Pretty-print a single persona as JSON. A missing persona is a clear error
/// (the caller maps it to exit code 1).
async fn show(core: &Core, id: &str) -> anyhow::Result<()> {
    let persona = core.get_persona(id).await.map_err(|err| match err {
        CoreError::NotFound(_) => anyhow::anyhow!("persona {id} not found"),
        other => anyhow::Error::from(other),
    })?;
    println!("{}", serde_json::to_string_pretty(&persona)?);
    Ok(())
}

/// Build a persona (from JSON or from flags), warn on validation issues, then
/// save it.
async fn add(core: &Core, args: PersonaAddArgs) -> anyhow::Result<()> {
    let persona = build_persona(args)?;

    for issue in persona.validate() {
        tracing::warn!("persona validation issue: {issue:?}");
        eprintln!("warning: persona validation issue: {issue:?}");
    }

    core.save_persona(&persona).await?;
    println!("added persona {} ({})", persona.id, persona.name);
    Ok(())
}

/// Delete a persona by id. A missing persona is a clear error.
async fn delete(core: &Core, id: &str) -> anyhow::Result<()> {
    core.delete_persona(id).await.map_err(|err| match err {
        CoreError::NotFound(_) => anyhow::anyhow!("persona {id} not found"),
        other => anyhow::Error::from(other),
    })?;
    println!("deleted persona {id}");
    Ok(())
}

/// Construct a [`SyntheticPersona`] from the add arguments. Prefers the JSON
/// document when `--from-json` is given; otherwise assembles one from the
/// individual field flags with `created_at = now` and a nine-day window.
fn build_persona(args: PersonaAddArgs) -> anyhow::Result<SyntheticPersona> {
    if let Some(source) = args.from_json {
        return persona_from_json(&source);
    }

    // clap's `required_unless_present = "from_json"` guarantees these are set
    // on this branch, but we still error explicitly rather than unwrap.
    let name = args.name.context("missing --name")?;
    let age_range = args.age_range.context("missing --age-range")?;
    let profession = args.profession.context("missing --profession")?;
    let region = args.region.context("missing --region")?;
    if args.interests.is_empty() {
        bail!("missing --interests");
    }

    let created_at = now_millis();
    let active_until = created_at.saturating_add(ACTIVE_WINDOW_MS);
    let id = args.id.unwrap_or_else(new_uuid);

    let mut persona = SyntheticPersona::new(
        id,
        name,
        age_range,
        profession,
        region,
        args.interests,
        created_at,
        active_until,
    );
    persona.note = args.note;
    Ok(persona)
}

/// Read and parse a persona JSON document from a file path, or from stdin when
/// the path is `-`.
fn persona_from_json(source: &Path) -> anyhow::Result<SyntheticPersona> {
    let raw = if source.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading persona JSON from stdin")?;
        buf
    } else {
        std::fs::read_to_string(source)
            .with_context(|| format!("reading persona JSON from {}", source.display()))?
    };
    serde_json::from_str(&raw).context("parsing persona JSON")
}

/// Current wall-clock time in epoch milliseconds (0 if the clock predates the
/// epoch, which cannot happen on a sane host).
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A fresh random UUID v4 string for a new persona id.
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
