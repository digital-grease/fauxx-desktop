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

//! Command handlers, one module per command group.
//!
//! Each handler is a thin shim: it opens the core with the resolved
//! [`fauxx_core::Config`], calls the relevant async core method, and renders
//! the result. No business logic lives here.

pub mod ab;
pub mod alias;
pub mod anchor;
pub mod broker;
pub mod campaign;
pub mod dns;
pub mod drift;
pub mod dsar;
pub mod egress;
pub mod export;
pub mod generate;
pub mod gpc;
pub mod logs;
pub mod mint;
pub mod mode;
pub mod native_host;
pub mod pack;
pub mod pair;
pub mod peers;
pub mod persona;
pub mod run;
pub mod schedule;
pub mod search;
pub mod serve;
pub mod serve_config;
pub mod status;
