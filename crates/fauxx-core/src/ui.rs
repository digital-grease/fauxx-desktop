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

//! UI sink abstraction.
//!
//! The core emits user-facing status through a [`UiSink`] so business logic
//! never references a GUI toolkit and carries no `#[cfg]`. The GUI client
//! supplies a real sink that feeds the Iced MVU loop; headless and CLI builds
//! use [`NullUi`]. This is the seam that keeps the front-end choice reversible:
//! swapping or removing the GUI never touches the core.

/// A sink for user-facing status updates emitted by the core.
pub trait UiSink: Send + Sync {
    /// Surface a short status message to the user (a toast, tray tooltip, or
    /// status line, depending on the client).
    fn status(&self, message: &str);
}

/// No-op sink used by headless and CLI builds, and anywhere no UI is attached.
/// Status surfaces through `tracing` and CLI output instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullUi;

impl UiSink for NullUi {
    fn status(&self, _message: &str) {}
}
