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

//! `fauxx-cli drift`: print the per-platform KL-divergence drift for a persona
//! (C4 #20). Thin shim over `core.all_platform_drift`; all KL math lives in the
//! core. The CLI renders the latest scalar per platform (or the full bundle as
//! JSON).

use fauxx_core::{Baseline, Config, Core};

use crate::cli::DriftArgs;

/// Compute and print the per-platform drift bundle for a persona.
pub async fn run(config: Config, args: DriftArgs) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    // The persona-intent baseline measures drift from the persona's declared
    // goal, so fetch the persona to build it (errors if the id is unknown).
    let persona = core.get_persona(&args.persona_id).await?;
    let baseline = Baseline::from_persona(&persona);
    let bundles = core.all_platform_drift(&args.persona_id, &baseline).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&bundles)?);
        return Ok(());
    }
    for bundle in &bundles {
        let platform = bundle.series.platform.label();
        match bundle.series.latest() {
            Some(divergence) => println!(
                "{platform}: drift={divergence:.4} ({} points)",
                bundle.series.points.len()
            ),
            None => println!("{platform}: (no data)"),
        }
    }
    Ok(())
}
