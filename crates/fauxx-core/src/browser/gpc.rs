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

//! Global Privacy Control: emission and per-site honoring detection (D4c #18).
//!
//! ## Emission
//!
//! GPC is a lawful, deterministic opt-out signal: a request header
//! (`Sec-GPC: 1`) and a JS property (`navigator.globalPrivacyControl === true`)
//! that together tell a site the user opts out of sale/sharing of their data.
//! The decoy browser ([`crate::browser`]) emits both on every decoy navigation,
//! default-ON, applied ONLY to the isolated decoy profile (R3) and never a real
//! authenticated session.
//!
//! [`NAVIGATOR_GPC_INJECT_JS`] is the exact script injected (before page scripts
//! run) to set the navigator property; the request header is set over CDP. They
//! live here so the GPC contract is reviewable in one place.
//!
//! ## Honoring detection (where observable)
//!
//! A site that honors GPC can advertise it at the well-known resource
//! `/.well-known/gpc.json`, defined by the GPC spec to return a small JSON
//! object: `{"gpc": true, "lastUpdate": "2022-01-01"}`. [`parse_gpc_well_known`]
//! parses that body into a typed [`GpcSupport`] and is deliberately tolerant:
//! a missing file, a network failure, or a garbage body all yield a well-formed
//! `GpcSupport { honored: false, .. }`, because "no advertised support" is a
//! valid observation, not a crash. The parse is pure and hermetic-testable; the
//! live fetch (over the decoy browser) is in [`crate::browser::DecoyPage`].

use serde::{Deserialize, Serialize};

/// The exact JavaScript injected into every new decoy document to make GPC
/// observable to page scripts (D4c #18). Defines `navigator.globalPrivacyControl`
/// as a non-configurable `true` so a site reading the property sees the opt-out.
/// Guarded with a `try/catch` so a browser that already defines the property (or
/// one that refuses redefinition) does not throw and break page load.
pub(crate) const NAVIGATOR_GPC_INJECT_JS: &str = "try { \
    Object.defineProperty(navigator, 'globalPrivacyControl', { \
        value: true, configurable: false, enumerable: true \
    }); \
} catch (e) { \
    try { navigator.globalPrivacyControl = true; } catch (e2) {} \
}";

/// The well-known path a site uses to advertise GPC honoring, per the spec.
pub const GPC_WELL_KNOWN_PATH: &str = "/.well-known/gpc.json";

/// A parsed observation of a site's advertised GPC support, from its
/// `/.well-known/gpc.json` (D4c #18).
///
/// `honored` is the bottom line: did the site advertise `{"gpc": true}`. The
/// `last_update` and `version` fields are carried verbatim when present (both
/// optional in the spec) so the store keeps the full advertised record. A site
/// with no well-known file, a non-200 response, or a malformed body parses to
/// `GpcSupport { honored: false, last_update: None, version: None }`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GpcSupport {
    /// Whether the site advertised that it honors GPC (`"gpc": true`).
    pub honored: bool,
    /// The advertised last-update date string (`"lastUpdate"`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<String>,
    /// The advertised GPC spec version (`"version"`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl GpcSupport {
    /// A "no advertised support" observation (missing file, error, or garbage).
    pub fn not_advertised() -> Self {
        Self::default()
    }
}

/// Build the `/.well-known/gpc.json` URL for an `https://host` origin.
///
/// Accepts an origin with or without a trailing slash, and with or without a
/// path (only the scheme+authority are kept). The minimal, dependency-free split
/// mirrors [`crate::browser::isolation`]'s parser: we only need scheme+authority.
pub(crate) fn well_known_url_for(origin: &str) -> String {
    let trimmed = origin.trim();
    // Keep scheme://authority; drop any path/query/fragment the caller passed.
    let base = match trimmed.split_once("://") {
        Some((scheme, rest)) => {
            let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            format!("{scheme}://{}", &rest[..auth_end])
        }
        // No scheme: assume https and treat the whole thing as the authority.
        None => {
            let auth_end = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
            format!("https://{}", &trimmed[..auth_end])
        }
    };
    format!("{base}{GPC_WELL_KNOWN_PATH}")
}

/// Parse a `/.well-known/gpc.json` body into a typed [`GpcSupport`] (D4c #18).
///
/// Tolerant by design: `None` (no body fetched, or a non-200 response the caller
/// mapped to `None`), an empty string, invalid JSON, or a JSON value that is not
/// an object all parse to a "not advertised" observation rather than an error.
/// When the body IS a JSON object, `gpc` is read as a boolean (a non-boolean or
/// absent `gpc` counts as not honored); `lastUpdate` and `version` are carried
/// when present as strings.
pub fn parse_gpc_well_known(body: Option<&str>) -> GpcSupport {
    let Some(text) = body else {
        return GpcSupport::not_advertised();
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return GpcSupport::not_advertised();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return GpcSupport::not_advertised();
    };
    let Some(map) = value.as_object() else {
        return GpcSupport::not_advertised();
    };

    let honored = map
        .get("gpc")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let last_update = map
        .get("lastUpdate")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let version = map.get("version").and_then(|v| match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    });

    GpcSupport {
        honored,
        last_update,
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_well_known_advertising_support() {
        // The canonical body the GPC spec shows.
        let body = r#"{ "gpc": true, "lastUpdate": "2022-06-01" }"#;
        let support = parse_gpc_well_known(Some(body));
        assert!(support.honored);
        assert_eq!(support.last_update.as_deref(), Some("2022-06-01"));
        assert_eq!(support.version, None);
    }

    #[test]
    fn parses_version_field_string_or_number() {
        let s = parse_gpc_well_known(Some(r#"{ "gpc": true, "version": "1" }"#));
        assert_eq!(s.version.as_deref(), Some("1"));
        let n = parse_gpc_well_known(Some(r#"{ "gpc": true, "version": 1 }"#));
        assert_eq!(n.version.as_deref(), Some("1"));
    }

    #[test]
    fn gpc_false_is_well_formed_not_honored() {
        let support = parse_gpc_well_known(Some(r#"{ "gpc": false }"#));
        assert!(!support.honored);
    }

    #[test]
    fn missing_body_is_not_advertised() {
        assert_eq!(parse_gpc_well_known(None), GpcSupport::not_advertised());
        assert!(!parse_gpc_well_known(None).honored);
    }

    #[test]
    fn garbage_and_empty_bodies_are_not_advertised_not_errors() {
        for body in [
            "",
            "   ",
            "not json at all",
            "<html>404</html>",
            "null",
            "[]",
            "42",
        ] {
            let support = parse_gpc_well_known(Some(body));
            assert!(
                !support.honored,
                "garbage body should parse to not-honored: {body:?}"
            );
            assert_eq!(support, GpcSupport::not_advertised());
        }
    }

    #[test]
    fn non_boolean_gpc_field_counts_as_not_honored() {
        // A site that puts a string where a bool belongs is malformed; treat it
        // as no advertised support rather than guessing.
        let support = parse_gpc_well_known(Some(r#"{ "gpc": "true" }"#));
        assert!(!support.honored);
    }

    #[test]
    fn gpc_support_json_round_trips_and_omits_none() -> Result<(), serde_json::Error> {
        let support = GpcSupport {
            honored: true,
            last_update: Some("2022-06-01".to_string()),
            version: None,
        };
        let json = serde_json::to_string(&support)?;
        assert!(!json.contains("version"));
        let back: GpcSupport = serde_json::from_str(&json)?;
        assert_eq!(back, support);
        Ok(())
    }

    #[test]
    fn well_known_url_is_built_from_origin() {
        assert_eq!(
            well_known_url_for("https://example.com"),
            "https://example.com/.well-known/gpc.json"
        );
        assert_eq!(
            well_known_url_for("https://example.com/"),
            "https://example.com/.well-known/gpc.json"
        );
        // A full URL: only scheme+authority are kept.
        assert_eq!(
            well_known_url_for("https://example.com/some/path?q=1"),
            "https://example.com/.well-known/gpc.json"
        );
        // No scheme: assume https.
        assert_eq!(
            well_known_url_for("example.com"),
            "https://example.com/.well-known/gpc.json"
        );
    }
}
