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

//! `fauxx-cli export`: write an efficacy snapshot to CSV/JSON/PDF (C4 #23).
//!
//! Thin shim over `core.export_efficacy_snapshot`: it builds the artifact in the
//! core (against the persona-intent baseline) and writes the bytes to the
//! requested path. No analytics live here.

use fauxx_core::{Baseline, Config, Core};

use crate::cli::ExportArgs;

/// Build and write the efficacy snapshot for a persona.
pub async fn run(config: Config, args: ExportArgs) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    // The persona-intent baseline measures drift from the persona's declared
    // goal, so fetch the persona to build it (errors if the id is unknown).
    let persona = core.get_persona(&args.persona_id).await?;
    let baseline = Baseline::from_persona(&persona);
    let artifact = core
        .export_efficacy_snapshot(
            &args.persona_id,
            &baseline,
            now_millis(),
            args.format.into(),
        )
        .await?;
    artifact.write_to(&args.out)?;
    println!(
        "wrote {} bytes ({}) to {}",
        artifact.len(),
        artifact.metadata.content_type,
        args.out.display()
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
