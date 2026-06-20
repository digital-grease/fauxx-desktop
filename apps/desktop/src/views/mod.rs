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

//! Per-state render functions, one module per [`crate::state::AppState`]
//! variant. Each is a pure `&state -> Element` function dispatched from
//! [`crate::view::view`].

pub mod brokers;
pub mod campaigns;
pub mod charts;
pub mod dashboard;
pub mod devices;
pub mod error;
pub mod faq;
pub mod loading;
pub mod network;
pub mod privacy;
pub mod running;
pub mod settings;
pub mod studio;
pub mod wizard;
