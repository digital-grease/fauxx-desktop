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

//! `fauxx-cli serve`: the long-running headless homelab mode (C8 #35, U4).
//!
//! Driven by a JSON config file (see [`super::serve_config`] for the schema and
//! the per-OS search path). On start it:
//!
//! 1. resolves the encrypted-store config from the serve config (headless key
//!    provisioning: the Argon2id passphrase-file [`KeySource`], no interactive
//!    prompt),
//! 2. opens the store, FAILING CLOSED with a clear error if it cannot open,
//! 3. resumes persisted campaigns (the planner already loads them from the
//!    store; serve simply re-`start`s the ones that were `Running`),
//! 4. runs a loop ticking every running campaign at the configured interval,
//! 5. optionally starts the Home Assistant MQTT bridge (only when built with the
//!    `mqtt` feature; otherwise a requested bridge is a clear error),
//! 6. shuts down gracefully on SIGINT (ctrlc).
//!
//! The thin-client rule holds: every tick and every campaign transition is a
//! core async call; serve only owns the loop, the signal handling, and the
//! config-to-`Config` resolution.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context};
use fauxx_core::{Config, Core, KeySource};
use tokio::sync::Notify;

use crate::cli::ServeArgs;
use crate::commands::serve_config::ServeConfig;

/// Run the headless serve mode.
pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let serve_config = ServeConfig::load(args.config.as_deref())?;

    // `--check`: print the effective config and exit without opening anything.
    if args.check {
        println!("{}", serde_json::to_string_pretty(&serve_config)?);
        return Ok(());
    }

    // Live LAN sync (C1 #7) is opt-in: the `--lan-sync` flag OR the config flag.
    // When on, the core attaches the mDNS + TCP seams at open time.
    let lan_sync_enabled = args.lan_sync || serve_config.lan_sync;

    let mut config = resolve_store_config(&serve_config)?.with_lan_sync(lan_sync_enabled);
    if let Some(port) = serve_config.sync_port {
        config = config.with_sync_port(port);
    }

    // Fail closed: a store that cannot open is a hard error, never an
    // unencrypted or partial start.
    let core = Core::open(config)
        .await
        .context("opening the encrypted store (serve fails closed)")?;
    let status = core.status().await?;
    tracing::info!(
        version = status.version,
        persona_count = status.persona_count,
        "serve: store opened, resuming campaigns"
    );

    // Resume persisted campaigns: the planner already loaded them from the
    // store; re-start the ones recorded as Running so the loop drives them.
    let resumed = resume_running_campaigns(&core).await?;
    tracing::info!(resumed, "serve: resumed running campaigns");

    // Optionally start the Home Assistant MQTT bridge (only with the `mqtt`
    // feature; otherwise a requested bridge is a clear, fail-closed error).
    let mqtt_enabled = args.mqtt
        || serve_config
            .mqtt
            .as_ref()
            .map(|m| m.enabled)
            .unwrap_or(false);
    #[cfg(feature = "mqtt")]
    let mqtt = if mqtt_enabled {
        Some(MqttRuntime::start(&serve_config)?)
    } else {
        None
    };
    #[cfg(not(feature = "mqtt"))]
    if mqtt_enabled {
        return Err(mqtt_feature_disabled());
    }

    run_loop(
        &core,
        &serve_config,
        args.max_ticks,
        lan_sync_enabled,
        #[cfg(feature = "mqtt")]
        mqtt,
    )
    .await?;

    tracing::info!("serve: shutdown complete");
    drop(core);
    Ok(())
}

/// Resolve the encrypted-store [`Config`] from the serve config. The headless
/// path reads the passphrase from the configured file (the Argon2id
/// passphrase-file key source); with no passphrase file it falls back to the OS
/// keystore. Fails closed on an unreadable/empty passphrase file.
fn resolve_store_config(serve: &ServeConfig) -> anyhow::Result<Config> {
    let mut config = Config::new();
    if let Some(db) = &serve.db_path {
        config = config.with_path(db.clone());
    }
    if let Some(passphrase_file) = &serve.passphrase_file {
        let passphrase = read_passphrase_file(passphrase_file)?;
        let key_path = resolve_key_path(serve);
        config = config.with_key_source(KeySource::EncryptedFile {
            path: key_path,
            passphrase,
        });
    }
    Ok(config)
}

/// Resolve the key file path: the explicit `keyFile`, else `<db>.key` beside the
/// database, else the per-OS default `<data dir>/fauxx.db.key`.
fn resolve_key_path(serve: &ServeConfig) -> PathBuf {
    if let Some(key_file) = &serve.key_file {
        return key_file.clone();
    }
    let db_path = serve
        .db_path
        .clone()
        .or_else(|| fauxx_core::store::EncryptedStore::default_path().ok())
        .unwrap_or_else(|| PathBuf::from("fauxx.db"));
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".key");
    db_path.with_file_name(name)
}

/// Read a passphrase from a file, trimming a single trailing newline. Fails
/// closed on an unreadable or empty file (mirrors the global StoreOpts resolver).
fn read_passphrase_file(path: &std::path::Path) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading passphrase file {}", path.display()))?;
    let trimmed = raw.strip_suffix('\n').unwrap_or(&raw);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.is_empty() {
        bail!("passphrase file {} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

/// Re-start every campaign the store recorded as `Running` so the serve loop
/// drives it. Returns the count resumed. A campaign in any other state is left
/// as-is (the operator starts it explicitly).
async fn resume_running_campaigns(core: &Core) -> anyhow::Result<usize> {
    let mut resumed = 0usize;
    for campaign in core.list_campaigns(None).await? {
        if campaign.status == fauxx_core::CampaignStatus::Running {
            core.start_campaign(&campaign.id, now_millis()).await?;
            resumed += 1;
        }
    }
    Ok(resumed)
}

/// The serve loop: tick every running campaign each interval until SIGINT (or
/// `max_ticks`, when set, for one-shot / test runs). When the `mqtt` feature is
/// on and a bridge was started, each tick also publishes the HA status/efficacy
/// sensors and routes any inbound HA campaign commands (C8 #36).
async fn run_loop(
    core: &Core,
    serve: &ServeConfig,
    max_ticks: Option<u64>,
    lan_sync_enabled: bool,
    #[cfg(feature = "mqtt")] mut mqtt: Option<MqttRuntime>,
) -> anyhow::Result<()> {
    let interval = serve.tick_interval();

    let shutdown = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    let handler_flag = Arc::clone(&shutdown);
    let handler_notify = Arc::clone(&notify);
    ctrlc::set_handler(move || {
        handler_flag.store(true, Ordering::SeqCst);
        // `notify_waiters` (not `notify_one`) so BOTH the tick loop and the LAN
        // sync inbound listener, when running, observe the shutdown.
        handler_notify.notify_waiters();
    })
    .context("installing Ctrl-C handler")?;

    // Bring up live LAN sync (C1 #7) when enabled: advertise this device over
    // mDNS and spawn the inbound listener for sealed persona frames. The listener
    // runs for the life of the loop and stops on the same shutdown signal.
    let listener_handle = if lan_sync_enabled {
        if let Err(e) = core.advertise_sync().await {
            tracing::warn!(error = %e, "serve: LAN sync advertise failed (continuing)");
        }
        let listener_core = core.clone();
        let listener_notify = Arc::clone(&notify);
        match core.sync_listen_addr() {
            Ok(addr) => {
                tracing::info!(%addr, "serve: LAN sync enabled, starting inbound listener");
                Some(tokio::spawn(async move {
                    if let Err(e) = listener_core.run_sync_listener(listener_notify).await {
                        tracing::error!(error = %e, "serve: LAN sync listener exited with error");
                    }
                }))
            }
            Err(e) => {
                tracing::warn!(error = %e, "serve: LAN sync listener not started");
                None
            }
        }
    } else {
        None
    };

    tracing::info!(
        interval_secs = interval.as_secs(),
        max_ticks = ?max_ticks,
        lan_sync = lan_sync_enabled,
        "serve: entering tick loop (Ctrl-C to stop)"
    );

    let mut ticks: u64 = 0;
    while !shutdown.load(Ordering::SeqCst) {
        // Tick advances each running campaign's closed loop and PERSISTS its
        // progress; the resulting directives steer the next plan any executor
        // builds for that persona (the extension decoy plan, the household
        // schedule), since those consult `campaign_directive_for_persona`. serve
        // itself does not drive a browser (headless live execution is the deferred
        // C2/C3 path); it owns the loop and surfaces what each campaign is driving.
        let directives = core.tick_running_campaigns(now_millis()).await?;
        let driving = directives.iter().filter(|(_, d)| d.is_running()).count();
        if !directives.is_empty() {
            tracing::info!(
                campaigns = directives.len(),
                driving,
                "serve: ticked running campaigns"
            );
            for (id, directive) in &directives {
                if let Some(intensity) = directive.intensity {
                    tracing::debug!(
                        campaign = %id,
                        ?intensity,
                        target_segment = %directive.target_segment,
                        "serve: campaign directive steers the next plan for its persona"
                    );
                }
            }
        }
        // Rotate any persona whose window has elapsed (C5 #24): mint a fresh
        // identity in the same slot, preserving locked fields. This is what makes
        // a persona's identity churn over time in the always-on deployment.
        let rotated = core.rotate_due_personas(now_millis()).await?;
        if !rotated.is_empty() {
            tracing::info!(personas = rotated.len(), "serve: rotated due personas");
        }
        // Refresh the LAN sync routing table from freshly mDNS-discovered peers
        // (C1 #7) so a newly-seen peer becomes reachable for an outbound push.
        if lan_sync_enabled {
            match core.refresh_sync_routes().await {
                Ok(n) if n > 0 => tracing::debug!(routes = n, "serve: refreshed LAN sync routes"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "serve: LAN sync route refresh failed"),
            }
        }
        // Publish HA status/efficacy + route inbound HA commands (C8 #36).
        #[cfg(feature = "mqtt")]
        if let Some(mqtt) = mqtt.as_mut() {
            mqtt.tick(core).await;
        }
        ticks += 1;
        if let Some(max) = max_ticks {
            if ticks >= max {
                tracing::info!(ticks, "serve: reached max_ticks, stopping");
                break;
            }
        }
        // Wait for the interval OR an early shutdown signal, whichever first.
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = notify.notified() => {
                tracing::info!("serve: shutdown signal received");
                break;
            }
        }
    }
    // Stop the LAN sync listener (it may not have been notified when the loop
    // exits via `max_ticks` rather than Ctrl-C). Notify again to release a waiting
    // listener, then abort to guarantee the task is gone before serve returns.
    if let Some(handle) = listener_handle {
        notify.notify_waiters();
        handle.abort();
        let _ = handle.await;
    }
    Ok(())
}

/// The live Home Assistant MQTT bridge runtime (C8 #36), held for the lifetime
/// of the serve loop. Owns the single shared connection's publish handle + the
/// inbound-command receiver; [`tick`](MqttRuntime::tick) publishes the
/// status/efficacy sensors and routes any inbound HA commands each loop turn.
///
/// Enabling the bridge requires the `mqtt` feature (which turns on the
/// `fauxx-core` `mqtt` feature; see `apps/cli/Cargo.toml`). Without it, a
/// requested bridge is a clear, fail-closed error ([`mqtt_feature_disabled`]),
/// never a silent no-op.
#[cfg(feature = "mqtt")]
struct MqttRuntime {
    bridge: fauxx_core::mqtt::RumqttcBridge,
    commands: tokio::sync::mpsc::Receiver<Vec<u8>>,
    cfg: fauxx_core::MqttConfig,
    /// The efficacy-sensor object-id set last announced via HA discovery, so
    /// discovery is (re)published only when the running-campaign set changes
    /// (and once up front, so the status sensor is always announced). `None`
    /// until the first publish.
    announced: Option<Vec<String>>,
}

#[cfg(feature = "mqtt")]
impl MqttRuntime {
    /// Open the single shared rumqttc connection from the serve config. Never
    /// fails on a down broker (rumqttc reconnects); only a missing `[mqtt]`
    /// block is an error.
    fn start(serve: &ServeConfig) -> anyhow::Result<Self> {
        let mqtt = serve
            .mqtt
            .clone()
            .context("mqtt enabled but no [mqtt] config block present")?;
        let mut cfg = fauxx_core::MqttConfig::new(mqtt.host)
            .with_port(mqtt.port)
            .with_base_topic(mqtt.base_topic)
            .with_discovery_prefix(mqtt.discovery_prefix)
            .with_device_id(mqtt.device_id);
        if let (Some(user), Some(pass)) = (mqtt.username, mqtt.password) {
            cfg = cfg.with_credentials(user, pass);
        }
        // connect() builds the single shared client + poll task and never errors
        // on a down broker (rumqttc reconnects); a DOWN broker is non-fatal.
        let connection = fauxx_core::mqtt::connect(&cfg);
        tracing::info!(
            host = %cfg.host,
            port = cfg.port,
            "serve: Home Assistant MQTT bridge started"
        );
        Ok(Self {
            bridge: connection.bridge,
            commands: connection.commands,
            cfg,
            announced: None,
        })
    }

    /// One loop turn: (re)publish HA discovery if the sensor set changed, publish
    /// the current status + efficacy state, then drain and route any inbound HA
    /// campaign commands. Every publish is fire-and-warn (a down broker or a bad
    /// snapshot never stops the loop).
    async fn tick(&mut self, core: &Core) {
        let (status, efficacy) = match core.mqtt_status_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!(error = %e, "serve: mqtt status snapshot failed; skipping publish");
                return;
            }
        };

        // (Re)announce HA discovery when the efficacy-sensor set changes (and on
        // the first tick, so the status sensor is always created). Discovery is
        // retained, so HA keeps the entities across restarts.
        let mut object_ids: Vec<String> = efficacy.iter().map(|s| s.object_id()).collect();
        object_ids.sort();
        object_ids.dedup();
        if self.announced.as_deref() != Some(object_ids.as_slice()) {
            let discovery = fauxx_core::mqtt::DiscoveryConfig::build(&self.cfg, &object_ids);
            fauxx_core::mqtt::publish_discovery(&self.bridge, &discovery).await;
            self.announced = Some(object_ids);
        }

        fauxx_core::mqtt::publish_status(&self.bridge, &self.cfg, &status, &efficacy).await;

        // Drain inbound HA commands non-blockingly and route each into the planner
        // (try_recv ends the drain on Empty or Disconnected; the poll task keeps
        // the channel fed across reconnects).
        while let Ok(payload) = self.commands.try_recv() {
            match core.route_campaign_command(&payload, now_millis()).await {
                Ok(campaign) => tracing::info!(
                    campaign = %campaign.id,
                    status = campaign.status.as_str(),
                    "serve: routed Home Assistant campaign command"
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "serve: Home Assistant campaign command rejected")
                }
            }
        }
    }
}

/// The fail-closed error when the bridge is requested but the `mqtt` feature is
/// off, so an operator who asked for HA control is never misled by a silent
/// no-op.
#[cfg(not(feature = "mqtt"))]
fn mqtt_feature_disabled() -> anyhow::Error {
    anyhow::anyhow!(
        "the Home Assistant MQTT bridge was requested but this binary was built without the \
         `mqtt` feature; rebuild with `cargo build -p fauxx-cli --features mqtt`"
    )
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
