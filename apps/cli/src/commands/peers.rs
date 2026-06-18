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

//! `fauxx peers` and `fauxx unpair`: list and revoke cross-device peers.
//!
//! `peers` lists paired (trusted) peers by default, or mDNS-discovered
//! (untrusted) peers with `--discovered`; `--json` emits the structured list.
//! `unpair` revokes a paired peer by its base64url public key; a key that is
//! not paired is a runtime error (exit 1).

use anyhow::bail;
use fauxx_core::{Config, Core};

/// List paired peers, or discovered peers when `discovered` is set. With `json`
/// the structured list is emitted instead of a summary table.
pub async fn run(config: Config, discovered: bool, json: bool) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    if discovered {
        list_discovered(&core, json).await
    } else {
        list_paired(&core, json).await
    }
}

/// Print paired peers as summary lines, or the full list as JSON.
async fn list_paired(core: &Core, json: bool) -> anyhow::Result<()> {
    let peers = core.paired_peers().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&peers)?);
        return Ok(());
    }
    if peers.is_empty() {
        println!("(no paired peers)");
        return Ok(());
    }
    for p in &peers {
        let host = p.host.as_deref().unwrap_or("?");
        println!(
            "{}  {}  {}:{}  paired_at={}",
            p.name, p.fingerprint, host, p.port, p.paired_at
        );
    }
    Ok(())
}

/// Print mDNS-discovered peers as summary lines, or the full list as JSON.
async fn list_discovered(core: &Core, json: bool) -> anyhow::Result<()> {
    let peers = core.discovered_peers().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&peers)?);
        return Ok(());
    }
    if peers.is_empty() {
        println!("(no discovered peers)");
        return Ok(());
    }
    for p in &peers {
        let fp = p.fingerprint.as_deref().unwrap_or("?");
        let addrs = p.addresses.join(",");
        println!("{}  {}  [{}]  port={}", p.name, fp, addrs, p.port);
    }
    Ok(())
}

/// Revoke a paired peer by its base64url public key. A key that matched no
/// paired record is a runtime error (exit 1).
pub async fn unpair(config: Config, public_key: &str) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    if core.unpair(public_key).await? {
        println!("unpaired {public_key}");
        Ok(())
    } else {
        bail!("no paired peer with public key {public_key}");
    }
}
