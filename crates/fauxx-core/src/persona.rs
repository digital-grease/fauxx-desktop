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

//! The persona data model.
//!
//! [`SyntheticPersona`] is the frozen cross-device contract: it serializes,
//! field-for-field, to the same JSON the Android app persists via Gson. The
//! wire fields are deliberately `String`/`Vec<String>` (not Rust enums) so the
//! round-trip is lossless and tolerant of legacy or future enum values the
//! phone may emit. The validated enum types ([`CategoryPool`], [`AgeRange`],
//! [`Profession`], [`Region`]) exist for the desktop studio/linter to reason
//! about personas *without* changing what goes on the wire.
//!
//! Desktop-only additive fields are gated behind
//! `#[serde(default, skip_serializing_if = "Option::is_none")]` plus a
//! defaulted [`SyntheticPersona::schema_version`], so the phone's lenient Gson
//! reader simply ignores anything it does not recognize.

use serde::{Deserialize, Serialize};

/// The current desktop schema version stamped onto newly built personas.
///
/// Bumped only when the desktop adds additive fields; the phone ignores it.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Number of interests a well-formed persona carries, inclusive.
pub const INTEREST_COUNT: std::ops::RangeInclusive<usize> = 3..=5;

/// A synthetic browsing persona.
///
/// Serializes to the exact Android Gson schema (camelCase keys). All wire
/// fields are strings to guarantee a lossless cross-device round-trip; use
/// [`CategoryPool`], [`AgeRange`], [`Profession`], and [`Region`] to validate
/// or interpret the string values without mutating the persisted form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntheticPersona {
    /// UUID v4 string identifying this persona.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// [`AgeRange`] enum *name* (e.g. `"AGE_35_44"`), not a display label.
    pub age_range: String,
    /// [`Profession`] enum *name* (e.g. `"FINANCE_PROF"`).
    pub profession: String,
    /// [`Region`] enum *name* (e.g. `"US_MIDWEST"`).
    pub region: String,
    /// [`CategoryPool`] enum *names*; a well-formed persona carries 3..=5.
    pub interests: Vec<String>,
    /// Creation time, epoch milliseconds. JSON key `createdAt`.
    pub created_at: i64,
    /// Expiry/rotation time, epoch milliseconds. JSON key `activeUntil`.
    pub active_until: i64,

    // --- Desktop-only additive fields (the phone ignores these) ---
    /// Desktop schema version. Defaults to `0` for records written by clients
    /// that predate it (notably the phone), and to [`CURRENT_SCHEMA_VERSION`]
    /// for personas built here via [`SyntheticPersona::new`].
    #[serde(default)]
    pub schema_version: u32,
    /// Optional desktop-only freeform note. Never emitted when `None`, so the
    /// phone never sees the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Optional desktop-only home location (a freeform place label the Persona
    /// Studio editor lets the user pin, distinct from the coarse synced
    /// [`region`](Self::region)). Additive and optional: never emitted when
    /// `None`, so the Android Gson reader never sees the key and the cross-device
    /// round-trip stays lossless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_location: Option<String>,
    /// Optional desktop-only daily-rhythm schedule label (e.g. `"early_bird"`),
    /// editor metadata the week simulator can read as a hint. Additive and
    /// optional; omitted from JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// Optional desktop-only browsing-style label (e.g. `"skimmer"` vs
    /// `"deep_reader"`), editor metadata. Additive and optional; omitted from
    /// JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browsing_style: Option<String>,
}

impl SyntheticPersona {
    /// Build a persona from already-validated string field values, stamping the
    /// current desktop [`schema_version`](Self::schema_version). The caller is
    /// responsible for supplying valid enum names and a UUID; this constructor
    /// does no validation so it can faithfully reconstruct legacy records.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        age_range: String,
        profession: String,
        region: String,
        interests: Vec<String>,
        created_at: i64,
        active_until: i64,
    ) -> Self {
        Self {
            id,
            name,
            age_range,
            profession,
            region,
            interests,
            created_at,
            active_until,
            schema_version: CURRENT_SCHEMA_VERSION,
            note: None,
            home_location: None,
            schedule: None,
            browsing_style: None,
        }
    }

    /// Validate the wire fields against the known enum names and interest-count
    /// rule. Returns the list of problems found (empty if valid). This is the
    /// studio/linter entry point; it never mutates the persona or the wire form.
    pub fn validate(&self) -> Vec<PersonaIssue> {
        let mut issues = Vec::new();
        if AgeRange::from_name(&self.age_range).is_none() {
            issues.push(PersonaIssue::UnknownAgeRange(self.age_range.clone()));
        }
        if Profession::from_name(&self.profession).is_none() {
            issues.push(PersonaIssue::UnknownProfession(self.profession.clone()));
        }
        if Region::from_name(&self.region).is_none() {
            issues.push(PersonaIssue::UnknownRegion(self.region.clone()));
        }
        if !INTEREST_COUNT.contains(&self.interests.len()) {
            issues.push(PersonaIssue::InterestCount(self.interests.len()));
        }
        for interest in &self.interests {
            if CategoryPool::from_name(interest).is_none() {
                issues.push(PersonaIssue::UnknownInterest(interest.clone()));
            }
        }
        issues
    }
}

/// A single problem found by [`SyntheticPersona::validate`]. Surfaced to the
/// studio/linter; it does not affect persistence (unknown values still round-
/// trip losslessly).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PersonaIssue {
    /// The `ageRange` string is not a known [`AgeRange`] name.
    UnknownAgeRange(String),
    /// The `profession` string is not a known [`Profession`] name.
    UnknownProfession(String),
    /// The `region` string is not a known [`Region`] name.
    UnknownRegion(String),
    /// An `interests` entry is not a known [`CategoryPool`] name.
    UnknownInterest(String),
    /// The number of interests is outside the 3..=5 window. Carries the count.
    InterestCount(usize),
}

/// Generates a Rust enum mirroring an Android enum, with lossless
/// name<->variant mapping via `as_name` / `from_name` and an `all()` slice.
macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        // The variant names are the frozen Android enum NAMES (e.g. `AGE_35_44`,
        // `US_MIDWEST`); they must match the cross-device contract verbatim, so
        // Rust's camel-case convention is deliberately waived here.
        #[allow(non_camel_case_types)]
        #[non_exhaustive]
        pub enum $name {
            $(
                #[allow(missing_docs)]
                $variant,
            )+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The Android enum *name* for this variant (e.g. `"AGE_35_44"`).
            pub fn as_name(&self) -> &'static str {
                match self {
                    $($name::$variant => stringify!($variant),)+
                }
            }

            /// Parse an Android enum name into a variant, or `None` if unknown.
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $(n if n == stringify!($variant) => Some($name::$variant),)+
                    _ => None,
                }
            }

            /// All variant names, in declaration order.
            pub fn all() -> &'static [$name] {
                Self::ALL
            }
        }
    };
}

named_enum! {
    /// Interest categories. The 32 frozen `CategoryPool` names from Android.
    CategoryPool {
        MEDICAL, LEGAL, AUTOMOTIVE, PARENTING, RETIREMENT, GAMING, AGRICULTURE,
        FASHION, ACADEMIC, REAL_ESTATE, COOKING, SPORTS, FINANCE, TRAVEL,
        TECHNOLOGY, PETS, HOME_IMPROVEMENT, BEAUTY, MUSIC, FITNESS, ENTERTAINMENT,
        FOOD, POLITICS, SCIENCE, BUSINESS, OUTDOOR_RECREATION, CRAFTS, HISTORY,
        ENVIRONMENT, MILITARY_DEFENSE, WELLNESS_ALTERNATIVE, RELATIONSHIPS_DATING,
    }
}

named_enum! {
    /// Age brackets. The 6 frozen `AgeRange` names from Android.
    AgeRange {
        AGE_18_24, AGE_25_34, AGE_35_44, AGE_45_54, AGE_55_64, AGE_65_PLUS,
    }
}

named_enum! {
    /// Professions. The 12 frozen `Profession` names from Android.
    Profession {
        STUDENT, TEACHER, ENGINEER, HEALTHCARE, LEGAL, FINANCE_PROF, RETAIL,
        TRADES, CREATIVE, RETIRED, HOMEMAKER, OTHER,
    }
}

named_enum! {
    /// Regions. The 23 frozen `Region` names from Android.
    Region {
        US_NORTHEAST, US_SOUTHEAST, US_MIDWEST, US_SOUTHWEST, US_WEST, CANADA,
        UK, WESTERN_EUROPE, EASTERN_EUROPE, ASIA_PACIFIC, LATIN_AMERICA,
        MIDDLE_EAST_AFRICA, SPAIN, MEXICO, ARGENTINA, COLOMBIA, CHILE, PERU,
        FRANCE, QUEBEC, BELGIUM, SWITZERLAND, OTHER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SyntheticPersona {
        SyntheticPersona::new(
            "11111111-1111-4111-8111-111111111111".to_string(),
            "Test Persona".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::FINANCE_PROF.as_name().to_string(),
            Region::US_MIDWEST.as_name().to_string(),
            vec![
                CategoryPool::FINANCE.as_name().to_string(),
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::TRAVEL.as_name().to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    #[test]
    fn json_uses_android_camelcase_keys() -> crate::Result<()> {
        let json = serde_json::to_string(&sample())?;
        for key in [
            "\"id\"",
            "\"name\"",
            "\"ageRange\"",
            "\"profession\"",
            "\"region\"",
            "\"interests\"",
            "\"createdAt\"",
            "\"activeUntil\"",
        ] {
            assert!(json.contains(key), "expected key {key} in {json}");
        }
        // The enum names ride through verbatim.
        assert!(json.contains("\"AGE_35_44\""));
        assert!(json.contains("\"FINANCE_PROF\""));
        assert!(json.contains("\"US_MIDWEST\""));
        Ok(())
    }

    #[test]
    fn round_trips_equal() -> crate::Result<()> {
        let original = sample();
        let json = serde_json::to_string(&original)?;
        let back: SyntheticPersona = serde_json::from_str(&json)?;
        assert_eq!(original, back);
        Ok(())
    }

    #[test]
    fn optional_note_omitted_when_none() -> crate::Result<()> {
        let json = serde_json::to_string(&sample())?;
        assert!(!json.contains("\"note\""));
        Ok(())
    }

    #[test]
    fn android_eight_keys_serialize_exactly_and_no_extras_leak() -> crate::Result<()> {
        // The wire model must serialize the EXACT eight Android keys and NOTHING
        // else when the additive desktop fields are unset (the phone's lenient
        // reader ignores extras, but the contract is that None desktop fields
        // never even appear on the wire). schema_version always rides along (it
        // defaults to 0 on the phone, which the phone ignores).
        let value: serde_json::Value = serde_json::to_value(sample())?;
        let obj = value.as_object().ok_or_else(|| {
            crate::CoreError::Key("persona did not serialize to an object".into())
        })?;
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "activeUntil",
                "ageRange",
                "createdAt",
                "id",
                "interests",
                "name",
                "profession",
                "region",
                "schemaVersion",
            ],
            "only the 8 Android keys (plus defaulted schemaVersion) may appear when the \
             additive desktop fields are None"
        );
        Ok(())
    }

    #[test]
    fn new_desktop_fields_omitted_when_none() -> crate::Result<()> {
        let json = serde_json::to_string(&sample())?;
        assert!(!json.contains("\"homeLocation\""));
        assert!(!json.contains("\"schedule\""));
        assert!(!json.contains("\"browsingStyle\""));
        Ok(())
    }

    #[test]
    fn new_desktop_fields_round_trip_when_set() -> crate::Result<()> {
        let mut p = sample();
        p.home_location = Some("Portland, OR".to_string());
        p.schedule = Some("early_bird".to_string());
        p.browsing_style = Some("deep_reader".to_string());
        let json = serde_json::to_string(&p)?;
        assert!(json.contains("\"homeLocation\""));
        assert!(json.contains("\"schedule\""));
        assert!(json.contains("\"browsingStyle\""));
        let back: SyntheticPersona = serde_json::from_str(&json)?;
        assert_eq!(back, p);
        Ok(())
    }

    #[test]
    fn old_phone_json_without_new_fields_still_deserializes() -> crate::Result<()> {
        // Exactly the eight frozen Android camelCase keys, as an old phone (with
        // no knowledge of the desktop additive fields) would emit. It must
        // deserialize, with the additive desktop fields defaulting to None and
        // schema_version to 0.
        let android_json = r#"{
            "id": "55555555-5555-4555-8555-555555555555",
            "name": "Phone Persona",
            "ageRange": "AGE_25_34",
            "profession": "ENGINEER",
            "region": "US_WEST",
            "interests": ["TECHNOLOGY", "GAMING", "SCIENCE"],
            "createdAt": 1700000000000,
            "activeUntil": 1700600000000
        }"#;
        let p: SyntheticPersona = serde_json::from_str(android_json)?;
        assert_eq!(p.schema_version, 0);
        assert_eq!(p.note, None);
        assert_eq!(p.home_location, None);
        assert_eq!(p.schedule, None);
        assert_eq!(p.browsing_style, None);
        // And it serializes back to exactly the 8 keys plus the defaulted version
        // (no desktop fields leak), so the round-trip stays lossless.
        let back = serde_json::to_string(&p)?;
        assert!(!back.contains("\"homeLocation\""));
        assert!(!back.contains("\"schedule\""));
        assert!(!back.contains("\"browsingStyle\""));
        assert!(!back.contains("\"note\""));
        Ok(())
    }

    #[test]
    fn desktop_extra_fields_are_ignored_by_a_lenient_phone_reader() -> crate::Result<()> {
        // The reverse direction: a desktop persona with all additive fields set
        // must still be readable by the phone, whose Gson reader is LENIENT
        // (drops unknown keys). Modeled by a plain 8-key struct WITHOUT
        // deny_unknown_fields.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct PhonePersona {
            id: String,
            name: String,
            age_range: String,
            profession: String,
            region: String,
            interests: Vec<String>,
            created_at: i64,
            active_until: i64,
        }
        // A STRICT reader of just the 8 Android keys, to prove the desktop-only
        // keys are genuinely extra (it must reject them).
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[allow(dead_code)]
        struct StrictPhonePersona {
            id: String,
            name: String,
            age_range: String,
            profession: String,
            region: String,
            interests: Vec<String>,
            created_at: i64,
            active_until: i64,
        }

        let mut desktop = sample();
        desktop.home_location = Some("Portland, OR".to_string());
        desktop.schedule = Some("early_bird".to_string());
        desktop.browsing_style = Some("deep_reader".to_string());
        let json = serde_json::to_string(&desktop)?;

        // The lenient phone reader deserializes fine, dropping the desktop-only
        // keys, and the 8 core values survive intact.
        let phone: PhonePersona = serde_json::from_str(&json)?;
        assert_eq!(phone.id, desktop.id);
        assert_eq!(phone.name, desktop.name);
        assert_eq!(phone.age_range, desktop.age_range);
        assert_eq!(phone.profession, desktop.profession);
        assert_eq!(phone.region, desktop.region);
        assert_eq!(phone.interests, desktop.interests);
        assert_eq!(phone.created_at, desktop.created_at);
        assert_eq!(phone.active_until, desktop.active_until);

        // The strict reader REJECTS the JSON, proving homeLocation / schedule /
        // browsingStyle (and schemaVersion) are extra keys beyond the frozen 8.
        let strict: std::result::Result<StrictPhonePersona, _> = serde_json::from_str(&json);
        assert!(
            strict.is_err(),
            "desktop-only keys must be extra keys beyond the 8 Android keys"
        );
        Ok(())
    }

    #[test]
    fn tolerates_extra_and_unknown_fields_from_phone() -> crate::Result<()> {
        // The phone may emit fields we do not know, and legacy enum values.
        let phone_json = r#"{
            "id": "22222222-2222-4222-8222-222222222222",
            "name": "Legacy",
            "ageRange": "AGE_99_PLUS",
            "profession": "ASTRONAUT",
            "region": "MARS",
            "interests": ["SPACE", "FINANCE"],
            "createdAt": 1700000000000,
            "activeUntil": 1700600000000,
            "futurePhoneOnlyField": true
        }"#;
        let p: SyntheticPersona = serde_json::from_str(phone_json)?;
        // Unknown values round-trip losslessly...
        assert_eq!(p.age_range, "AGE_99_PLUS");
        assert_eq!(p.profession, "ASTRONAUT");
        assert_eq!(p.schema_version, 0); // defaulted: record predates the field
                                         // ...but the validator flags them.
        let issues = p.validate();
        assert!(issues.contains(&PersonaIssue::UnknownAgeRange("AGE_99_PLUS".into())));
        assert!(issues.contains(&PersonaIssue::UnknownProfession("ASTRONAUT".into())));
        assert!(issues.contains(&PersonaIssue::UnknownRegion("MARS".into())));
        assert!(issues.contains(&PersonaIssue::UnknownInterest("SPACE".into())));
        Ok(())
    }

    #[test]
    fn enum_counts_are_frozen() {
        assert_eq!(CategoryPool::all().len(), 32);
        assert_eq!(AgeRange::all().len(), 6);
        assert_eq!(Profession::all().len(), 12);
        assert_eq!(Region::all().len(), 23);
    }

    #[test]
    fn enum_name_round_trip() {
        for c in CategoryPool::all() {
            assert_eq!(CategoryPool::from_name(c.as_name()), Some(*c));
        }
        for a in AgeRange::all() {
            assert_eq!(AgeRange::from_name(a.as_name()), Some(*a));
        }
        for p in Profession::all() {
            assert_eq!(Profession::from_name(p.as_name()), Some(*p));
        }
        for r in Region::all() {
            assert_eq!(Region::from_name(r.as_name()), Some(*r));
        }
        assert_eq!(CategoryPool::from_name("NOPE"), None);
    }

    #[test]
    fn valid_persona_has_no_issues() {
        assert!(sample().validate().is_empty());
    }
}
