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

//! The four behavioral subsystems, as C0 stubs.
//!
//! Each subsystem ships a minimal trait so the API *shape* exists and both
//! clients can compile against it, but every method returns
//! [`CoreError::Unimplemented`](crate::error::CoreError::Unimplemented) until
//! its milestone (C1-C8) lands. Keeping them tiny here pins the seams without
//! committing to behavior.

pub mod browser;
pub mod measurement;
pub mod scheduler;
pub mod sync;
