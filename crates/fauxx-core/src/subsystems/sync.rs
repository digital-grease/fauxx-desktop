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

//! Cross-device sync subsystem seam.
//!
//! Reconciles personas with the Android app over a local, peer-to-peer channel
//! (no cloud, no telemetry). The real implementation landed in C1 #7 as the
//! [`crate::sync`] module (the [`LanSync`](crate::sync::LanSync) engine, reached
//! through [`Core`](crate::Core)'s async sync accessors). This trait remains the
//! abstract push/pull seam other subsystems (the scheduler) can depend on
//! without taking on the full engine; its default methods still report
//! [`CoreError::Unimplemented`] until the scheduler wiring uses them.

use async_trait::async_trait;

use crate::error::{CoreError, Result};

/// Reconciles persona state with a paired device.
///
/// Named `SyncEngine` (not `Sync`) to avoid colliding with the
/// [`std::marker::Sync`] auto-trait that appears in its own `Send + Sync`
/// bound.
#[async_trait]
pub trait SyncEngine: Send + Sync {
    /// Push local personas to the paired peer.
    async fn push(&self) -> Result<()> {
        Err(CoreError::Unimplemented("sync::push"))
    }

    /// Pull personas from the paired peer.
    async fn pull(&self) -> Result<()> {
        Err(CoreError::Unimplemented("sync::pull"))
    }
}

/// No-op sync used until the real one lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubSync;

#[async_trait]
impl SyncEngine for StubSync {}
