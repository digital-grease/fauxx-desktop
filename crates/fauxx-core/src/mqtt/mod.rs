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

//! Home Assistant / MQTT hooks (C8 #36, U5), the cinder `RumqttcBridge` pattern.
//!
//! An always-on homelab deployment should be observable and controllable from
//! the homelab's hub. This module:
//!
//! - publishes STATUS and EFFICACY (the A1 drift summary) as Home Assistant
//!   MQTT-DISCOVERY sensors under a configurable base topic + discovery prefix
//!   (see [`discovery`]),
//! - subscribes a COMMAND topic so HA can start / pause / adjust campaigns,
//!   routed into the U2 campaign planner (see [`command`]),
//! - keeps all MQTT config in plain [`MqttConfig`] (no GUI/CLI types).
//!
//! ## Bridge seam (mirrors `cinder/src/mqtt/mod.rs`)
//!
//! [`MqttBridge`] is an `#[async_trait]`, `Send + Sync` trait. [`MockMqtt`]
//! records (and logs) every publish and ALWAYS compiles. The real
//! `RumqttcBridge` lives behind the off-by-default `mqtt`
//! cargo feature, so the default headless build links no MQTT client. The real
//! bridge follows the cinder pattern exactly:
//!
//! - ONE `AsyncClient` + `EventLoop`; ONE poll task that re-subscribes the
//!   command topic on every `ConnAck` and routes inbound via a bounded mpsc
//!   `try_send` (drop + warn, never block),
//! - warn + sleep on a poll error (rumqttc auto-reconnects); a DOWN broker is
//!   NON-fatal (the core degrades, it does not crash),
//! - the publish handle is a CLONED `AsyncClient` (a request-channel sender),
//!   NEVER an `Arc<Mutex>`; a dropped/failed publish WARNS and never crashes.

pub mod command;
pub mod discovery;

#[cfg(feature = "mqtt")]
pub mod real;

#[cfg(feature = "mqtt")]
pub use real::{connect, MqttConnection, RumqttcBridge};

pub use command::CampaignCommand;
pub use discovery::{DiscoveryConfig, EfficacySensor, SensorPayload, StatusPayload};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default plaintext-MQTT port for a LAN Home Assistant broker (Mosquitto).
pub const DEFAULT_MQTT_PORT: u16 = 1883;
/// Default base topic the companion publishes its state/commands under.
pub const DEFAULT_BASE_TOPIC: &str = "fauxx";
/// Default Home Assistant MQTT-discovery prefix (HA's own default).
pub const DEFAULT_DISCOVERY_PREFIX: &str = "homeassistant";
/// Default device/node id used in topics and the HA device registry entry.
pub const DEFAULT_DEVICE_ID: &str = "fauxx_desktop";

/// MQTT connection + topic configuration (C8 #36). A plain config type with no
/// GUI/CLI types, so the headless deployment, the CLI, and the GUI all build it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MqttConfig {
    /// The broker host (the Home Assistant / Mosquitto LAN address).
    pub host: String,
    /// The broker TCP port (plaintext MQTT; [`DEFAULT_MQTT_PORT`] by default).
    pub port: u16,
    /// The base topic the companion publishes its state and listens for
    /// commands under (e.g. `fauxx`).
    pub base_topic: String,
    /// The Home Assistant MQTT-discovery prefix (e.g. `homeassistant`).
    pub discovery_prefix: String,
    /// The device/node id used in topics and the HA device registry entry.
    pub device_id: String,
    /// Optional broker username (no credential is logged).
    pub username: Option<String>,
    /// Optional broker password (no credential is logged).
    pub password: Option<String>,
}

impl MqttConfig {
    /// A config for `host` with the conventional defaults (port 1883, base
    /// topic `fauxx`, discovery prefix `homeassistant`, no auth).
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_MQTT_PORT,
            base_topic: DEFAULT_BASE_TOPIC.to_string(),
            discovery_prefix: DEFAULT_DISCOVERY_PREFIX.to_string(),
            device_id: DEFAULT_DEVICE_ID.to_string(),
            username: None,
            password: None,
        }
    }

    /// Override the broker port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Override the base topic.
    pub fn with_base_topic(mut self, base_topic: impl Into<String>) -> Self {
        self.base_topic = base_topic.into();
        self
    }

    /// Override the discovery prefix.
    pub fn with_discovery_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.discovery_prefix = prefix.into();
        self
    }

    /// Override the device/node id.
    pub fn with_device_id(mut self, device_id: impl Into<String>) -> Self {
        self.device_id = device_id.into();
        self
    }

    /// Set broker credentials.
    pub fn with_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// The topic the companion's status sensor publishes its state to.
    pub fn status_state_topic(&self) -> String {
        format!("{}/status/state", self.base_topic)
    }

    /// The topic the companion's efficacy sensors publish their state to,
    /// namespaced by the sensor's object id (the persona/platform key).
    pub fn efficacy_state_topic(&self, object_id: &str) -> String {
        format!("{}/efficacy/{}/state", self.base_topic, object_id)
    }

    /// The command topic HA publishes start/pause/adjust requests to.
    pub fn command_topic(&self) -> String {
        format!("{}/campaign/command", self.base_topic)
    }

    /// The Home Assistant MQTT-discovery config topic for a sensor of the given
    /// component (`sensor`) and object id, under the discovery prefix.
    pub fn discovery_config_topic(&self, component: &str, object_id: &str) -> String {
        format!(
            "{}/{}/{}/{}/config",
            self.discovery_prefix, component, self.device_id, object_id
        )
    }
}

/// The injectable MQTT bridge seam (C8 #36), mirroring cinder's `MqttBridge`.
///
/// [`MockMqtt`] records publishes for tests/disconnected mode; the real
/// `RumqttcBridge` publishes over a cloned `AsyncClient`.
/// Object-safe (`async_trait`, `Send + Sync`) so callers hold a
/// `Box<dyn MqttBridge>` without caring which is wired.
#[async_trait]
pub trait MqttBridge: Send + Sync {
    /// Publish a Home Assistant MQTT-discovery config payload for one sensor to
    /// its discovery config topic, so HA auto-creates the entity. Retained, so
    /// HA re-discovers it after a broker/HA restart.
    async fn publish_discovery(&self, config_topic: &str, payload: &str);

    /// Publish a sensor STATE payload to its state topic. A dropped/failed
    /// publish WARNS and never crashes (HA simply keeps the last value).
    async fn publish_state(&self, state_topic: &str, payload: &str);
}

/// Records publishes (and logs them) and always succeeds (C8 #36). The default
/// bridge in tests and disconnected mode; ALWAYS compiles (no rumqttc).
///
/// Recorded publishes are kept in memory so tests can assert the exact HA-
/// discovery and state payloads without a live broker.
#[derive(Debug, Clone, Default)]
pub struct MockMqtt {
    discovery: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    state: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl MockMqtt {
    /// A fresh mock with no recorded publishes.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded `(config_topic, payload)` discovery publishes, in order.
    pub fn discovery_publishes(&self) -> Vec<(String, String)> {
        self.discovery.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// The recorded `(state_topic, payload)` state publishes, in order.
    pub fn state_publishes(&self) -> Vec<(String, String)> {
        self.state.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl MqttBridge for MockMqtt {
    async fn publish_discovery(&self, config_topic: &str, payload: &str) {
        tracing::debug!(target: "mqtt", topic = config_topic, "publish HA discovery (mock)");
        if let Ok(mut v) = self.discovery.lock() {
            v.push((config_topic.to_string(), payload.to_string()));
        }
    }

    async fn publish_state(&self, state_topic: &str, payload: &str) {
        tracing::debug!(target: "mqtt", topic = state_topic, "publish sensor state (mock)");
        if let Ok(mut v) = self.state.lock() {
            v.push((state_topic.to_string(), payload.to_string()));
        }
    }
}

/// Publish every Home Assistant MQTT-discovery sensor config in `discovery`
/// through `bridge` (C8 #36), so HA auto-creates the entities. Retained by the
/// bridge so HA re-discovers after a restart. Generic over the [`MqttBridge`]
/// seam, so the real `RumqttcBridge` publishes to a broker
/// while tests assert the payloads via [`MockMqtt`]. A serialize failure for one
/// sensor warns and is skipped (never crashes the always-on core).
pub async fn publish_discovery(bridge: &dyn MqttBridge, discovery: &DiscoveryConfig) {
    for (topic, payload) in &discovery.sensors {
        match serde_json::to_string(payload) {
            Ok(json) => bridge.publish_discovery(topic, &json).await,
            Err(e) => tracing::warn!(
                target: "mqtt", topic = %topic, error = %e,
                "discovery payload serialize failed; skipping sensor"
            ),
        }
    }
}

/// Publish the STATUS and per-segment EFFICACY state payloads through `bridge`
/// (C8 #36): the status sensor's JSON to [`MqttConfig::status_state_topic`], and
/// each efficacy reading to its [`MqttConfig::efficacy_state_topic`]. Generic over
/// the [`MqttBridge`] seam. A serialize failure warns and is skipped.
pub async fn publish_status(
    bridge: &dyn MqttBridge,
    cfg: &MqttConfig,
    status: &StatusPayload,
    efficacy: &[EfficacySensor],
) {
    match serde_json::to_string(status) {
        Ok(json) => bridge.publish_state(&cfg.status_state_topic(), &json).await,
        Err(e) => tracing::warn!(target: "mqtt", error = %e, "status payload serialize failed"),
    }
    for sensor in efficacy {
        match serde_json::to_string(sensor) {
            Ok(json) => {
                bridge
                    .publish_state(&cfg.efficacy_state_topic(&sensor.object_id()), &json)
                    .await
            }
            Err(e) => tracing::warn!(
                target: "mqtt", error = %e, "efficacy payload serialize failed"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn config_topics_are_derived_from_base_and_prefix() {
        let cfg = MqttConfig::new("ha.lan")
            .with_base_topic("fx")
            .with_discovery_prefix("ha")
            .with_device_id("fx_box");
        assert_eq!(cfg.status_state_topic(), "fx/status/state");
        assert_eq!(
            cfg.efficacy_state_topic("p1_google"),
            "fx/efficacy/p1_google/state"
        );
        assert_eq!(cfg.command_topic(), "fx/campaign/command");
        assert_eq!(
            cfg.discovery_config_topic("sensor", "status"),
            "ha/sensor/fx_box/status/config"
        );
    }

    #[test]
    fn config_round_trips() -> Result<()> {
        let cfg = MqttConfig::new("ha.lan").with_credentials("u", "p");
        let json = serde_json::to_string(&cfg)?;
        let back: MqttConfig = serde_json::from_str(&json)?;
        assert_eq!(back, cfg);
        Ok(())
    }

    #[tokio::test]
    async fn mock_records_publishes() {
        let mock = MockMqtt::new();
        mock.publish_discovery("ha/sensor/fx/status/config", "{\"name\":\"x\"}")
            .await;
        mock.publish_state("fx/status/state", "{\"running\":true}")
            .await;
        assert_eq!(mock.discovery_publishes().len(), 1);
        assert_eq!(mock.state_publishes().len(), 1);
        assert_eq!(mock.state_publishes()[0].0, "fx/status/state");
    }
}
