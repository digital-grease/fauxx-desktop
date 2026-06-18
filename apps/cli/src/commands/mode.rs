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

//! `fauxx mode` (show) and `fauxx mode set <coherent|fragmentation>`.
//!
//! Thin shims over the core coordination-mode API. The mode persists in the
//! encrypted store, so a `set` survives across process invocations against the
//! same `--db`. The lowercase short forms map to [`CoordinationMode`] in the
//! clap layer (see [`crate::cli::ModeArg`]).

use fauxx_core::{Config, CoordinationMode, Core};

/// Print the active coordination mode (its stable string form).
pub async fn show(config: Config) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    let mode = core.coordination_mode().await?;
    println!("{mode}");
    Ok(())
}

/// Set the active coordination mode (persisted) and confirm.
pub async fn set(config: Config, mode: CoordinationMode) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    core.set_coordination_mode(mode).await?;
    println!("mode set to {mode}");
    Ok(())
}
