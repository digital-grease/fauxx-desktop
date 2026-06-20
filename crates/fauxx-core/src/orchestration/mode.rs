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

//! Coordination modes and per-device persona assignment (C1 #8, O2).
//!
//! The household runs in one of two explicit, persisted [`CoordinationMode`]s:
//!
//! - [`CoordinationMode::CoherentHousehold`]: every paired device presents the
//!   SAME persona, advancing together at the phone's frozen 8-to-10-day cadence
//!   ([`active_until`](crate::persona::SyntheticPersona::active_until)). One
//!   elected persona is propagated to
//!   all devices over the O1 sealed channel; on rotation, all devices advance
//!   in lockstep. This models one person whose devices share an identity.
//! - [`CoordinationMode::Fragmentation`]: each paired device is assigned a
//!   DISTINCT persona with independently tracked timing. This is the input
//!   contract O3 (WAN-IP linkage risk) and O4 (non-correlated timing) consume.
//!
//! The mode and the per-device assignment are scalar coordination state, so
//! they persist in the encrypted store (the `orchestration_kv` and
//! `device_assignments` tables) and survive restart. They are *not* secret, but
//! the set is household-coordination state, so it lives behind SQLCipher with
//! the persona cache.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The orchestration-key under which the active [`CoordinationMode`] persists in
/// the `orchestration_kv` table.
pub(crate) const MODE_KEY: &str = "coordination_mode";

/// The reserved `device_key` for THIS device's own persona assignment in the
/// `device_assignments` table. Paired peers use their base64url public key.
pub(crate) const SELF_DEVICE_KEY: &str = "";

/// How the household coordinates personas across paired devices.
///
/// Explicit and persisted, selectable from the core API so the GUI and the
/// headless CLI both set it the same way. Defaults to
/// [`CoordinationMode::CoherentHousehold`] (the safest, simplest posture: one
/// shared identity that advances together) when no mode has been chosen yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CoordinationMode {
    /// One persona, pinned and propagated to every paired device; all devices
    /// advance together on rotation. Models a single shared identity. This is
    /// the default (the safest, simplest posture) until a mode is chosen.
    #[default]
    CoherentHousehold,
    /// Each paired device gets a distinct persona with independent timing.
    /// Models deliberately divergent per-device identities.
    Fragmentation,
}

impl CoordinationMode {
    /// The stable wire/persistence string for this mode.
    pub fn as_str(&self) -> &'static str {
        match self {
            CoordinationMode::CoherentHousehold => "CoherentHousehold",
            CoordinationMode::Fragmentation => "Fragmentation",
        }
    }

    /// Parse a mode from its persisted string, failing closed on an unknown
    /// value (rather than silently defaulting).
    pub fn from_str_strict(s: &str) -> Result<Self> {
        match s {
            "CoherentHousehold" => Ok(CoordinationMode::CoherentHousehold),
            "Fragmentation" => Ok(CoordinationMode::Fragmentation),
            other => Err(CoreError::Orchestration(format!(
                "unknown coordination mode {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for CoordinationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single device-to-persona assignment, surfaced over the core API so a
/// client can render which device presents which persona.
///
/// `device_key` is the base64url public key of a paired peer, or empty for this
/// device itself (see `SELF_DEVICE_KEY`). `is_self` flags the local device so
/// a client does not have to know the empty-key convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAssignment {
    /// The device's base64url public key, or empty for this device.
    pub device_key: String,
    /// Whether this assignment is for the local device.
    pub is_self: bool,
    /// The id of the persona assigned to this device.
    pub persona_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_string_round_trips() -> Result<()> {
        for mode in [
            CoordinationMode::CoherentHousehold,
            CoordinationMode::Fragmentation,
        ] {
            assert_eq!(CoordinationMode::from_str_strict(mode.as_str())?, mode);
            assert_eq!(mode.to_string(), mode.as_str());
        }
        Ok(())
    }

    #[test]
    fn mode_rejects_unknown_string() {
        assert!(matches!(
            CoordinationMode::from_str_strict("Nope"),
            Err(CoreError::Orchestration(_))
        ));
    }

    #[test]
    fn default_mode_is_coherent() {
        assert_eq!(
            CoordinationMode::default(),
            CoordinationMode::CoherentHousehold
        );
    }

    #[test]
    fn device_assignment_serializes_camelcase() -> Result<()> {
        let a = DeviceAssignment {
            device_key: "abc".to_string(),
            is_self: false,
            persona_id: "p1".to_string(),
        };
        let json = serde_json::to_string(&a)?;
        assert!(json.contains("\"deviceKey\""));
        assert!(json.contains("\"isSelf\""));
        assert!(json.contains("\"personaId\""));
        let back: DeviceAssignment = serde_json::from_str(&json)?;
        assert_eq!(back, a);
        Ok(())
    }
}
