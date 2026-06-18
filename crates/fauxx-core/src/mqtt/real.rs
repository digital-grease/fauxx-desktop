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

//! The real `rumqttc` bridge (C8 #36, U5), behind the off-by-default `mqtt`
//! feature. Mirrors `cinder/src/mqtt/mod.rs`.
//!
//! [`connect`] builds the SINGLE shared `AsyncClient` + `EventLoop`, spawns ONE
//! poll task that re-subscribes the command topic on every `ConnAck` and routes
//! inbound command messages onto a bounded mpsc (drop + warn, never block), and
//! warns + sleeps on a poll error (rumqttc auto-reconnects). A DOWN broker is
//! NON-fatal: the core comes up and degrades.
//!
//! The publish handle, [`RumqttcBridge`], holds a CLONED `AsyncClient` (a
//! request-channel sender), NEVER an `Arc<Mutex>`. Every publish is fire-and-
//! warn: a failed/dropped publish logs a warning and never crashes the process.

use std::time::Duration;

use async_trait::async_trait;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

use super::{MqttBridge, MqttConfig};

/// Depth of the inbound command channel from the poll task to the router. A
/// bounded channel so a stalled router cannot grow memory without bound; an
/// overflow drops (and warns) rather than blocking the poll loop.
const COMMAND_CHANNEL_CAP: usize = 32;
/// rumqttc request-channel capacity for the `AsyncClient` (its docs suggest ~10
/// for light publishers; 32 leaves headroom for the discovery burst).
const REQUEST_CHANNEL_CAP: usize = 32;
/// Pace between poll-loop reconnect attempts after a connection error.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
/// Keep-alive interval for the broker connection.
const KEEP_ALIVE: Duration = Duration::from_secs(30);

/// The owned ends of the single shared rumqttc connection (C8 #36), returned by
/// [`connect`]. The caller clones `bridge`'s client for publishing and drains
/// `commands` for inbound HA campaign commands. The poll task is already spawned
/// and detached.
pub struct MqttConnection {
    /// The publish bridge (a cloned `AsyncClient` inside).
    pub bridge: RumqttcBridge,
    /// Inbound raw command-topic payloads, to be parsed + routed into the U2
    /// planner by the caller (kept as raw bytes so the bridge stays free of the
    /// campaign types and the routing policy lives in the orchestration layer).
    pub commands: mpsc::Receiver<Vec<u8>>,
}

/// Open the SINGLE shared connection (C8 #36): build `AsyncClient` + `EventLoop`,
/// spawn ONE poll task that re-subscribes the command topic on every `ConnAck`
/// and routes inbound command messages onto a bounded mpsc.
///
/// Never returns an error for a down broker (rumqttc reconnects on the next
/// poll); it only logs. The connection comes up and degrades. Never panics.
pub fn connect(cfg: &MqttConfig) -> MqttConnection {
    let command_topic = cfg.command_topic();

    let mut opts = MqttOptions::new(cfg.device_id.clone(), cfg.host.clone(), cfg.port);
    opts.set_keep_alive(KEEP_ALIVE);
    if let (Some(user), Some(pass)) = (cfg.username.clone(), cfg.password.clone()) {
        opts.set_credentials(user, pass);
    }

    let (client, mut eventloop) = AsyncClient::new(opts, REQUEST_CHANNEL_CAP);
    let (command_tx, command_rx) = mpsc::channel::<Vec<u8>>(COMMAND_CHANNEL_CAP);

    // The subscribe handle for the poll task (the client itself is cloned into
    // the bridge for publishing).
    let sub_client = client.clone();
    let host = cfg.host.clone();
    let port = cfg.port;

    tokio::spawn(async move {
        // The broker HOST (often an internal FQDN / home domain) goes to DEBUG
        // only, so it is not written to the persisted log at the default `info`
        // level and cannot leak into a scrubbed bug-report export. The info line
        // keeps the non-identifying port + topic.
        tracing::info!(
            target: "mqtt",
            port, topic = %command_topic,
            "mqtt poll task started"
        );
        tracing::debug!(target: "mqtt", %host, "mqtt broker host");
        loop {
            match eventloop.poll().await {
                // Connected (or reconnected): (re)subscribe so commands resume.
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    match sub_client.subscribe(&command_topic, QoS::AtLeastOnce).await {
                        Ok(()) => tracing::info!(
                            target: "mqtt", topic = %command_topic, "subscribed command topic"
                        ),
                        Err(e) => tracing::warn!(
                            target: "mqtt", error = %e,
                            "command subscribe failed; will retry on reconnect"
                        ),
                    }
                }
                // Inbound message: only the command topic is interesting.
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    if p.topic == command_topic {
                        // Drop (warn) rather than block the poll loop if the
                        // router is gone or backed up.
                        if command_tx.try_send(p.payload.to_vec()).is_err() {
                            tracing::warn!(
                                target: "mqtt",
                                "campaign command dropped (router closed or full)"
                            );
                        }
                    }
                }
                // Any other packet / outgoing event: nothing to do.
                Ok(_) => {}
                // Connection error: log and pace; rumqttc reconnects next poll.
                Err(e) => {
                    tracing::warn!(
                        target: "mqtt", error = %e,
                        "mqtt connection error; reconnecting"
                    );
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                }
            }
        }
    });

    MqttConnection {
        bridge: RumqttcBridge::from_client(client),
        commands: command_rx,
    }
}

/// The connected publish bridge (C8 #36). Holds a CLONED [`AsyncClient`] (a
/// request-channel sender), never an `Arc<Mutex>`. Every publish is fire-and-
/// warn: a failed publish logs and continues (a dropped status update must
/// never take the always-on core down).
pub struct RumqttcBridge {
    client: Option<AsyncClient>,
}

impl RumqttcBridge {
    /// Build from the shared connection's client (the deployed happy path).
    pub fn from_client(client: AsyncClient) -> Self {
        Self {
            client: Some(client),
        }
    }

    /// A bridge with no client: publishes are warned-and-dropped. Reached only
    /// if the caller wants a type-correct bridge before/without a connection;
    /// keeps the always-on core up with no broker.
    pub fn disconnected() -> Self {
        Self { client: None }
    }

    /// Publish `payload` to `topic` at QoS 1. Discovery configs are retained so
    /// HA re-discovers after a restart; state is not (HA keeps the last value
    /// from its own retained-state handling / templates).
    async fn publish(&self, topic: &str, payload: &str, retain: bool) {
        let Some(client) = &self.client else {
            tracing::warn!(target: "mqtt", topic, "publish dropped: no mqtt client");
            return;
        };
        if let Err(e) = client
            .publish(topic, QoS::AtLeastOnce, retain, payload.as_bytes().to_vec())
            .await
        {
            tracing::warn!(target: "mqtt", topic, error = %e, "publish failed");
        }
    }
}

#[async_trait]
impl MqttBridge for RumqttcBridge {
    async fn publish_discovery(&self, config_topic: &str, payload: &str) {
        // Retained so HA re-creates the entity after a broker/HA restart.
        self.publish(config_topic, payload, true).await;
    }

    async fn publish_state(&self, state_topic: &str, payload: &str) {
        self.publish(state_topic, payload, false).await;
    }
}
