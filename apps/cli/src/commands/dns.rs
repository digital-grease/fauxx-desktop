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

//! `fauxx-cli dns ...`: per-persona DNS strategy (C7 #31).
//!
//! Thin shims over the core DNS API. The strategy persists in the encrypted
//! store; the EXPLICIT observer trade-off note (who sees this persona's lookups)
//! is computed in the core and always surfaced.

use anyhow::Context;
use fauxx_core::{Config, Core, DnsStrategy};

use crate::cli::{DnsCommand, DnsModeArg};

/// Dispatch a `dns` subcommand against a freshly opened core.
pub async fn run(config: Config, command: DnsCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        DnsCommand::Set {
            persona_id,
            mode,
            resolver,
        } => set(&core, &persona_id, mode, resolver).await,
        DnsCommand::Get { persona_id, json } => get(&core, &persona_id, json).await,
        DnsCommand::Verify { persona_id } => verify(&core, &persona_id).await,
    }
}

/// Build the [`DnsStrategy`] from the CLI flags, failing closed when a doh/dot
/// mode lacks a resolver.
fn build_dns(mode: DnsModeArg, resolver: Option<String>) -> anyhow::Result<DnsStrategy> {
    match mode {
        DnsModeArg::System => Ok(DnsStrategy::SystemDefault),
        DnsModeArg::Doh => {
            let resolver = resolver.context("--resolver is required for a doh strategy")?;
            Ok(DnsStrategy::doh(resolver))
        }
        DnsModeArg::Dot => {
            let resolver = resolver.context("--resolver is required for a dot strategy")?;
            Ok(DnsStrategy::dot(resolver))
        }
    }
}

/// Bind a per-persona DNS strategy.
async fn set(
    core: &Core,
    persona_id: &str,
    mode: DnsModeArg,
    resolver: Option<String>,
) -> anyhow::Result<()> {
    let dns = build_dns(mode, resolver)?;
    core.set_persona_dns(persona_id, dns).await?;
    println!("set DNS for {persona_id}");
    println!("  {}", core.persona_dns_observer_note(persona_id).await?);
    Ok(())
}

/// Show a persona's bound DNS strategy and its observer note.
async fn get(core: &Core, persona_id: &str, json: bool) -> anyhow::Result<()> {
    let dns = core.get_persona_dns(persona_id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&dns)?);
        return Ok(());
    }
    match dns.resolver() {
        Some(resolver) => println!("persona={persona_id} resolver={resolver}"),
        None => println!("persona={persona_id} resolver=(system default)"),
    }
    println!("  {}", core.persona_dns_observer_note(persona_id).await?);
    Ok(())
}

/// Verify the secure-DNS routing a persona's decoy launch applies (C7 #31): the
/// strategy, resolver, and the exact Chromium secure-DNS flags. This is a
/// CONFIG-application check (what the decoy launch does), so it is deterministic
/// and needs no network; a live resolver round-trip is the decoy-browser path
/// (the non-browser fetch path is out of scope by design).
async fn verify(core: &Core, persona_id: &str) -> anyhow::Result<()> {
    let dns = core.get_persona_dns(persona_id).await?;
    println!("DNS verification for persona {persona_id}:");
    match dns.resolver() {
        Some(resolver) => {
            println!("  strategy: secure DNS via {resolver}");
            println!("  decoy launch applies these secure-DNS flags:");
            for arg in dns.chromium_dns_args() {
                println!("    {arg}");
            }
            println!(
                "  => the decoy's lookups for this persona are configured to resolve over \
                 {resolver}, not the system resolver."
            );
        }
        None => {
            println!("  strategy: system default (no secure-DNS override)");
            println!("  => the decoy's lookups use the OS resolver; no per-persona DNS isolation.");
        }
    }
    println!(
        "  observer note: {}",
        core.persona_dns_observer_note(persona_id).await?
    );
    println!(
        "  (config-application check: confirms the decoy is set to route DNS as above; a live \
         resolver round-trip needs the decoy browser + network)"
    );
    Ok(())
}
