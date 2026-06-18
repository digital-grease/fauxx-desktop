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

//! `fauxx-cli alias ...`: per-persona email alias management (C3 #17).
//!
//! Thin shims over the core alias API. The only built-in provider is the local,
//! network-free plus-address provider; the no-reuse-across-sites rule and all
//! persistence live in the core.

use fauxx_core::{AliasKind, Config, Core, PlusAddressProvider};

use crate::cli::AliasCommand;

/// Dispatch an `alias` subcommand against a freshly opened core.
pub async fn run(config: Config, command: AliasCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        AliasCommand::Mint {
            persona_id,
            site,
            base,
            allow_reuse,
        } => mint(&core, &persona_id, &site, &base, allow_reuse).await,
        AliasCommand::Record {
            persona_id,
            site,
            address,
            allow_reuse,
        } => record(&core, &persona_id, &site, &address, allow_reuse).await,
        AliasCommand::List { persona, json } => list(&core, persona.as_deref(), json).await,
        AliasCommand::Revoke { alias_id } => revoke(&core, &alias_id).await,
        AliasCommand::Rotate { alias_id, base } => rotate(&core, &alias_id, &base).await,
    }
}

/// Mint a fresh plus-address alias for a persona/site.
async fn mint(
    core: &Core,
    persona_id: &str,
    site: &str,
    base: &str,
    allow_reuse: bool,
) -> anyhow::Result<()> {
    let provider = PlusAddressProvider::new(base)?;
    let alias = core
        .mint_email_alias(&provider, persona_id, site, allow_reuse)
        .await?;
    println!("minted alias {} -> {}", alias.id, alias.address);
    Ok(())
}

/// Record a manually-created alias.
async fn record(
    core: &Core,
    persona_id: &str,
    site: &str,
    address: &str,
    allow_reuse: bool,
) -> anyhow::Result<()> {
    let alias = core
        .record_email_alias(
            persona_id,
            site,
            address,
            AliasKind::Masked,
            None,
            allow_reuse,
        )
        .await?;
    println!("recorded alias {} -> {}", alias.id, alias.address);
    Ok(())
}

/// List aliases, optionally scoped to a persona.
async fn list(core: &Core, persona: Option<&str>, json: bool) -> anyhow::Result<()> {
    let aliases = core.list_email_aliases(persona).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&aliases)?);
        return Ok(());
    }
    if aliases.is_empty() {
        println!("(no aliases)");
        return Ok(());
    }
    for a in &aliases {
        println!(
            "{}  {}  site={}  persona={}  status={}",
            a.id,
            a.address,
            a.site,
            a.persona_id,
            a.status.as_str()
        );
    }
    Ok(())
}

/// Revoke an alias by id.
async fn revoke(core: &Core, alias_id: &str) -> anyhow::Result<()> {
    let alias = core.revoke_email_alias(alias_id).await?;
    println!("revoked alias {} ({})", alias.id, alias.address);
    Ok(())
}

/// Rotate an alias: revoke the old one and mint a fresh plus-address.
async fn rotate(core: &Core, alias_id: &str, base: &str) -> anyhow::Result<()> {
    let provider = PlusAddressProvider::new(base)?;
    let fresh = core.rotate_email_alias(alias_id, &provider).await?;
    println!("rotated to fresh alias {} -> {}", fresh.id, fresh.address);
    Ok(())
}
