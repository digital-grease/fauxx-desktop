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

//! Home Assistant MQTT-discovery payloads (C8 #36, U5).
//!
//! HA auto-creates entities from a retained "discovery config" JSON published
//! to `<prefix>/<component>/<node_id>/<object_id>/config`. Each config names
//! the entity, points at a `state_topic`, and (for our sensors) a
//! `value_template` that extracts the numeric/text value from the JSON state
//! payload. This module builds those config payloads ([`SensorPayload`]) and the
//! matching state payloads ([`StatusPayload`], [`EfficacySensor`]) for the
//! companion's STATUS and EFFICACY (A1 drift summary) sensors.
//!
//! Everything here is pure (no I/O): a bridge publishes what these build.

use serde::{Deserialize, Serialize};

use super::MqttConfig;
use crate::campaigns::CampaignStatus;
use crate::idle::IdleState;
use crate::orchestration::IntensityLevel;

/// The device-registry block shared by every discovery config so HA groups all
/// the companion's sensors under one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable device identifiers (HA dedupes on these).
    pub identifiers: Vec<String>,
    /// The device display name.
    pub name: String,
    /// The device manufacturer.
    pub manufacturer: String,
    /// The device model.
    pub model: String,
}

impl DeviceInfo {
    /// The companion's device-registry block for `cfg`.
    pub fn for_config(cfg: &MqttConfig) -> Self {
        Self {
            identifiers: vec![cfg.device_id.clone()],
            name: "Fauxx Desktop Companion".to_string(),
            manufacturer: "Digital Grease".to_string(),
            model: "fauxx-core".to_string(),
        }
    }
}

/// A Home Assistant MQTT-discovery SENSOR config payload (C8 #36).
///
/// Published (retained) to the sensor's discovery config topic; HA creates the
/// entity, subscribes `state_topic`, and renders `value_template` over the JSON
/// state payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorPayload {
    /// The entity display name.
    pub name: String,
    /// A globally-unique entity id (HA requires it for the device registry).
    pub unique_id: String,
    /// The topic the entity's state is published to.
    pub state_topic: String,
    /// A Jinja template extracting the value from the JSON state payload.
    pub value_template: String,
    /// Optional unit of measurement (omitted for text sensors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<String>,
    /// Optional icon (a Material Design Icons name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The device-registry block grouping the companion's entities.
    pub device: DeviceInfo,
}

/// The DISCOVERY configuration the bridge publishes for the companion (C8 #36):
/// every sensor's `(config_topic, SensorPayload)` plus the command-topic the
/// command entity listens on. Built once from an [`MqttConfig`].
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// `(config_topic, payload)` for each sensor to announce to HA.
    pub sensors: Vec<(String, SensorPayload)>,
}

impl DiscoveryConfig {
    /// Build the discovery set for the companion: a status sensor plus one
    /// efficacy (A1 drift summary) sensor per `(persona, platform)` object id in
    /// `efficacy_object_ids`. `object_id` strings should be HA-safe (lowercase,
    /// underscores), e.g. `"p1_google"`.
    pub fn build(cfg: &MqttConfig, efficacy_object_ids: &[String]) -> Self {
        let device = DeviceInfo::for_config(cfg);
        let mut sensors = Vec::with_capacity(1 + efficacy_object_ids.len());

        // Status sensor: a text sensor showing the running/idle state, with the
        // full status JSON available to HA templates.
        sensors.push((
            cfg.discovery_config_topic("sensor", "status"),
            SensorPayload {
                name: "Fauxx Status".to_string(),
                unique_id: format!("{}_status", cfg.device_id),
                state_topic: cfg.status_state_topic(),
                value_template: "{{ value_json.summary }}".to_string(),
                unit_of_measurement: None,
                icon: Some("mdi:incognito".to_string()),
                device: device.clone(),
            },
        ));

        // One efficacy sensor per object id: a numeric drift sensor.
        for object_id in efficacy_object_ids {
            sensors.push((
                cfg.discovery_config_topic("sensor", &format!("efficacy_{object_id}")),
                SensorPayload {
                    name: format!("Fauxx Efficacy {object_id}"),
                    unique_id: format!("{}_efficacy_{object_id}", cfg.device_id),
                    state_topic: cfg.efficacy_state_topic(object_id),
                    value_template: "{{ value_json.drift }}".to_string(),
                    unit_of_measurement: Some("KL".to_string()),
                    icon: Some("mdi:chart-bell-curve".to_string()),
                    device: device.clone(),
                },
            ));
        }

        Self { sensors }
    }
}

/// The STATUS state payload (C8 #36): the companion's live state, published to
/// the status sensor's state topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusPayload {
    /// A one-line human-readable summary (the status sensor's displayed value).
    pub summary: String,
    /// Whether decoy activity is currently running.
    pub running: bool,
    /// The current idle/lock state (U1).
    pub idle_state: IdleState,
    /// The effective decoy intensity, or `None` when paused.
    pub intensity: Option<IntensityLevel>,
    /// How many campaigns are currently in the `Running` state (U2).
    pub running_campaigns: usize,
}

impl StatusPayload {
    /// Build a status payload, deriving the summary line from the state.
    pub fn new(
        running: bool,
        idle_state: IdleState,
        intensity: Option<IntensityLevel>,
        running_campaigns: usize,
    ) -> Self {
        let summary = if running {
            match intensity {
                Some(level) => format!("running ({})", intensity_label(level)),
                None => "running".to_string(),
            }
        } else {
            "paused".to_string()
        };
        Self {
            summary,
            running,
            idle_state,
            intensity,
            running_campaigns,
        }
    }
}

/// The EFFICACY state payload (C8 #36): the A1 drift summary for one
/// `(persona, platform)` segment, published to that efficacy sensor's topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EfficacySensor {
    /// The persona this efficacy reading is for.
    pub persona_id: String,
    /// The platform label (e.g. `Google`).
    pub platform: String,
    /// The latest scalar A1 KL-divergence drift, or `0.0` when no data yet.
    pub drift: f64,
    /// How many drift points are in the series (0 = no data).
    pub points: usize,
}

impl EfficacySensor {
    /// Build an efficacy reading.
    pub fn new(
        persona_id: impl Into<String>,
        platform: impl Into<String>,
        drift: f64,
        points: usize,
    ) -> Self {
        Self {
            persona_id: persona_id.into(),
            platform: platform.into(),
            drift,
            points,
        }
    }

    /// An HA-safe object id for this reading: `<persona>_<platform>`, lowercased
    /// with non-alphanumerics folded to underscores.
    pub fn object_id(&self) -> String {
        ha_object_id(&format!("{}_{}", self.persona_id, self.platform))
    }
}

/// Lowercase and fold non-alphanumeric characters to underscores so a string is
/// safe as an HA object id / topic segment.
pub fn ha_object_id(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The displayed label for an intensity level.
fn intensity_label(level: IntensityLevel) -> &'static str {
    match level {
        IntensityLevel::Low => "low",
        IntensityLevel::Medium => "medium",
        IntensityLevel::High => "high",
        IntensityLevel::Extreme => "extreme",
    }
}

/// The displayed label for a campaign lifecycle status (re-exported helper for
/// callers building richer HA attributes).
pub fn campaign_status_label(status: CampaignStatus) -> &'static str {
    status.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn discovery_includes_status_and_one_sensor_per_object_id() {
        let cfg = MqttConfig::new("ha.lan");
        let discovery = DiscoveryConfig::build(&cfg, &["p1_google".to_string()]);
        // Status sensor + one efficacy sensor.
        assert_eq!(discovery.sensors.len(), 2);
        let (status_topic, status) = &discovery.sensors[0];
        assert_eq!(
            status_topic,
            "homeassistant/sensor/fauxx_desktop/status/config"
        );
        assert_eq!(status.state_topic, "fauxx/status/state");
        assert_eq!(status.unique_id, "fauxx_desktop_status");

        let (eff_topic, eff) = &discovery.sensors[1];
        assert_eq!(
            eff_topic,
            "homeassistant/sensor/fauxx_desktop/efficacy_p1_google/config"
        );
        assert_eq!(eff.state_topic, "fauxx/efficacy/p1_google/state");
        assert_eq!(eff.unit_of_measurement.as_deref(), Some("KL"));
        // Every sensor shares the one device-registry entry.
        assert_eq!(status.device.identifiers, vec!["fauxx_desktop".to_string()]);
        assert_eq!(eff.device.identifiers, vec!["fauxx_desktop".to_string()]);
    }

    #[test]
    fn discovery_payload_serializes_to_ha_shape() -> Result<()> {
        let cfg = MqttConfig::new("ha.lan");
        let discovery = DiscoveryConfig::build(&cfg, &[]);
        let (_topic, status) = &discovery.sensors[0];
        let json = serde_json::to_string(status)?;
        // The HA-expected keys are present.
        assert!(json.contains("\"state_topic\":\"fauxx/status/state\""));
        assert!(json.contains("\"value_template\""));
        assert!(json.contains("\"unique_id\""));
        assert!(json.contains("\"device\""));
        // unit_of_measurement is omitted for the text status sensor.
        assert!(!json.contains("unit_of_measurement"));
        Ok(())
    }

    #[test]
    fn status_payload_summary_reflects_state() {
        let running = StatusPayload::new(
            true,
            IdleState::idle_secs(600),
            Some(IntensityLevel::High),
            2,
        );
        assert_eq!(running.summary, "running (high)");
        assert!(running.running);

        let paused = StatusPayload::new(false, IdleState::Active, None, 0);
        assert_eq!(paused.summary, "paused");
        assert!(!paused.running);
    }

    #[test]
    fn efficacy_object_id_is_ha_safe() {
        let eff = EfficacySensor::new("Persona One", "Google", 0.42, 5);
        // Folded to lowercase underscores.
        assert_eq!(eff.object_id(), "persona_one_google");
    }
}
