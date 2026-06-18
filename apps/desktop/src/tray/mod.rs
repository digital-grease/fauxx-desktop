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

//! System-tray agent and its bridge into the iced event loop.
//!
//! The tray backend is split by operating system so the Linux build stays off
//! the archived gtk-rs GTK3 stack (`tray-icon` -> `libappindicator` -> `gtk` ->
//! `glib`), which is unmaintained and carries the `glib` `VariantStrIter`
//! unsoundness:
//!
//! - **Linux** ([`linux`]): `ksni`, a pure-Rust implementation of the
//!   freedesktop StatusNotifierItem spec over `zbus`. It runs *inside* the iced
//!   subscription on the GUI's tokio runtime, so there is no extra OS thread and
//!   no GTK/glib (or C libdbus) dependency at all. A menu selection is delivered
//!   straight into the iced message bus through an async channel.
//! - **Windows / macOS** ([`other`]): `tray-icon`, which requires a platform
//!   event loop on its own thread and publishes events through its process-wide
//!   channels; a forwarder relays them into an iced subscription. See that
//!   module for the loop co-existence rationale.
//!
//! Both backends expose the SAME surface to the rest of the app, so `main` and
//! `state` are platform-agnostic:
//!
//! - [`Tray`] — a one-shot carrier built once in `main`,
//! - [`TrayHandle`] — parked in `App` for the life of the process,
//! - [`subscription`] — surfaces tray events as [`crate::message::Message`].
//!
//! Each backend builds the same five quick-control menu items (Open Window,
//! Status, Pause, Resume, Quit), mapped to [`crate::message::TrayMessage`].

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{subscription, Tray, TrayHandle};

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
pub use other::{subscription, Tray, TrayHandle};
