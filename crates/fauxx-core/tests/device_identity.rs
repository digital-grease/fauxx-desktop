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

//! Cross-language contract test for the per-persona device-identity derivation
//! (#47 / fauxx#242).
//!
//! This is the Rust half of the cross-repo guarantee. It pins the SAME two anchors
//! the Android `DeviceDeriverTest` pins, so the two platforms cannot silently drift:
//!
//! 1. The vendored `device_templates.json` matches the pinned SHA-256 (asserted
//!    identically on both sides, over identical bytes).
//! 2. The committed `device_interop_vector.json` (mirror of `e13_interop_vector.json`,
//!    transcribed VERBATIM from the Kotlin golden maps) is reproduced BYTE-FOR-BYTE
//!    by [`fauxx_core::desktop_for`] / [`fauxx_core::mobile_for`] from the same
//!    `persona.id` + `createdAt`.
//!
//! The vector is loaded from the repo root at test time so a reviewer diffs it as a
//! plain fixture; the SHA-256 it declares is cross-checked against the live catalog.

use fauxx_core::persona::SyntheticPersona;
use fauxx_core::{desktop_for, device_templates_sha256, mobile_for, DeviceProfile};
use serde::Deserialize;

/// One golden case: a persona identity (only `id` + `createdAt` drive the
/// derivation) and the expected materialized devices.
#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    #[serde(rename = "createdAt")]
    created_at: i64,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    mobile: DeviceProfile,
    desktop: DeviceProfile,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    device_templates_sha256: String,
    cases: Vec<Case>,
}

/// Load the committed cross-language vector from the repo root.
fn load_vector() -> Result<Vector, Box<dyn std::error::Error>> {
    // `CARGO_MANIFEST_DIR` is `<repo>/crates/fauxx-core`; the vector lives at the
    // repo root beside `e13_interop_vector.json`.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../device_interop_vector.json"
    );
    let json = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&json)?)
}

/// Build a persona carrying only the two fields the derivation reads (`id`,
/// `created_at`); the rest are inert placeholders.
fn persona(id: &str, created_at: i64) -> SyntheticPersona {
    SyntheticPersona::new(
        id.to_string(),
        "golden".to_string(),
        "AGE_35_44".to_string(),
        "ENGINEER".to_string(),
        "US_MIDWEST".to_string(),
        vec!["TECHNOLOGY".to_string()],
        created_at,
        created_at + 7 * 24 * 60 * 60 * 1000,
    )
}

#[test]
fn vendored_catalog_matches_the_pinned_and_vectored_checksum(
) -> Result<(), Box<dyn std::error::Error>> {
    let vector = load_vector()?;
    let live = device_templates_sha256();
    // The live catalog matches the checksum the vector declares (guards the vector
    // against catalog drift)...
    assert_eq!(
        live, vector.device_templates_sha256,
        "device_interop_vector.json declares a stale device_templates.json checksum"
    );
    // ...and that value is the frozen cross-repo pin (identical on the Android side).
    assert_eq!(
        live, "3059247b5e83ea09b3ec69d8ed68577c4ceff27d3ca09f0842dd6db0b1e7a3dd",
        "device_templates.json changed — coordinate the checksum bump across both repos"
    );
    Ok(())
}

#[test]
fn golden_vector_is_reproduced_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let vector = load_vector()?;
    assert!(
        !vector.cases.is_empty(),
        "the golden vector must carry cases"
    );
    for case in &vector.cases {
        let p = persona(&case.id, case.created_at);
        let key = format!("{}@{}", case.id, case.created_at);

        // The DESKTOP identity is the one this companion actually presents; it is the
        // primary contract for #47.
        assert_eq!(
            desktop_for(&p),
            case.expected.desktop,
            "desktop golden mismatch for {key}"
        );

        // MOBILE parity: the desktop derives the phone's identity identically (it is
        // never emitted here, but proving parity is what lets both platforms derive
        // independently without syncing the device over the wire).
        assert_eq!(
            mobile_for(&p),
            case.expected.mobile,
            "mobile golden mismatch for {key}"
        );
    }
    Ok(())
}

#[test]
fn golden_desktop_devices_are_coherent_and_headless_free() -> Result<(), Box<dyn std::error::Error>>
{
    // Independent of the exact template, every derived desktop device must be a
    // coherent, non-mobile, non-headless identity.
    let vector = load_vector()?;
    for case in &vector.cases {
        let d = desktop_for(&persona(&case.id, case.created_at));
        assert!(!d.is_mobile);
        assert!(!d.user_agent.contains("HeadlessChrome"));
        assert!(!d.user_agent.contains("Mobile"));
        assert!(!d.user_agent.contains("Android"));
        assert!(d.model.is_empty(), "desktop reports no device model");
        assert_eq!(d.architecture(), "x86");
        assert_eq!(d.bitness(), "64");
        // The client-hint full-version list is coherent with the UA's major.
        assert!(d
            .full_version_brands()
            .iter()
            .all(|b| b.version.ends_with(".0.0.0")));
    }
    Ok(())
}
