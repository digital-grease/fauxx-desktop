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

//! Privacy Sandbox Topics read-back (C2 #12, R2): the typed measurement record
//! and the robust parser for a `document.browsingTopics()` payload.
//!
//! ## What the Topics API returns
//!
//! `document.browsingTopics()` resolves to an array of objects, each shaped
//! roughly like:
//!
//! ```json
//! [ { "topic": 57, "version": "chrome.1:1:2", "configVersion": "chrome.1",
//!     "modelVersion": "2", "taxonomyVersion": "1" } ]
//! ```
//!
//! Field names and exact shape have shifted across Chromium versions (older
//! builds returned a flat `version` string; newer ones split it into
//! `configVersion` / `taxonomyVersion` / `modelVersion`, and some builds add a
//! human-readable topic name). The parser here is deliberately tolerant: it
//! reads whichever of those fields are present and never fails on a missing
//! optional field. The ONLY required field is the integer `topic` id.
//!
//! ## The epoch-boundary reality (handled honestly)
//!
//! Topics are computed per WEEKLY EPOCH from recent history, so freshly injected
//! history does NOT immediately yield assigned topics. A read right after
//! seeding commonly returns an EMPTY array until the epoch rolls. That is not an
//! error: an empty, well-formed result is the expected outcome inside the
//! observation window. [`parse_topics_payload`] therefore treats `[]` as a valid
//! (empty) [`TopicsReadback`], and callers persist it as a real measurement.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// One assigned Privacy Sandbox topic, parsed from a `document.browsingTopics()`
/// entry into a typed, persistable record.
///
/// Only [`topic_id`](Self::topic_id) is required; the version and name fields are
/// optional because their presence and naming vary across Chromium versions. The
/// record is `serde`-serializable so it persists as JSON in the encrypted store
/// and rides the Core async API to dashboards (C4) and campaigns (C8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignedTopic {
    /// The integer topic id from the Topics taxonomy (the one required field).
    pub topic_id: i64,
    /// The taxonomy version (e.g. `"1"`), when the browser reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxonomy_version: Option<String>,
    /// The model version (e.g. `"2"`), when the browser reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    /// The combined/config version string (e.g. `"chrome.1:1:2"` or
    /// `"chrome.1"`), when the browser reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A human-readable topic name, when the browser reports one (newer builds
    /// may include it; most return only the numeric id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A full Topics read-back from one decoy page: the parsed topics plus whether
/// the API was even available in this context. An empty `topics` with
/// `available == true` is the expected epoch-boundary outcome, NOT a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicsReadback {
    /// Whether `document.browsingTopics` existed and was callable in the page's
    /// context (the Privacy Sandbox flags were enabled and the context secure).
    pub available: bool,
    /// The assigned topics. Commonly EMPTY right after seeding history, because
    /// topics are computed per weekly epoch; an empty list is well-formed.
    pub topics: Vec<AssignedTopic>,
}

impl TopicsReadback {
    /// Whether the read produced any assigned topics. `false` is the common
    /// epoch-boundary case (history seeded, but the weekly epoch has not rolled).
    pub fn is_empty(&self) -> bool {
        self.topics.is_empty()
    }

    /// The number of assigned topics returned.
    pub fn len(&self) -> usize {
        self.topics.len()
    }
}

/// The JavaScript the guarded Topics read evaluates in an eligible page context.
///
/// It is an async arrow function (so the CDP layer treats it as a callable
/// function and AWAITS the returned promise) that:
///
/// - returns `{ "available": false, "topics": [] }` when `document.browsingTopics`
///   is absent (flags off, insecure context, or the API simply not present), and
/// - otherwise awaits the call and returns `{ "available": true, "topics": [...] }`
///   with the raw topic objects passed straight through for the Rust side to
///   parse. A thrown call (e.g. a permissions-policy denial) is caught and
///   surfaced as `available: true, topics: []` so the read still yields a
///   well-formed, empty result rather than blowing up.
///
/// Kept as a `const` so the exact expression is reviewable in one place and the
/// guarded reader cannot accidentally diverge from it.
pub(crate) const BROWSING_TOPICS_READ_JS: &str = "async () => { \
    if (typeof document === 'undefined' || typeof document.browsingTopics !== 'function') { \
        return { available: false, topics: [] }; \
    } \
    try { \
        const result = await document.browsingTopics(); \
        const topics = Array.isArray(result) ? result : []; \
        return { available: true, topics: topics }; \
    } catch (e) { \
        return { available: true, topics: [] }; \
    } \
}";

/// Parse a raw `document.browsingTopics()` JSON payload (as returned by the
/// guarded read) into a typed [`TopicsReadback`].
///
/// Robust by design:
///
/// - A top-level `{ "available": bool, "topics": [...] }` object (what
///   `BROWSING_TOPICS_READ_JS` returns) is read directly.
/// - A bare top-level array (a raw `document.browsingTopics()` result) is also
///   accepted and treated as `available: true`, so the parser round-trips a
///   sample API payload too.
/// - An empty array yields an empty (but well-formed) readback. This is the
///   expected epoch-boundary result, never an error.
/// - Each topic object is parsed leniently: the integer `topic` id is required;
///   `taxonomyVersion` / `modelVersion` / `version` / `configVersion` / `name`
///   are optional and read when present. A topic entry missing the `topic` id is
///   skipped rather than failing the whole parse.
///
/// Returns [`CoreError::Browser`] only when the payload is neither an object with
/// a `topics` array nor a bare array (i.e. genuinely malformed, not merely empty).
pub fn parse_topics_payload(value: &serde_json::Value) -> Result<TopicsReadback> {
    // Two accepted top-level shapes: the wrapper object from our read JS, or a
    // bare array (a raw API result). `null` (a context with no value) is an
    // empty, unavailable readback rather than an error.
    let (available, raw_topics) = match value {
        serde_json::Value::Null => (false, Vec::new()),
        serde_json::Value::Array(items) => (true, items.clone()),
        serde_json::Value::Object(map) => {
            let available = map
                .get("available")
                .and_then(serde_json::Value::as_bool)
                // Absent `available` with a `topics` array present implies the
                // API was reachable; default to available in that case.
                .unwrap_or_else(|| map.contains_key("topics"));
            let topics = match map.get("topics") {
                Some(serde_json::Value::Array(items)) => items.clone(),
                // An object with no `topics` array (and not our wrapper) is not a
                // valid Topics payload.
                _ if map.contains_key("topics") => {
                    return Err(CoreError::Browser(
                        "Topics payload `topics` field is not an array".to_string(),
                    ));
                }
                None => {
                    return Err(CoreError::Browser(
                        "Topics payload object lacks a `topics` array".to_string(),
                    ));
                }
                _ => Vec::new(),
            };
            (available, topics)
        }
        other => {
            return Err(CoreError::Browser(format!(
                "unexpected Topics payload shape: {other}"
            )));
        }
    };

    let mut topics = Vec::with_capacity(raw_topics.len());
    for item in &raw_topics {
        if let Some(topic) = parse_one_topic(item) {
            topics.push(topic);
        }
    }
    Ok(TopicsReadback { available, topics })
}

/// Parse a single topic object leniently. Returns `None` (and the entry is
/// skipped) when the required integer `topic` id is absent, non-numeric, or
/// negative (real Topics ids are non-negative; a negative value is malformed and
/// is skipped like a missing one).
fn parse_one_topic(value: &serde_json::Value) -> Option<AssignedTopic> {
    let obj = value.as_object()?;
    // Accept `topic` (the canonical key) or `topicId` as a defensive alias.
    let topic_id = obj
        .get("topic")
        .or_else(|| obj.get("topicId"))
        .and_then(serde_json::Value::as_i64)
        .filter(|id| *id >= 0)?;

    // String-or-number tolerant: some builds report versions as numbers.
    let as_string = |key: &str| -> Option<String> {
        obj.get(key).and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };

    Some(AssignedTopic {
        topic_id,
        taxonomy_version: as_string("taxonomyVersion"),
        model_version: as_string("modelVersion"),
        // Prefer the split `configVersion`, fall back to a flat `version`.
        version: as_string("configVersion").or_else(|| as_string("version")),
        name: as_string("name").or_else(|| as_string("topicName")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_split_version_payload() -> Result<()> {
        // The wrapper shape our read JS returns, with the newer split-version
        // fields and a human-readable name.
        let payload = serde_json::json!({
            "available": true,
            "topics": [
                {
                    "topic": 57,
                    "configVersion": "chrome.1",
                    "modelVersion": "2206021246",
                    "taxonomyVersion": "1",
                    "name": "/Arts & Entertainment"
                },
                {
                    "topic": 104,
                    "configVersion": "chrome.1",
                    "modelVersion": "2206021246",
                    "taxonomyVersion": "1"
                }
            ]
        });
        let readback = parse_topics_payload(&payload)?;
        assert!(readback.available);
        assert_eq!(readback.len(), 2);
        let first = &readback.topics[0];
        assert_eq!(first.topic_id, 57);
        assert_eq!(first.taxonomy_version.as_deref(), Some("1"));
        assert_eq!(first.model_version.as_deref(), Some("2206021246"));
        assert_eq!(first.version.as_deref(), Some("chrome.1"));
        assert_eq!(first.name.as_deref(), Some("/Arts & Entertainment"));
        // The second topic has no name; that field is None, not an error.
        assert_eq!(readback.topics[1].topic_id, 104);
        assert_eq!(readback.topics[1].name, None);
        Ok(())
    }

    #[test]
    fn parses_legacy_flat_version_and_bare_array() -> Result<()> {
        // A bare top-level array (a raw API result) with the older flat
        // `version` string is accepted and treated as available.
        let payload = serde_json::json!([
            { "topic": 1, "version": "chrome.1:1:2" }
        ]);
        let readback = parse_topics_payload(&payload)?;
        assert!(readback.available);
        assert_eq!(readback.len(), 1);
        assert_eq!(readback.topics[0].topic_id, 1);
        assert_eq!(readback.topics[0].version.as_deref(), Some("chrome.1:1:2"));
        assert_eq!(readback.topics[0].taxonomy_version, None);
        Ok(())
    }

    #[test]
    fn empty_array_is_well_formed_not_an_error() -> Result<()> {
        // The expected epoch-boundary outcome: history seeded but no topics yet.
        let bare = parse_topics_payload(&serde_json::json!([]))?;
        assert!(bare.available);
        assert!(bare.is_empty());
        assert_eq!(bare.len(), 0);

        let wrapped = parse_topics_payload(&serde_json::json!({
            "available": true,
            "topics": []
        }))?;
        assert!(wrapped.available);
        assert!(wrapped.is_empty());
        Ok(())
    }

    #[test]
    fn unavailable_api_round_trips() -> Result<()> {
        // Flags off / insecure context: the read JS returns this shape.
        let readback = parse_topics_payload(&serde_json::json!({
            "available": false,
            "topics": []
        }))?;
        assert!(!readback.available);
        assert!(readback.is_empty());
        Ok(())
    }

    #[test]
    fn null_payload_is_unavailable_empty() -> Result<()> {
        let readback = parse_topics_payload(&serde_json::Value::Null)?;
        assert!(!readback.available);
        assert!(readback.is_empty());
        Ok(())
    }

    #[test]
    fn topic_without_id_is_skipped_not_fatal() -> Result<()> {
        let readback = parse_topics_payload(&serde_json::json!({
            "available": true,
            "topics": [
                { "configVersion": "chrome.1" },
                { "topic": 9, "taxonomyVersion": "1" }
            ]
        }))?;
        // The malformed (id-less) entry is dropped; the valid one survives.
        assert_eq!(readback.len(), 1);
        assert_eq!(readback.topics[0].topic_id, 9);
        Ok(())
    }

    #[test]
    fn numeric_version_fields_are_stringified() -> Result<()> {
        let readback = parse_topics_payload(&serde_json::json!({
            "available": true,
            "topics": [
                { "topic": 3, "taxonomyVersion": 1, "modelVersion": 2206021246_i64 }
            ]
        }))?;
        assert_eq!(readback.topics[0].taxonomy_version.as_deref(), Some("1"));
        assert_eq!(
            readback.topics[0].model_version.as_deref(),
            Some("2206021246")
        );
        Ok(())
    }

    #[test]
    fn genuinely_malformed_payload_errors() {
        // A scalar (neither array nor object) is malformed.
        assert!(matches!(
            parse_topics_payload(&serde_json::json!(42)),
            Err(CoreError::Browser(_))
        ));
        // An object whose `topics` is not an array is malformed.
        assert!(matches!(
            parse_topics_payload(&serde_json::json!({ "topics": "nope" })),
            Err(CoreError::Browser(_))
        ));
    }

    #[test]
    fn assigned_topic_json_round_trips_and_omits_none() -> Result<()> {
        let topic = AssignedTopic {
            topic_id: 57,
            taxonomy_version: Some("1".to_string()),
            model_version: None,
            version: Some("chrome.1".to_string()),
            name: None,
        };
        let json = serde_json::to_string(&topic)?;
        // None fields are omitted from the wire form.
        assert!(!json.contains("modelVersion") && !json.contains("model_version"));
        assert!(!json.contains("name"));
        let back: AssignedTopic = serde_json::from_str(&json)?;
        assert_eq!(back, topic);
        Ok(())
    }
}
