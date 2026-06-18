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

//! Scheduler subsystem (stub).
//!
//! Drives persona rotation and the cadence of synthetic browsing sessions. The
//! real implementation arrives in a later milestone; for C0 the trait pins the
//! seam and the default methods report [`CoreError::Unimplemented`].

use async_trait::async_trait;

use crate::error::{CoreError, Result};

/// Schedules persona activity (rotation, session cadence).
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// Start the scheduler loop.
    async fn start(&self) -> Result<()> {
        Err(CoreError::Unimplemented("scheduler::start"))
    }

    /// Stop the scheduler loop.
    async fn stop(&self) -> Result<()> {
        Err(CoreError::Unimplemented("scheduler::stop"))
    }
}

/// No-op scheduler used until the real one lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubScheduler;

#[async_trait]
impl Scheduler for StubScheduler {}
