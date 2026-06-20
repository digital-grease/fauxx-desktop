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

//! GUI-local desktop settings (the C5-style Settings screen backing store).
//!
//! `fauxx-core` exposes no generic GUI-setting KV (its store is reachable only
//! through typed domain methods, and these are GUI-only preferences), so the
//! settings live in a small JSON file under the OS config dir, beside the
//! first-run marker (see [`crate::firstrun`]). They are intentionally NOT in the
//! encrypted store: they carry no secret and some of them (device name, LAN-sync
//! toggle, sync port) must be readable BEFORE the store opens, to build the core
//! [`fauxx_core::Config`] at boot.
//!
//! Reads are best-effort and infallible (a missing or malformed file falls back
//! to defaults). Writes return a typed error so the Settings screen can surface
//! a save failure rather than swallow it.

use serde::{Deserialize, Serialize};

/// The settings filename written under the app config directory.
const SETTINGS_FILE: &str = "settings.json";

/// The OS qualifier/org/app triple, matching [`crate::firstrun`] and the core's
/// store-path derivation so the desktop config lands beside the core data dir.
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";

/// The window theme the user picked. `Light` is the default (matches the look
/// the app shipped with). System-theme detection needs an extra per-OS
/// dependency and is a follow-up, so only the two explicit choices exist today.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    /// The light palette (default).
    #[default]
    Light,
    /// The dark palette.
    Dark,
}

impl ThemeChoice {
    /// The short display label for the picker.
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    /// All choices in display order, for rendering the picker.
    pub fn all() -> [ThemeChoice; 2] {
        [ThemeChoice::Light, ThemeChoice::Dark]
    }

    /// Map to the concrete iced theme the application renders with.
    pub fn to_theme(self) -> iced::Theme {
        match self {
            ThemeChoice::Light => iced::Theme::Light,
            ThemeChoice::Dark => iced::Theme::Dark,
        }
    }
}

/// The smallest auto-refresh interval the user may pick, in seconds. Below this
/// the periodic status reload would hammer the core for no benefit.
pub const MIN_REFRESH_SECS: u64 = 1;
/// A sane upper bound for the auto-refresh interval picker, in seconds.
pub const MAX_REFRESH_SECS: u64 = 60;

/// The GUI-local desktop preferences. Serialized to `settings.json` under the
/// OS config dir. Every field has a default so a partial or absent file still
/// produces a usable value (`#[serde(default)]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DesktopSettings {
    /// The window theme (Light or Dark).
    pub theme: ThemeChoice,
    /// Seconds between the periodic status refresh on the Running screen.
    pub auto_refresh_secs: u64,
    /// When `true` (the default), the window-manager close button hides the
    /// window to the tray; when `false`, it quits the whole agent.
    pub close_to_tray: bool,
    /// The mDNS device name advertised for pairing. `None` lets the core derive
    /// one from the hostname. Applied at the next start (the sync identity is
    /// built when the store opens).
    pub device_name: Option<String>,
    /// Whether to bring up live LAN persona sync at start. Applied at the next
    /// start (the sync seams attach when the store opens).
    pub lan_sync: bool,
    /// The TCP port for the LAN-sync listener. `None` uses the core default.
    /// Applied at the next start.
    pub sync_port: Option<u16>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        // These defaults reproduce the behavior the app shipped with before the
        // Settings screen existed: light theme, a 2s tick, close-to-tray on, and
        // LAN sync enabled with the core-derived device name and default port.
        Self {
            theme: ThemeChoice::Light,
            auto_refresh_secs: 2,
            close_to_tray: true,
            device_name: None,
            lan_sync: true,
            sync_port: None,
        }
    }
}

impl DesktopSettings {
    /// The clamped auto-refresh interval as a `Duration`, for the subscription
    /// tick. Clamped into `[MIN_REFRESH_SECS, MAX_REFRESH_SECS]` so a hand-edited
    /// or stale file can never produce a zero (busy-loop) or absurd interval.
    pub fn refresh_interval(&self) -> std::time::Duration {
        let secs = self
            .auto_refresh_secs
            .clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS);
        std::time::Duration::from_secs(secs)
    }

    /// The trimmed device name, or `None` when blank (so an empty text box reads
    /// as "let the core derive it" rather than an empty advertised name).
    pub fn device_name_trimmed(&self) -> Option<String> {
        self.device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

/// The absolute path to the settings file, or `None` if no OS config directory
/// can be determined.
fn settings_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .map(|dirs| dirs.config_dir().join(SETTINGS_FILE))
}

/// Load the persisted settings, falling back to defaults on any problem (no
/// config dir, missing file, or malformed JSON). Never fails: a settings file
/// the user cannot read should not block the window from coming up.
pub fn load() -> DesktopSettings {
    let Some(path) = settings_path() else {
        return DesktopSettings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|err| {
            tracing::warn!(
                "settings file at {} is malformed, using defaults: {err}",
                path.display()
            );
            DesktopSettings::default()
        }),
        Err(_) => DesktopSettings::default(),
    }
}

/// Persist the settings to the config file, creating the config dir if needed.
/// Returns a human-readable error on failure so the Settings screen can surface
/// it (unlike the best-effort first-run marker, a failed Save should be visible).
pub fn save(settings: &DesktopSettings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "no OS config directory available".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create config dir: {err}"))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("could not serialize: {err}"))?;
    std::fs::write(&path, json).map_err(|err| format!("could not write settings: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_reproduce_prior_behavior() {
        let d = DesktopSettings::default();
        assert_eq!(d.theme, ThemeChoice::Light);
        assert_eq!(d.auto_refresh_secs, 2);
        assert!(d.close_to_tray);
        assert!(d.lan_sync);
        assert_eq!(d.device_name, None);
        assert_eq!(d.sync_port, None);
    }

    #[test]
    fn refresh_interval_is_clamped() {
        // Struct-literal construction (not default-then-assign) to satisfy the
        // workspace clippy lints; `..Default::default()` fills the rest.
        let low = DesktopSettings {
            auto_refresh_secs: 0,
            ..Default::default()
        };
        assert_eq!(low.refresh_interval().as_secs(), MIN_REFRESH_SECS);
        let high = DesktopSettings {
            auto_refresh_secs: 9999,
            ..Default::default()
        };
        assert_eq!(high.refresh_interval().as_secs(), MAX_REFRESH_SECS);
        let mid = DesktopSettings {
            auto_refresh_secs: 5,
            ..Default::default()
        };
        assert_eq!(mid.refresh_interval().as_secs(), 5);
    }

    #[test]
    fn device_name_blank_reads_as_none() {
        let blank = DesktopSettings {
            device_name: Some("   ".to_string()),
            ..Default::default()
        };
        assert_eq!(blank.device_name_trimmed(), None);
        let padded = DesktopSettings {
            device_name: Some("  Den PC ".to_string()),
            ..Default::default()
        };
        assert_eq!(padded.device_name_trimmed().as_deref(), Some("Den PC"));
    }

    #[test]
    fn settings_round_trip_through_json() -> Result<(), Box<dyn std::error::Error>> {
        let d = DesktopSettings {
            theme: ThemeChoice::Dark,
            auto_refresh_secs: 10,
            close_to_tray: false,
            device_name: Some("Studio".to_string()),
            lan_sync: false,
            sync_port: Some(45999),
        };
        let json = serde_json::to_string(&d)?;
        let back: DesktopSettings = serde_json::from_str(&json)?;
        assert_eq!(d, back);
        Ok(())
    }

    #[test]
    fn partial_json_fills_defaults() -> Result<(), Box<dyn std::error::Error>> {
        // A file written by an older build with only some keys still loads.
        let back: DesktopSettings = serde_json::from_str(r#"{"theme":"dark"}"#)?;
        assert_eq!(back.theme, ThemeChoice::Dark);
        assert_eq!(back.auto_refresh_secs, 2);
        assert!(back.lan_sync);
        Ok(())
    }
}
