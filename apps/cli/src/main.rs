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

//! `fauxx-cli`: the CLI / headless entrypoint, a thin client over `fauxx-core`.
//!
//! Because the core does all real work, the headless mode falls out of the
//! architecture for free. This is the C0 #4 foundation: a clap-derive CLI with
//! `run` (the headless agent skeleton), `status`, and a `persona` group
//! (list/show/add/delete), extended by the C1 cross-device surface (`pair`,
//! `peers`, `unpair`, `mode`, `schedule`) so the sync/coordination API is
//! reachable headlessly. The 24/7 homelab serve mode lands in C8 #35 and full
//! persona management in C5. The binary links no GUI dependencies.
//!
//! Exit codes: `0` success, `1` a runtime error (store/IO/core failure or a
//! missing persona), `2` a usage error (bad flags). clap already exits with `2`
//! on its own parse failures.

#![forbid(unsafe_code)]

mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command, ModeCommand, PairCommand};

// The C8 #35 serve mode and the C3 dsar group own their own exit-code split
// in places; the rest are plain runtime results wrapped below.

/// Exit code for a usage error (bad flags), matching clap's own convention.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    // Stderr logging (RUST_LOG, default info) PLUS a persisted, rotating debug
    // log file and a crash-capturing panic hook; `fauxx-cli logs export` ships it,
    // scrubbed, to a bug report. See fauxx_core::logging.
    fauxx_core::logging::init();

    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(err)) => {
            eprintln!("error: {err:#}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(Failure::Runtime(err)) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// A CLI failure, split so `main` can map it to the right exit code. Visible to
/// the command handlers so a command that owns its own exit-code split (e.g.
/// `pair add`, where a bad scanned payload is a usage error) can return it.
pub(crate) enum Failure {
    /// A usage error (e.g. flags that do not name a usable passphrase, or a
    /// malformed scanned pairing payload): exit 2.
    Usage(anyhow::Error),
    /// A runtime error (store/IO/core failure, missing persona): exit 1.
    Runtime(anyhow::Error),
}

/// `#[tokio::main]` turns this into a synchronous call that drives the runtime,
/// so `main` stays a plain `fn` returning an exit code. A multi-thread runtime
/// is used so `fauxx-cli run` can host background work in later milestones.
#[tokio::main]
async fn run(cli: Cli) -> std::result::Result<(), Failure> {
    // `serve` resolves its OWN store config from its config file (the headless
    // homelab mode), so it does not consume the global store flags. Dispatch it
    // before resolving the global config so those flags stay optional for it.
    if let Command::Serve(args) = cli.command {
        return commands::serve::run(args).await.map_err(Failure::Runtime);
    }

    // Resolve the store config first. A failure here is a usage error (the
    // flags did not name a usable passphrase / key file).
    let config = cli.store.to_config().map_err(Failure::Usage)?;

    // `pair` owns its own exit-code split: a malformed/old-version scanned
    // payload is a usage error (exit 2), while everything else here is a runtime
    // error (exit 1). So it returns the already-classified `Failure` directly;
    // the remaining commands return a plain runtime result we wrap below.
    let result = match cli.command {
        Command::Run => commands::run::run(config).await,
        Command::Search {
            persona_id,
            decoy_id,
            json,
        } => commands::search::run(config, persona_id, decoy_id, json).await,
        Command::Status { json } => commands::status::run(config, json).await,
        Command::Persona { command } => commands::persona::run(config, command).await,
        Command::Pair { command } => return dispatch_pair(config, command).await,
        Command::Peers { discovered, json } => commands::peers::run(config, discovered, json).await,
        Command::Unpair { public_key } => commands::peers::unpair(config, &public_key).await,
        Command::Mode { command } => dispatch_mode(config, command).await,
        Command::Schedule { seed, limit } => commands::schedule::run(config, seed, limit).await,
        Command::Broker { command } => commands::broker::run(config, command).await,
        Command::Dsar { command } => commands::dsar::run(config, command).await,
        Command::Alias { command } => commands::alias::run(config, command).await,
        Command::Anchor { command } => commands::anchor::run(config, command).await,
        Command::Gpc { command } => commands::gpc::run(config, command).await,
        Command::Export(args) => commands::export::run(config, args).await,
        Command::Ab { command } => commands::ab::run(config, command).await,
        Command::Drift(args) => commands::drift::run(config, args).await,
        Command::Pack { command } => commands::pack::run(config, command).await,
        Command::Generate(args) => commands::generate::run(config, args).await,
        Command::Mint(args) => commands::mint::run(config, args).await,
        Command::Egress { command } => commands::egress::run(config, command).await,
        Command::Dns { command } => commands::dns::run(config, command).await,
        Command::Campaign { command } => commands::campaign::run(config, command).await,
        Command::Logs { command } => commands::logs::run(config, command).await,
        Command::NativeHost => commands::native_host::run(config).await,
        // Handled above (resolves its own config); unreachable here.
        Command::Serve(_) => unreachable!("serve is dispatched before global config resolution"),
    };
    result.map_err(Failure::Runtime)
}

/// Dispatch a `pair` subcommand, classifying a bad scanned payload as a usage
/// error (exit 2) and any other failure as a runtime error (exit 1).
async fn dispatch_pair(
    config: fauxx_core::Config,
    command: PairCommand,
) -> std::result::Result<(), Failure> {
    match command {
        PairCommand::Show => commands::pair::show(config).await.map_err(Failure::Runtime),
        PairCommand::Add { payload } => commands::pair::add(config, &payload).await,
    }
}

/// Dispatch a `mode` subcommand (show when no `set` subcommand is given).
async fn dispatch_mode(
    config: fauxx_core::Config,
    command: Option<ModeCommand>,
) -> anyhow::Result<()> {
    match command {
        None | Some(ModeCommand::Show) => commands::mode::show(config).await,
        Some(ModeCommand::Set { mode }) => commands::mode::set(config, mode.into()).await,
    }
}
