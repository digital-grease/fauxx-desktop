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

//! `fauxx mint`: mint N coherent PUMS personas into a signed pack (C6 #29).
//!
//! Thin shim over `core.mint_persona_pack` / `core.mint_and_push_pack`. The
//! PUMS draw, coherence re-sampling, and signing all run in the core; the CLI
//! writes the signed pack bytes to a path and optionally pushes to paired peers.

use std::path::Path;

use anyhow::Context;
use fauxx_core::{Config, Core};

use crate::cli::MintArgs;

/// Mint the requested personas, write the signed pack, optionally push.
pub async fn run(config: Config, args: MintArgs) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    if args.push {
        let outcome = core.mint_and_push_pack(args.count, args.seed).await?;
        write_pack(&args.out, &outcome.pack_bytes)?;
        println!(
            "minted {} persona(s) -> {} ({} bytes), pushed to {} peer(s)",
            args.count,
            args.out.display(),
            outcome.pack_bytes.len(),
            outcome.peers_reached
        );
    } else {
        let bytes = core.mint_persona_pack(args.count, args.seed).await?;
        write_pack(&args.out, &bytes)?;
        println!(
            "minted {} persona(s) -> {} ({} bytes)",
            args.count,
            args.out.display(),
            bytes.len()
        );
    }
    Ok(())
}

/// Write signed pack bytes to a path.
fn write_pack(out: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::write(out, bytes).with_context(|| format!("writing pack to {}", out.display()))
}
