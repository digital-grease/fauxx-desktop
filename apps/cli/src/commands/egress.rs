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

//! `fauxx egress ...`: per-persona network egress (C7 #30).
//!
//! Thin shims over the core egress API. The egress routing config persists in
//! the encrypted store; the exit indicator (and its fail-closed pause state) is
//! computed in the core. Proxy CREDENTIALS are out of scope for this CLI surface
//! (they live in the OS keystore via a dedicated core call).

use anyhow::{bail, Context};
use fauxx_core::{Config, Core, Egress};

use crate::cli::{EgressCommand, EgressKindArg};

/// Dispatch an `egress` subcommand against a freshly opened core.
pub async fn run(config: Config, command: EgressCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        EgressCommand::Set {
            persona_id,
            kind,
            host,
            port,
            socks_addr,
        } => set(&core, &persona_id, kind, host, port, socks_addr).await,
        EgressCommand::Get { persona_id, json } => get(&core, &persona_id, json).await,
        EgressCommand::Clear { persona_id } => clear(&core, &persona_id).await,
    }
}

/// Build the [`Egress`] from the CLI flags for a kind, failing closed on a
/// missing host/port where the kind requires it.
fn build_egress(
    kind: EgressKindArg,
    host: Option<String>,
    port: Option<u16>,
    socks_addr: Option<String>,
) -> anyhow::Result<Egress> {
    match kind {
        EgressKindArg::Direct => Ok(Egress::Direct),
        EgressKindArg::Http => {
            let host = host.context("--host is required for an http egress")?;
            let port = port.context("--port is required for an http egress")?;
            Ok(Egress::http_proxy(host, port))
        }
        EgressKindArg::Socks => {
            let host = host.context("--host is required for a socks egress")?;
            let port = port.context("--port is required for a socks egress")?;
            Ok(Egress::socks_proxy(host, port))
        }
        EgressKindArg::Tor => match socks_addr {
            Some(addr) => Ok(Egress::tor_at(addr)),
            None => Ok(Egress::tor()),
        },
    }
}

/// Bind a per-persona egress.
async fn set(
    core: &Core,
    persona_id: &str,
    kind: EgressKindArg,
    host: Option<String>,
    port: Option<u16>,
    socks_addr: Option<String>,
) -> anyhow::Result<()> {
    let egress = build_egress(kind, host, port, socks_addr)?;
    let label = egress.exit_label();
    core.set_persona_egress(persona_id, egress).await?;
    println!("set egress for {persona_id}: {label}");
    Ok(())
}

/// Show a persona's bound egress and its live exit indicator.
async fn get(core: &Core, persona_id: &str, json: bool) -> anyhow::Result<()> {
    let exit = core.persona_egress_exit_live(persona_id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&exit)?);
        return Ok(());
    }
    println!(
        "persona={} exit={} reachable={} paused={}",
        exit.persona_id, exit.label, exit.reachable, exit.paused
    );
    if let Some(reason) = &exit.paused_reason {
        println!("  {reason}");
    }
    Ok(())
}

/// Clear a persona's egress binding (revert to direct).
async fn clear(core: &Core, persona_id: &str) -> anyhow::Result<()> {
    if core.clear_persona_egress(persona_id).await? {
        println!("cleared egress for {persona_id} (now direct)");
        Ok(())
    } else {
        bail!("no egress binding for persona {persona_id}");
    }
}
