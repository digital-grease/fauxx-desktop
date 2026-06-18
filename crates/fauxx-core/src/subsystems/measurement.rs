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

//! Measurement subsystem (stub).
//!
//! Scores how effectively a persona is shaping its ad/topic profile, feeding
//! `efficacy_history`. The real measurement lands in a later milestone; for C0
//! the trait pins the seam and the default method reports
//! [`CoreError::Unimplemented`].

use async_trait::async_trait;

use crate::error::{CoreError, Result};

/// Measures persona efficacy.
#[async_trait]
pub trait Measurement: Send + Sync {
    /// Measure efficacy for the persona with the given id, returning a score.
    async fn measure(&self, _persona_id: &str) -> Result<f64> {
        Err(CoreError::Unimplemented("measurement::measure"))
    }
}

/// No-op measurement used until the real one lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubMeasurement;

#[async_trait]
impl Measurement for StubMeasurement {}
