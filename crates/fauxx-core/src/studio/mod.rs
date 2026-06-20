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

//! The Persona Studio core (C5: #24 P1 editor model, #25 P2 coherence linter,
//! #26 P3 week simulator).
//!
//! This is the HEADLESS core of the studio: the editable persona model metadata
//! (`settings`), the coherence linter (`linter`), and the deterministic week
//! simulator (`simulator`). The Iced GUI views are a SEPARATE later batch; no
//! GUI/CLI type appears here. Everything is 100% local with no network or
//! telemetry.
//!
//! ## The change-event mechanism (#24, P1)
//!
//! Dependent views (the P2 linter panel, the P3 week preview) recompute when a
//! persona changes. The core exposes a NON-GUI change-event stream:
//! [`Core::subscribe_persona_changes`](crate::Core::subscribe_persona_changes)
//! returns a [`tokio::sync::broadcast::Receiver`] of [`PersonaChanged`] events.
//! Saving a persona, or saving/changing its [`PersonaSettings`], emits an event;
//! a subscriber reacts by reloading the persona and re-running
//! [`Core::lint_persona`](crate::Core::lint_persona) /
//! [`Core::simulate_week`](crate::Core::simulate_week). The GUI subscription is a
//! later batch; the broadcast and the recompute helpers are wired here so that
//! batch is a thin client.

mod linter;
mod settings;
mod simulator;

pub use linter::{lint_persona, Finding, Severity};
pub use settings::{PersonaField, PersonaSettings, RotationSchedule};
pub use simulator::{
    simulate_week, QueryWeighting, SimulatedQuery, SimulatedSession, SimulatedWeek, DAYS_PER_WEEK,
};

use serde::{Deserialize, Serialize};

/// The capacity of the persona-change broadcast channel. A slow subscriber that
/// lags more than this many events sees a `RecvError::Lagged` and should simply
/// reload current state (the events are recompute triggers, not a durable log),
/// so a generous buffer is sufficient.
pub(crate) const PERSONA_CHANGE_CHANNEL_CAPACITY: usize = 64;

/// What changed about a persona, carried on the [`PersonaChanged`] event so a
/// subscriber can scope its recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum PersonaChangeKind {
    /// The persona record itself was saved (created or edited).
    Saved,
    /// The persona was deleted.
    Deleted,
    /// The persona's desktop-local [`PersonaSettings`] (locks/rotation) changed.
    SettingsChanged,
}

/// A persona-changed event, broadcast so dependent views recompute. Carries the
/// affected persona id and what changed. Deliberately small (no persona payload):
/// a subscriber reloads current state through the `Core` API, which keeps the
/// event a pure trigger and avoids stale snapshots racing the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaChanged {
    /// The id of the persona that changed.
    pub persona_id: String,
    /// What kind of change occurred.
    pub kind: PersonaChangeKind,
}

impl PersonaChanged {
    /// A `Saved` event for `persona_id`.
    pub fn saved(persona_id: impl Into<String>) -> Self {
        Self {
            persona_id: persona_id.into(),
            kind: PersonaChangeKind::Saved,
        }
    }

    /// A `Deleted` event for `persona_id`.
    pub fn deleted(persona_id: impl Into<String>) -> Self {
        Self {
            persona_id: persona_id.into(),
            kind: PersonaChangeKind::Deleted,
        }
    }

    /// A `SettingsChanged` event for `persona_id`.
    pub fn settings_changed(persona_id: impl Into<String>) -> Self {
        Self {
            persona_id: persona_id.into(),
            kind: PersonaChangeKind::SettingsChanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_event_serializes_camelcase() -> crate::Result<()> {
        let ev = PersonaChanged::saved("p1");
        let json = serde_json::to_string(&ev)?;
        assert!(json.contains("\"personaId\""));
        assert!(json.contains("\"kind\""));
        let back: PersonaChanged = serde_json::from_str(&json)?;
        assert_eq!(back, ev);
        Ok(())
    }

    #[test]
    fn change_event_constructors_set_kind() {
        assert_eq!(PersonaChanged::saved("p").kind, PersonaChangeKind::Saved);
        assert_eq!(
            PersonaChanged::deleted("p").kind,
            PersonaChangeKind::Deleted
        );
        assert_eq!(
            PersonaChanged::settings_changed("p").kind,
            PersonaChangeKind::SettingsChanged
        );
    }
}
