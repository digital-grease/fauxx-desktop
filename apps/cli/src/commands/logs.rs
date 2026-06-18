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

//! `fauxx-cli logs ...`: the bug-report path. Show where the persisted debug logs
//! live, export a SCRUBBED copy to attach to a GitHub issue, or clear them.
//!
//! The scrubbing happens at export time (the on-disk log keeps full fidelity for
//! the user's own debugging). The redaction set is the fixed pattern policy in
//! `fauxx_core::logging` PLUS the live persona ids/names and the home directory,
//! so a persona's display name and the operator's username never reach a public
//! issue.

use std::path::PathBuf;

use anyhow::Context;
use fauxx_core::{logging, Config, Core};

use crate::cli::LogsCommand;

/// Dispatch a `logs` subcommand.
pub async fn run(config: Config, command: LogsCommand) -> anyhow::Result<()> {
    match command {
        LogsCommand::Path => {
            println!("{}", logging::log_dir()?.display());
            Ok(())
        }
        LogsCommand::Clear => clear(),
        LogsCommand::Export { out } => export(config, out).await,
    }
}

/// Export a scrubbed, shareable copy of the debug logs.
async fn export(config: Config, out: Option<PathBuf>) -> anyhow::Result<()> {
    // Open the core to learn the live literals to redact (persona/device/peer
    // names, egress hosts, account ids). A store that cannot open is not fatal:
    // we still scrub via the pattern policy + the local account literals.
    let literals = match Core::open(config).await {
        Ok(core) => core.redaction_literals().await,
        Err(e) => {
            eprintln!("note: could not open the store to redact persona/peer names ({e}); exporting with the pattern policy + account literals only");
            logging::account_literals()
        }
    };

    let redactions = logging::Redactions::new(literals)?;
    let out_path = match out {
        Some(p) => p,
        None => std::env::current_dir()
            .context("resolving the current directory for the default export path")?
            .join("fauxx-debug-log.txt"),
    };
    let summary = logging::export(&redactions, &logging::diagnostics_header(), &out_path)?;

    println!("wrote scrubbed debug log: {}", summary.out_path.display());
    println!(
        "  {} file(s), {} line(s); paths/IPs/emails/keys/ids/persona names redacted",
        summary.files, summary.lines
    );
    println!("  review it, then attach it to your GitHub issue.");
    Ok(())
}

/// Delete every persisted debug log file in the log directory.
fn clear() -> anyhow::Result<()> {
    let dir = logging::log_dir()?;
    let mut removed = 0usize;
    for entry in
        std::fs::read_dir(&dir).with_context(|| format!("reading log dir {}", dir.display()))?
    {
        let path = entry?.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(logging::LOG_FILE_PREFIX) {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                removed += 1;
            }
        }
    }
    println!("cleared {removed} debug log file(s) from {}", dir.display());
    Ok(())
}
