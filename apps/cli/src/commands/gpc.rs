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

//! `fauxx-cli gpc ...`: per-site Global Privacy Control honoring observations
//! (C3 #18).
//!
//! A thin shim over the core's GPC read-back store: the decoy browser / R4
//! extension records whether each visited origin advertised honoring GPC (via
//! its `/.well-known/gpc.json`); this surfaces those observations headlessly
//! (the GUI Privacy hub renders the same data). Read-only: it records nothing.

use fauxx_core::{Config, Core};

use crate::cli::GpcCommand;

/// Dispatch a `gpc` subcommand against a freshly opened core.
pub async fn run(config: Config, command: GpcCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        GpcCommand::List => list(&core).await,
        GpcCommand::Status { origin } => status(&core, &origin).await,
    }
}

/// List every recorded per-site GPC honoring observation.
async fn list(core: &Core) -> anyhow::Result<()> {
    let statuses = core.list_gpc_status().await?;
    if statuses.is_empty() {
        println!("(no gpc observations recorded yet)");
        return Ok(());
    }
    for status in &statuses {
        println!("{}", format_row(&status.origin, status.support.honored));
    }
    Ok(())
}

/// Show the recorded GPC status for one origin.
async fn status(core: &Core, origin: &str) -> anyhow::Result<()> {
    match core.gpc_status_for(origin).await? {
        Some(status) => println!("{}", format_row(&status.origin, status.support.honored)),
        None => println!("(no gpc observation recorded for {origin})"),
    }
    Ok(())
}

/// A one-line, human-readable GPC observation row.
fn format_row(origin: &str, honored: bool) -> String {
    let label = if honored { "honored" } else { "NOT honored" };
    format!("{origin}  gpc={label}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_row_labels_honoring() {
        assert_eq!(
            format_row("https://example.com", true),
            "https://example.com  gpc=honored"
        );
        assert_eq!(
            format_row("https://tracker.test", false),
            "https://tracker.test  gpc=NOT honored"
        );
    }
}
