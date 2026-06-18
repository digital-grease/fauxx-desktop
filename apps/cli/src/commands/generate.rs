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

//! `fauxx-cli generate`: run a generation pass producing signed artifacts (C6 #28).
//!
//! Thin shim over `core.generate_signed_artifacts` / `core.run_generation_pass`.
//! The heavy work (the adversarial-allocation weight map and the weight-map-
//! biased query plan, both signed) runs in the core; the CLI selects whether to
//! push the artifacts to paired peers.

use fauxx_core::{Config, Core, GeneratedArtifacts, DEFAULT_FRESHNESS_MS};

use crate::cli::GenerateArgs;

/// Run the generation pass for a persona, optionally pushing to paired peers.
pub async fn run(config: Config, args: GenerateArgs) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    if args.push {
        let outcome = core
            .run_generation_pass(
                &args.persona_id,
                args.intensity.into(),
                args.seed,
                DEFAULT_FRESHNESS_MS,
            )
            .await?;
        print_artifacts(&outcome.artifacts);
        println!("pushed to {} paired peer(s)", outcome.peers_reached);
    } else {
        let artifacts = core
            .generate_signed_artifacts(
                &args.persona_id,
                args.intensity.into(),
                args.seed,
                DEFAULT_FRESHNESS_MS,
            )
            .await?;
        print_artifacts(&artifacts);
    }
    Ok(())
}

/// Print a one-line summary of each signed artifact.
fn print_artifacts(artifacts: &GeneratedArtifacts) {
    println!(
        "weight-map: persona={} expires_at={} signer={}",
        artifacts.weight_map.content.persona_id,
        artifacts.weight_map.content.expires_at,
        artifacts.weight_map.signer_public_key
    );
    println!(
        "query-plan: persona={} expires_at={} signer={}",
        artifacts.query_plan.content.persona_id,
        artifacts.query_plan.content.expires_at,
        artifacts.query_plan.signer_public_key
    );
}
