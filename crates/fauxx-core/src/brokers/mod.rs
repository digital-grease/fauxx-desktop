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

//! Data-broker opt-out & deletion automation (C3 #15, D1c).
//!
//! This is part of the lawful, high-leverage "deterministic-channel defense"
//! the phone cannot do: drive PUBLIC opt-out request forms on major data-broker
//! and people-search sites so a persona (or the user) can be suppressed from
//! their data products, then track the request to confirmation and re-scan for
//! re-listing.
//!
//! ## What this is and is NOT
//!
//! - It operates ONLY on the brokers' public opt-out request forms. It NEVER
//!   automates against an authenticated account: there is no account to log into
//!   on these forms, and every browser-driven submission goes through the
//!   R3-guarded [`DecoyPage::navigate`](crate::browser::DecoyPage::navigate),
//!   which refuses sign-in endpoints. A unit test asserts no registry entry is
//!   itself on the R3 auth-flow blocklist.
//! - Generation and state recording are the solid, hermetic-testable core;
//!   web-form field entry is best-effort over the decoy browser.
//!
//! ## Layout
//!
//! - [`registry`]: the bundled, data-driven registry of opt-out request
//!   TEMPLATES (`brokers.json`, loaded via `include_str!` + serde).
//! - [`submission`]: the filled-request model, the persisted submission record
//!   ([`BrokerSubmission`]), the deadline-reminder predicate, and the
//!   re-listing detection seam ([`ListingCheck`]).
//! - [`scan`]: per-`(broker, persona)` identity scan SNAPSHOTS
//!   ([`BrokerScanSnapshot`]) and the time-ordered diff timeline
//!   ([`BrokerDiffTimeline`]) the A3 broker diff view (C4 #22) computes from
//!   stored snapshots, classifying each field added/removed/unchanged and
//!   distinctly flagging re-listing.
//!
//! The async Core surface ([`crate::Core`]) exposes the registry, submission
//! generate/record/list/track, the re-scan, and the snapshot record/list plus
//! computed diff timeline.

pub mod registry;
pub mod scan;
pub mod submission;

pub use registry::{
    broker, brokers, BrokerTemplate, OptOutMethod, RequiredField, BROKER_REGISTRY_JSON,
};
pub use scan::{
    compute_broker_diff_timeline, BrokerDiffTimeline, BrokerScanSnapshot, FieldChange, FieldDelta,
    SnapshotDiff,
};
pub use submission::{
    BrokerSubmission, FilledRequest, ListingCheck, RelistOutcome, StaticListingCheck,
    SubmissionStatus,
};
