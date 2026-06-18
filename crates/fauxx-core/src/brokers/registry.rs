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

//! The bundled, data-driven registry of data-broker opt-out request TEMPLATES
//! (C3 #15, D1c).
//!
//! Each entry is a pre-filled opt-out request template for a major data-broker /
//! people-search site, keyed by a short broker id. The table is embedded at
//! compile time from `brokers.json` via `include_str!` + serde, so it ships in
//! the binary with no runtime file dependency, is validated once (and cached) on
//! first use, and stays trivially extensible: adding a broker is a JSON edit.
//!
//! Invariants a unit test enforces:
//!
//! - every entry has a non-empty display name and a non-empty `https://`
//!   opt-out URL,
//! - every entry lists at least one required request field,
//! - NO entry's opt-out URL is on the R3 auth-flow blocklist (these are PUBLIC
//!   request forms, never sign-in endpoints),
//! - an `email` method entry carries an `email_to` address.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The bundled broker registry, embedded at compile time.
pub const BROKER_REGISTRY_JSON: &str = include_str!("brokers.json");

/// How a broker accepts an opt-out request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OptOutMethod {
    /// A public web form the decoy browser can navigate to and fill (the common
    /// case): driven through the R3-guarded [`DecoyPage::navigate`].
    ///
    /// [`DecoyPage::navigate`]: crate::browser::DecoyPage::navigate
    WebForm,
    /// An email request: the filled request is a message body sent to the
    /// broker's privacy address (carried in [`BrokerTemplate::email_to`]).
    Email,
    /// A manual step the operator must perform (e.g. a mailed letter or a phone
    /// call); generation still produces the filled request for them to use.
    Manual,
}

/// One required request field a broker's opt-out form needs. The set is
/// data-driven (any new field name is just a JSON value); these are the common
/// ones across the curated set, so callers can match exhaustively where useful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredField {
    /// The persona/user's display name.
    Name,
    /// A location (city/state) disambiguating the listing.
    Location,
    /// The URL of the specific listing/profile to suppress.
    ListingUrl,
    /// A confirmation email address (the broker emails a removal link).
    Email,
    /// A phone number (some brokers confirm via an automated call).
    Phone,
    /// Any other field name a future broker requires, carried verbatim.
    #[serde(untagged)]
    Other(String),
}

impl RequiredField {
    /// The canonical field key (matches the JSON value), for stable display and
    /// for keying a [`FilledRequest`](crate::brokers::FilledRequest) value map.
    pub fn key(&self) -> &str {
        match self {
            RequiredField::Name => "name",
            RequiredField::Location => "location",
            RequiredField::ListingUrl => "listing_url",
            RequiredField::Email => "email",
            RequiredField::Phone => "phone",
            RequiredField::Other(s) => s.as_str(),
        }
    }
}

/// A pre-filled opt-out request TEMPLATE for one data broker (C3 #15).
///
/// Data-driven and serde-loaded from `brokers.json`. The template is the static
/// description of HOW to opt out of this broker; a per-persona
/// [`FilledRequest`](crate::brokers::FilledRequest) is generated from it plus a
/// persona's details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerTemplate {
    /// Human-readable broker name (e.g. "Spokeo").
    pub display_name: String,
    /// The PUBLIC opt-out request URL (always `https://`, never a sign-in host).
    pub opt_out_url: String,
    /// How the request is submitted.
    pub method: OptOutMethod,
    /// The request fields this broker requires, in display order.
    pub required_fields: Vec<RequiredField>,
    /// Operator-facing notes on the broker's confirmation flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The privacy email address for an [`OptOutMethod::Email`] broker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_to: Option<String>,
    /// Days from submission within which the broker is expected to action the
    /// request, used to compute deadline reminders. Defaults to 30.
    #[serde(default = "default_deadline_days")]
    pub deadline_days: i64,
}

/// The default opt-out actioning window when a broker omits one (30 days).
fn default_deadline_days() -> i64 {
    30
}

/// Parsed, validated, cached registry. A `BTreeMap` keeps iteration order
/// deterministic so listings and tests are stable.
fn registry() -> &'static BTreeMap<String, BrokerTemplate> {
    static TABLE: OnceLock<BTreeMap<String, BrokerTemplate>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The registry is a compile-time-embedded const we author and a unit
        // test validates, so a parse failure here is a build-time authoring bug,
        // not runtime input. Fall back to an empty map rather than panicking so
        // the no-unwrap rule holds; the accessors then surface a typed error.
        serde_json::from_str(BROKER_REGISTRY_JSON).unwrap_or_default()
    })
}

/// All broker ids in the registry, in deterministic (sorted) order.
pub fn broker_ids() -> Vec<&'static str> {
    registry().keys().map(String::as_str).collect()
}

/// The full registry as `(id, template)` pairs, deterministic order. The clean
/// list surface the Core API exposes.
pub fn brokers() -> Vec<(&'static str, &'static BrokerTemplate)> {
    registry().iter().map(|(k, v)| (k.as_str(), v)).collect()
}

/// Look up one broker template by id. Returns [`CoreError::NotFound`] when the
/// id is unknown.
pub fn broker(id: &str) -> Result<&'static BrokerTemplate> {
    registry()
        .get(id)
        .ok_or_else(|| CoreError::NotFound(format!("broker {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::isolation;

    #[test]
    fn registry_parses_and_is_non_empty() -> Result<()> {
        let all = brokers();
        assert!(
            all.len() >= 5,
            "expected a curated set of at least 5 major brokers, got {}",
            all.len()
        );
        // The named curated set is present.
        for id in [
            "spokeo",
            "whitepages",
            "beenverified",
            "intelius",
            "radaris",
        ] {
            broker(id)?;
        }
        Ok(())
    }

    #[test]
    fn every_entry_has_url_fields_and_is_not_an_auth_host() {
        for (id, t) in brokers() {
            assert!(
                !t.display_name.trim().is_empty(),
                "{id} has no display name"
            );
            assert!(
                t.opt_out_url.starts_with("https://"),
                "{id} opt-out URL must be HTTPS: {}",
                t.opt_out_url
            );
            assert!(
                !t.required_fields.is_empty(),
                "{id} must require at least one request field"
            );
            // HARD GUARDRAIL: a broker opt-out URL must never be a sign-in
            // endpoint. These are PUBLIC request forms only.
            assert!(
                !isolation::is_blocked_auth_flow(&t.opt_out_url),
                "{id} opt-out URL is on the R3 auth-flow blocklist: {}",
                t.opt_out_url
            );
            assert!(t.deadline_days > 0, "{id} deadline window must be positive");
        }
    }

    #[test]
    fn email_method_brokers_carry_an_email_address() {
        for (id, t) in brokers() {
            if t.method == OptOutMethod::Email {
                assert!(
                    t.email_to.as_deref().is_some_and(|e| e.contains('@')),
                    "{id} is an email-method broker but has no email_to address"
                );
            }
        }
    }

    #[test]
    fn unknown_broker_is_not_found() {
        assert!(matches!(broker("nope"), Err(CoreError::NotFound(_))));
    }

    #[test]
    fn required_field_keys_are_stable() {
        assert_eq!(RequiredField::Name.key(), "name");
        assert_eq!(RequiredField::ListingUrl.key(), "listing_url");
        assert_eq!(RequiredField::Other("custom".to_string()).key(), "custom");
    }

    #[test]
    fn required_field_deserializes_known_and_other() -> Result<()> {
        let known: RequiredField = serde_json::from_str("\"listing_url\"")?;
        assert_eq!(known, RequiredField::ListingUrl);
        let other: RequiredField = serde_json::from_str("\"date_of_birth\"")?;
        assert_eq!(other, RequiredField::Other("date_of_birth".to_string()));
        Ok(())
    }
}
