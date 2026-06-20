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

//! Desktop-LOCAL Persona Studio editor metadata (C5 #24, P1).
//!
//! [`PersonaSettings`] is editor-only state keyed by persona id. It is
//! DELIBERATELY NOT part of the synced `SyntheticPersona` wire model: per-field
//! locking and rotation tuning are a desktop authoring concern and must never
//! pollute the cross-device contract the phone reads. So this lives in its OWN
//! encrypted-store table (not in the persona JSON), and the persona round-trip to
//! Android stays byte-faithful.
//!
//! Two pieces of metadata per persona:
//!
//! - A set of LOCKED field names. A locked field is one the user has pinned by
//!   hand; the studio's regeneration and rotation logic must preserve a locked
//!   field's value rather than re-rolling it. The names are the persona's own
//!   field identifiers (see [`PersonaField`]).
//! - A [`RotationSchedule`]: either the frozen 8-to-10-day cadence (base interval
//!   plus asymmetric jitter, matching [`crate::constants::BASE_ROTATION_DAYS`] +
//!   [`crate::constants::ROTATION_JITTER_DAYS`]), or disabled to PIN the persona
//!   (never auto-rotate).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::constants::{BASE_ROTATION_DAYS, ROTATION_JITTER_DAYS};

/// The lockable fields of a persona, as stable identifier strings. Locking a
/// field pins its value across regeneration and rotation. These names are the
/// editor's own vocabulary (NOT the wire JSON keys); they are kept stable so
/// persisted [`PersonaSettings`] round-trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PersonaField {
    /// The display name.
    Name,
    /// The age range.
    AgeRange,
    /// The profession.
    Profession,
    /// The region.
    Region,
    /// The interest set.
    Interests,
    /// The desktop-only home location.
    HomeLocation,
    /// The desktop-only schedule label.
    Schedule,
    /// The desktop-only browsing-style label.
    BrowsingStyle,
}

impl PersonaField {
    /// Every lockable field, in declaration order.
    pub const ALL: &'static [PersonaField] = &[
        PersonaField::Name,
        PersonaField::AgeRange,
        PersonaField::Profession,
        PersonaField::Region,
        PersonaField::Interests,
        PersonaField::HomeLocation,
        PersonaField::Schedule,
        PersonaField::BrowsingStyle,
    ];

    /// The stable identifier string for this field (used in the locked-field
    /// set and persisted form).
    pub fn as_str(&self) -> &'static str {
        match self {
            PersonaField::Name => "name",
            PersonaField::AgeRange => "ageRange",
            PersonaField::Profession => "profession",
            PersonaField::Region => "region",
            PersonaField::Interests => "interests",
            PersonaField::HomeLocation => "homeLocation",
            PersonaField::Schedule => "schedule",
            PersonaField::BrowsingStyle => "browsingStyle",
        }
    }

    /// Parse a field identifier string, or `None` if unknown. Named to avoid
    /// confusion with the `std::str::FromStr::from_str` trait method (which
    /// returns a `Result`, not the `Option` this lenient lookup wants).
    pub fn from_field_name(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.as_str() == s)
    }
}

/// A persona's rotation schedule: the frozen cadence, or disabled (pinned).
///
/// The cadence mirrors the Android contract: a base interval of
/// [`BASE_ROTATION_DAYS`] days plus asymmetric jitter in
/// [`ROTATION_JITTER_DAYS`] (added, never subtracted), yielding the 8-to-10-day
/// window. Disabling it PINS the persona so it never auto-rotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[non_exhaustive]
pub enum RotationSchedule {
    /// Auto-rotate after `base_days + uniform(jitter_min_days..=jitter_max_days)`,
    /// matching the Android contract: jitter is added, never subtracted, with a
    /// minimum of 1, so the frozen window is 8 to 10 days (never as early as the
    /// bare base interval).
    Cadence {
        /// Base interval in days. Defaults to [`BASE_ROTATION_DAYS`].
        base_days: u32,
        /// Minimum jitter in days ADDED to the base (the bottom of
        /// [`ROTATION_JITTER_DAYS`], 1, in the frozen cadence). The rotation
        /// never fires earlier than `base_days + jitter_min_days`.
        jitter_min_days: u32,
        /// Maximum jitter in days ADDED to the base (the top of
        /// [`ROTATION_JITTER_DAYS`], 3, in the frozen cadence). The window is
        /// `base + jitter_min ..= base + jitter_max`.
        jitter_max_days: u32,
    },
    /// Never auto-rotate: the persona is pinned.
    Disabled,
}

impl RotationSchedule {
    /// The frozen default cadence (8-to-10 days): base 7 + jitter in 1..=3.
    pub fn frozen_cadence() -> Self {
        RotationSchedule::Cadence {
            base_days: BASE_ROTATION_DAYS,
            jitter_min_days: *ROTATION_JITTER_DAYS.start(),
            jitter_max_days: *ROTATION_JITTER_DAYS.end(),
        }
    }

    /// Build a custom cadence: rotate after `base_days` plus a uniform jitter in
    /// `jitter_min_days..=jitter_max_days`. Mirrors the Android contract (jitter
    /// is ADDED, never subtracted), so the bounds are clamped to keep
    /// `1 <= jitter_min <= jitter_max`: a persona never rotates as early as the
    /// bare base interval. This is the constructor the GUI's cadence presets and
    /// any custom interval/jitter editor use (C5 #24).
    pub fn cadence(base_days: u32, jitter_min_days: u32, jitter_max_days: u32) -> Self {
        let jitter_min_days = jitter_min_days.max(1);
        let jitter_max_days = jitter_max_days.max(jitter_min_days);
        RotationSchedule::Cadence {
            base_days,
            jitter_min_days,
            jitter_max_days,
        }
    }

    /// Whether this schedule auto-rotates (vs pins the persona).
    pub fn is_enabled(&self) -> bool {
        matches!(self, RotationSchedule::Cadence { .. })
    }

    /// The inclusive rotation window in days `(min, max)` for an enabled
    /// cadence, or `None` when rotation is disabled (pinned). The window is
    /// `base + jitter_min ..= base + jitter_max` (the frozen cadence yields
    /// `(8, 10)`, never the bare base interval).
    pub fn window_days(&self) -> Option<(u32, u32)> {
        match self {
            RotationSchedule::Cadence {
                base_days,
                jitter_min_days,
                jitter_max_days,
            } => Some((
                base_days.saturating_add(*jitter_min_days),
                base_days.saturating_add(*jitter_max_days),
            )),
            RotationSchedule::Disabled => None,
        }
    }
}

impl Default for RotationSchedule {
    fn default() -> Self {
        Self::frozen_cadence()
    }
}

/// Desktop-local editor metadata for one persona, keyed by persona id.
///
/// Holds the LOCKED field set and the [`RotationSchedule`]. Persisted in its own
/// `persona_settings` store table (NOT in the synced persona JSON), so the
/// cross-device wire contract is untouched. A persona with no stored settings
/// uses [`PersonaSettings::default_for`] (nothing locked, frozen cadence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaSettings {
    /// The persona id these settings belong to.
    pub persona_id: String,
    /// The set of locked field identifiers (see [`PersonaField::as_str`]).
    /// Stored as a sorted set so the persisted JSON is deterministic. Locked
    /// fields survive regeneration and rotation.
    #[serde(default)]
    pub locked_fields: BTreeSet<String>,
    /// The rotation schedule (frozen cadence, or disabled to pin the persona).
    #[serde(default)]
    pub rotation: RotationSchedule,
}

impl PersonaSettings {
    /// The default settings for a persona id: nothing locked, frozen cadence.
    pub fn default_for(persona_id: impl Into<String>) -> Self {
        Self {
            persona_id: persona_id.into(),
            locked_fields: BTreeSet::new(),
            rotation: RotationSchedule::default(),
        }
    }

    /// Whether `field` is currently locked.
    pub fn is_locked(&self, field: PersonaField) -> bool {
        self.locked_fields.contains(field.as_str())
    }

    /// Lock `field` (idempotent). Returns `true` if it was newly locked.
    pub fn lock(&mut self, field: PersonaField) -> bool {
        self.locked_fields.insert(field.as_str().to_string())
    }

    /// Unlock `field` (idempotent). Returns `true` if it was previously locked.
    pub fn unlock(&mut self, field: PersonaField) -> bool {
        self.locked_fields.remove(field.as_str())
    }

    /// Replace the rotation schedule.
    pub fn set_rotation(&mut self, rotation: RotationSchedule) {
        self.rotation = rotation;
    }

    /// The locked fields as parsed [`PersonaField`]s, skipping any unknown
    /// identifier (forward-compatible: a future field name persisted by a newer
    /// build is ignored rather than erroring).
    pub fn locked(&self) -> Vec<PersonaField> {
        self.locked_fields
            .iter()
            .filter_map(|s| PersonaField::from_field_name(s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_name_round_trip() {
        for f in PersonaField::ALL {
            assert_eq!(PersonaField::from_field_name(f.as_str()), Some(*f));
        }
        assert_eq!(PersonaField::from_field_name("nope"), None);
    }

    #[test]
    fn frozen_cadence_matches_the_8_to_10_day_window() -> crate::Result<()> {
        let window = RotationSchedule::frozen_cadence()
            .window_days()
            .ok_or_else(|| crate::CoreError::Key("cadence has no window".into()))?;
        // base 7 + jitter in 1..=3 => 8..=10. The phone's rotation is 8-to-10
        // days (jitter is added, never subtracted, with a minimum of 1), so the
        // window never includes the bare base interval (day 7).
        assert_eq!(window, (BASE_ROTATION_DAYS + 1, BASE_ROTATION_DAYS + 3));
        assert_eq!(window, (8, 10));
        Ok(())
    }

    #[test]
    fn disabled_rotation_pins_the_persona() {
        let pinned = RotationSchedule::Disabled;
        assert!(!pinned.is_enabled());
        assert_eq!(pinned.window_days(), None);
    }

    #[test]
    fn custom_cadence_sets_the_window_and_clamps_jitter() {
        // A monthly-ish cadence: 30 + jitter 3..=9 => 33..=39 days.
        assert_eq!(
            RotationSchedule::cadence(30, 3, 9).window_days(),
            Some((33, 39))
        );
        // jitter_min clamps up to 1 (never the bare base interval), and
        // jitter_max is floored to jitter_min when given inverted/zero bounds.
        assert_eq!(
            RotationSchedule::cadence(14, 0, 0).window_days(),
            Some((15, 15))
        );
        assert_eq!(
            RotationSchedule::cadence(14, 6, 2).window_days(),
            Some((20, 20))
        );
    }

    #[test]
    fn lock_unlock_is_idempotent() {
        let mut s = PersonaSettings::default_for("p1");
        assert!(!s.is_locked(PersonaField::AgeRange));
        assert!(s.lock(PersonaField::AgeRange));
        assert!(!s.lock(PersonaField::AgeRange)); // already locked
        assert!(s.is_locked(PersonaField::AgeRange));
        assert!(s.unlock(PersonaField::AgeRange));
        assert!(!s.unlock(PersonaField::AgeRange)); // already unlocked
        assert!(!s.is_locked(PersonaField::AgeRange));
    }

    #[test]
    fn settings_round_trip_through_json() -> crate::Result<()> {
        let mut s = PersonaSettings::default_for("p1");
        s.lock(PersonaField::Name);
        s.lock(PersonaField::Interests);
        s.set_rotation(RotationSchedule::Disabled);
        let json = serde_json::to_string(&s)?;
        let back: PersonaSettings = serde_json::from_str(&json)?;
        assert_eq!(back, s);
        assert!(back.is_locked(PersonaField::Name));
        assert!(back.is_locked(PersonaField::Interests));
        assert!(!back.rotation.is_enabled());
        Ok(())
    }
}
