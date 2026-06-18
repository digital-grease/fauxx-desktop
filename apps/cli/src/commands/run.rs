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

//! `fauxx-cli run`: the headless agent entrypoint skeleton.
//!
//! Opens the core, logs that the agent is running, then holds the core open
//! until Ctrl-C (SIGINT). There is no scheduler yet (it is a fauxx-core stub),
//! so this just keeps the store attached and exits cleanly. C8 #35 hardens
//! this into a long-running serve mode.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use fauxx_core::Config;
use tokio::sync::Notify;

/// Open the core and block until SIGINT, then shut down cleanly.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let core = fauxx_core::Core::open(config).await?;
    let status = core.status().await?;
    tracing::info!(
        version = status.version,
        persona_count = status.persona_count,
        "agent running (Ctrl-C to stop)"
    );

    // ctrlc fires its handler on a separate thread, so it flips the shared flag
    // and wakes the async waiter via a tokio Notify. There is no scheduler to
    // drive yet, so the core is simply held open for the lifetime of the run.
    let shutdown = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    let handler_flag = Arc::clone(&shutdown);
    let handler_notify = Arc::clone(&notify);
    ctrlc::set_handler(move || {
        handler_flag.store(true, Ordering::SeqCst);
        handler_notify.notify_one();
    })
    .context("installing Ctrl-C handler")?;

    // Park until the handler signals shutdown. The flag guards against a signal
    // delivered before this await is reached.
    while !shutdown.load(Ordering::SeqCst) {
        notify.notified().await;
    }

    tracing::info!("shutdown signal received, stopping agent");
    // Dropping `core` here releases the store; explicit for clarity.
    drop(core);
    Ok(())
}
