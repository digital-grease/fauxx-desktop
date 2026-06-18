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

//! The `fauxx serve` configuration file schema and its per-OS search path
//! (C8 #35, the homelab mode).
//!
//! The serve config is JSON (serde_json, no new dependency). It is OPTIONAL: a
//! missing file uses the documented defaults below. The search order, when no
//! explicit `--config` path is given, is:
//!
//! 1. `$FAUXX_SERVE_CONFIG` (handled at the CLI layer via the env-backed flag),
//! 2. `<per-OS config dir>/serve.json` (the [`directories`] project config dir,
//!    e.g. `~/.config/fauxx/serve.json` on Linux),
//!
//! A documented example lives in `deploy/serve.example.json`; the full schema is
//! documented in `docs/DEPLOYMENT.md`.
//!
//! The config never carries the STORE passphrase inline: it points at a
//! passphrase FILE (the headless Argon2id passphrase-file KeySource), so the
//! secret stays in a file the operator controls (and systemd can mode-0600), not
//! in the config JSON.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

/// The same application qualifier/org/app the core store uses, so the serve
/// config sits beside the data dir under one project namespace.
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";

/// The default config file name under the per-OS config dir.
const CONFIG_FILE: &str = "serve.json";

/// The default seconds between campaign-tick loop iterations.
const DEFAULT_TICK_INTERVAL_SECS: u64 = 60;

/// The persisted `fauxx serve` configuration (C8 #35).
///
/// Every field has a default so a minimal (or absent) file still runs. Field
/// names are camelCase on the wire to match the rest of the project's JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServeConfig {
    /// The encrypted store path. `None` resolves to the per-OS data dir (the
    /// core default). Mirrors the global `--db` flag.
    pub db_path: Option<PathBuf>,

    /// The passphrase FILE the headless Argon2id key source reads. When set,
    /// serve opens the store with the encrypted-key-file key source (no OS
    /// keystore, no interactive prompt). When `None`, serve uses the OS
    /// keystore (only sensible on a host with a working Secret Service).
    pub passphrase_file: Option<PathBuf>,

    /// The Argon2id-wrapped key file path. `None` derives `<db>.key` beside the
    /// database, matching the CLI default.
    pub key_file: Option<PathBuf>,

    /// Seconds between campaign-tick loop iterations. Clamped to at least 1.
    pub tick_interval_secs: u64,

    /// The Home Assistant MQTT bridge configuration. `None` (or absent) leaves
    /// the bridge OFF. Enabling it requires building the binary with the `mqtt`
    /// feature; see `docs/DEPLOYMENT.md`.
    pub mqtt: Option<ServeMqttConfig>,

    /// Whether to bring up live LAN persona sync (C1 #7): mDNS advertise/browse +
    /// the TCP inbound listener for sealed persona frames. `false` (the default)
    /// opens no sockets and advertises nothing; the `--lan-sync` flag also forces
    /// this on. See `docs/DEPLOYMENT.md`.
    pub lan_sync: bool,

    /// The TCP port the LAN sync listener binds and advertises. `None` uses the
    /// core default. Set it to avoid a conflict when several Fauxx instances run
    /// on one host. Only relevant when `lan_sync` is on.
    pub sync_port: Option<u16>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            passphrase_file: None,
            key_file: None,
            tick_interval_secs: DEFAULT_TICK_INTERVAL_SECS,
            mqtt: None,
            lan_sync: false,
            sync_port: None,
        }
    }
}

/// The MQTT-bridge slice of the serve config (C8 #36 U5).
///
/// A plain mirror of the fields `fauxx_core::MqttConfig` needs, so the serve
/// config stays self-describing without depending on the core type's exact
/// shape. `enabled` lets an operator keep the broker details but toggle the
/// bridge off without deleting them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServeMqttConfig {
    /// Whether the bridge is enabled (the `--mqtt` flag also forces this on).
    pub enabled: bool,
    /// The broker host (the Home Assistant / Mosquitto LAN address).
    pub host: String,
    /// The broker TCP port (plaintext MQTT, 1883 by default).
    pub port: u16,
    /// The base topic the companion publishes its state/commands under.
    pub base_topic: String,
    /// The Home Assistant MQTT-discovery prefix.
    pub discovery_prefix: String,
    /// The device/node id used in topics and the HA device registry.
    pub device_id: String,
    /// Optional broker username (never logged).
    pub username: Option<String>,
    /// Optional broker password (never logged).
    pub password: Option<String>,
}

impl Default for ServeMqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "localhost".to_string(),
            port: fauxx_core::DEFAULT_MQTT_PORT,
            base_topic: fauxx_core::DEFAULT_BASE_TOPIC.to_string(),
            discovery_prefix: fauxx_core::DEFAULT_DISCOVERY_PREFIX.to_string(),
            device_id: "fauxx_desktop".to_string(),
            username: None,
            password: None,
        }
    }
}

impl ServeConfig {
    /// The effective tick interval, clamped to at least one second so the loop
    /// always makes forward progress.
    pub fn tick_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.tick_interval_secs.max(1))
    }

    /// Load the serve config. With an explicit `path`, that file MUST exist and
    /// parse (fail closed). With `None`, the default config-dir path is tried; a
    /// missing default file yields the documented defaults rather than an error.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        match path {
            Some(p) => {
                if !p.exists() {
                    bail!("serve config {} does not exist", p.display());
                }
                Self::from_file(p)
            }
            None => match default_config_path() {
                Some(p) if p.exists() => Self::from_file(&p),
                _ => Ok(Self::default()),
            },
        }
    }

    /// Parse a serve config from a JSON file, failing closed on malformed JSON.
    fn from_file(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading serve config {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parsing serve config {}", path.display()))
    }
}

/// The default per-OS serve config path (`<config dir>/serve.json`), or `None`
/// when no home directory can be determined (a headless container can always
/// pass `--config` explicitly instead).
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .map(|dirs| dirs.config_dir().join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = ServeConfig::default();
        assert!(cfg.db_path.is_none());
        assert!(cfg.mqtt.is_none());
        assert_eq!(cfg.tick_interval().as_secs(), DEFAULT_TICK_INTERVAL_SECS);
    }

    #[test]
    fn missing_explicit_config_is_an_error() {
        let res = ServeConfig::load(Some(Path::new("/nonexistent/serve.json")));
        assert!(res.is_err());
    }

    #[test]
    fn partial_config_fills_defaults() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("serve.json");
        std::fs::write(&path, r#"{ "tickIntervalSecs": 5 }"#)?;
        let cfg = ServeConfig::load(Some(&path))?;
        assert_eq!(cfg.tick_interval_secs, 5);
        assert!(cfg.mqtt.is_none());
        Ok(())
    }

    #[test]
    fn zero_tick_interval_is_clamped_to_one() {
        let cfg = ServeConfig {
            tick_interval_secs: 0,
            ..Default::default()
        };
        assert_eq!(cfg.tick_interval().as_secs(), 1);
    }
}
