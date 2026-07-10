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

//! Per-persona device-identity derivation (#47): the Rust port of the Android
//! `DeviceDeriver` (fauxx#242), reproducing its output BYTE-FOR-BYTE.
//!
//! A persona owns exactly two coherent device identities, derived deterministically
//! from the persona alone: one [`FormFactor::Mobile`] (Android-Chromium, emitted by
//! the phone over its System WebView TLS) and one [`FormFactor::Desktop`]
//! (desktop-Chrome, emitted by THIS companion over its real desktop Chromium TLS).
//! The desktop companion drives a real desktop TLS stack, so it must present a
//! DESKTOP user-agent; a mobile UA there would recreate the #168 TLS/UA-mismatch
//! tell. This module only ever emits the DESKTOP identity onto the decoy browser.
//!
//! ## Why derivation, not wire-sync
//!
//! The identity is a pure function of [`SyntheticPersona::id`] (which template) and
//! [`SyntheticPersona::created_at`] (which Chrome major). Both platforms compute the
//! same bytes from the same synced persona, so nothing new crosses the LAN wire. The
//! guarantee that the two platforms cannot silently drift has two anchors, both
//! asserted in tests:
//!
//! 1. The catalog [`DEVICE_TEMPLATES_JSON`] is vendored VERBATIM from the Android
//!    app and its SHA-256 is pinned (see the `tests/device_identity.rs` checksum
//!    assertion and the matching Kotlin `DeviceDeriverTest`).
//! 2. A committed cross-language golden vector (`device_interop_vector.json`, mirror
//!    of `e13_interop_vector.json`) pins the exact derived output for a handful of
//!    `(id, createdAt)` cases spanning template and Chrome-version boundaries.
//!
//! ## Determinism primitive
//!
//! `pick` is a counter-based hash selection needing only a shared hash (SHA-256)
//! and a canonical byte layout — no PRNG, no floats — so it is trivially identical
//! across Kotlin and Rust. See `.devloop/spikes/c2-device-identity-derivation.md`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persona::SyntheticPersona;

/// The bundled device catalog, vendored VERBATIM from the Android app's
/// `app/src/main/assets/device_templates.json`. Embedded at compile time so it
/// ships in the binary with no runtime file dependency, exactly like the other
/// bundled corpora (`brokers.json`, `category_sites.json`). Its
/// SHA-256 is pinned and asserted in `tests/device_identity.rs`; the same value is
/// asserted on the Android side, so the two repos cannot diverge unnoticed.
pub const DEVICE_TEMPLATES_JSON: &str = include_str!("device_templates.json");

/// The token substituted with the derived Chrome major in `uaTemplate` and each
/// brand `version`. Frozen; matches the Android `DeviceDeriver.MAJOR_TOKEN`.
const MAJOR_TOKEN: &str = "{MAJOR}";

/// The `pick` domain for the MOBILE template selection. Frozen cross-repo.
const DOMAIN_MOBILE: &str = "device:mobile";
/// The `pick` domain for the DESKTOP template selection. Frozen cross-repo.
const DOMAIN_DESKTOP: &str = "device:desktop";

/// Separator byte between the `pick` inputs (`id | domain | index`). ASCII `|`.
const SEPARATOR: u8 = 0x7C;

/// Chrome-version baseline. Frozen by the cross-repo golden vector: this is a
/// COORDINATED change with the Android app, not one this repo bumps unilaterally.
/// [`chrome_major`] advances one major per [`RELEASE_INTERVAL_MS`] after
/// [`BASELINE_EPOCH_MS`], and floors here so a persona minted before the baseline
/// never claims a version below it.
pub const BASELINE_MAJOR: u32 = 142;
/// The baseline epoch (2026-01-13T00:00:00Z), epoch milliseconds. Frozen cross-repo.
pub const BASELINE_EPOCH_MS: i64 = 1_768_262_400_000;
/// Chrome ships a new stable major roughly every 4 weeks. Frozen cross-repo.
pub const RELEASE_INTERVAL_MS: i64 = 28 * 24 * 60 * 60 * 1000;

/// Device form factor. A persona owns one [`Mobile`](FormFactor::Mobile) identity
/// (the phone's WebView) and one [`Desktop`](FormFactor::Desktop) identity (this
/// companion). Splitting by form factor is what keeps each device's UA coherent
/// with the TLS stack it is actually presented over (#168).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FormFactor {
    /// Android-Chromium; emitted by the phone. Never emitted from the desktop.
    Mobile,
    /// Desktop-Chrome; emitted by this companion over its real desktop TLS.
    Desktop,
}

impl FormFactor {
    /// The `pick` domain string this form factor selects its template from.
    fn domain(self) -> &'static str {
        match self {
            FormFactor::Mobile => DOMAIN_MOBILE,
            FormFactor::Desktop => DOMAIN_DESKTOP,
        }
    }
}

/// A `Sec-CH-UA` / `navigator.userAgentData` brand entry. `version` carries the
/// SIGNIFICANT (major-only) version, matching what `Sec-CH-UA` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brand {
    /// Brand name, e.g. `"Chromium"`, `"Google Chrome"`, `"Not?A_Brand"`.
    pub name: String,
    /// Brand version. For real browser brands this is the Chrome major; the
    /// GREASE `"Not?A_Brand"` carries its own constant (`"24"`).
    pub version: String,
}

/// A coherent device template parsed from `device_templates.json`, with the
/// `{MAJOR}` token still unresolved. Deserialized from the vendored catalog; the
/// camelCase keys mirror the Android JSON schema exactly.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceTemplate {
    model: String,
    platform: String,
    platform_version: String,
    ua_template: String,
    is_mobile: bool,
    brands: Vec<Brand>,
    screen_width: i64,
    screen_height: i64,
    device_pixel_ratio: f64,
    hardware_concurrency: u32,
    device_memory: u32,
}

impl DeviceTemplate {
    /// Materialize this template into a concrete [`DeviceProfile`] by substituting
    /// `chrome_major` for every `{MAJOR}` token in the UA and the brand versions.
    fn materialize(&self, form_factor: FormFactor, chrome_major: u32) -> DeviceProfile {
        let major = chrome_major.to_string();
        DeviceProfile {
            form_factor,
            user_agent: self.ua_template.replace(MAJOR_TOKEN, &major),
            platform: self.platform.clone(),
            platform_version: self.platform_version.clone(),
            model: self.model.clone(),
            is_mobile: self.is_mobile,
            brands: self
                .brands
                .iter()
                .map(|b| Brand {
                    name: b.name.clone(),
                    version: b.version.replace(MAJOR_TOKEN, &major),
                })
                .collect(),
            screen_width: self.screen_width,
            screen_height: self.screen_height,
            device_pixel_ratio: self.device_pixel_ratio,
            hardware_concurrency: self.hardware_concurrency,
            device_memory: self.device_memory,
        }
    }
}

/// A coherent, stable device identity derived for a persona (#47). This is a
/// bundle, not just a UA string: the UA, client-hint metadata, screen, and fixed
/// navigator values are mutually consistent and appropriate to [`model`](Self::model),
/// so a site cannot catch a contradiction between them.
///
/// Serializes with camelCase keys matching the Android `DeviceProfile`, so the
/// committed cross-language golden vector reads identically in both repos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProfile {
    /// Which form factor this identity is for.
    pub form_factor: FormFactor,
    /// Fully materialized User-Agent (the Chrome major already substituted). Never
    /// carries a `HeadlessChrome` token; the decoy applies this over the real UA.
    pub user_agent: String,
    /// `navigator.userAgentData.platform`: `"Android" | "Windows" | "macOS" | "Linux"`.
    pub platform: String,
    /// `Sec-CH-UA-Platform-Version`, e.g. `"14.5.0"` (macOS), `"15.0.0"` (Windows).
    pub platform_version: String,
    /// Device model (`"Pixel 8"` for mobile); empty for desktop, which reports none.
    pub model: String,
    /// `navigator.userAgentData.mobile`. Always `false` for a desktop identity.
    pub is_mobile: bool,
    /// `Sec-CH-UA` / `navigator.userAgentData.brands` (significant/major versions).
    pub brands: Vec<Brand>,
    /// Screen width in CSS pixels.
    pub screen_width: i64,
    /// Screen height in CSS pixels.
    pub screen_height: i64,
    /// `window.devicePixelRatio` (the CDP `deviceScaleFactor`).
    pub device_pixel_ratio: f64,
    /// Fixed, device-appropriate `navigator.hardwareConcurrency` (a real device
    /// never varies it read-to-read).
    pub hardware_concurrency: u32,
    /// Fixed, device-appropriate `navigator.deviceMemory` in GB.
    pub device_memory: u32,
}

impl DeviceProfile {
    /// The `Sec-CH-UA-Arch` value for this device's client hints: `"arm"` for a
    /// mobile/Apple-silicon UA, else `"x86"`. All current desktop templates are
    /// Intel/x86_64, so they report `"x86"`; the check reads the UA so an ARM
    /// template added later reports correctly without extra wiring.
    pub fn architecture(&self) -> &'static str {
        let ua = self.user_agent.to_ascii_lowercase();
        if self.is_mobile || ua.contains("arm") || ua.contains("aarch64") {
            "arm"
        } else {
            "x86"
        }
    }

    /// The `Sec-CH-UA-Bitness` value: all templates are 64-bit builds.
    pub fn bitness(&self) -> &'static str {
        "64"
    }

    /// The legacy `navigator.platform` value coherent with this device. This is
    /// DISTINCT from [`platform`](Self::platform) (which is
    /// `navigator.userAgentData.platform`, e.g. `"macOS"`): real Chrome reports the
    /// old fixed token here (`"MacIntel"`, `"Win32"`, `"Linux x86_64"`), so a
    /// fingerprinter comparing `navigator.platform` against the UA sees no
    /// contradiction. `None` for a platform outside the known desktop/mobile set, in
    /// which case the decoy leaves `navigator.platform` untouched rather than guess.
    pub fn navigator_platform(&self) -> Option<&'static str> {
        match self.platform.as_str() {
            "macOS" => Some("MacIntel"),
            "Windows" => Some("Win32"),
            "Linux" => Some("Linux x86_64"),
            "Android" => Some("Linux armv8l"),
            _ => None,
        }
    }

    /// The `Sec-CH-UA-Full-Version-List` entries: each brand expanded to a full
    /// `<version>.0.0.0` string, coherent with the `Chrome/<major>.0.0.0` token in
    /// the UA. Real Chrome reports the four-part full version in the full-version
    /// list while `Sec-CH-UA` keeps the significant (major) version.
    pub fn full_version_brands(&self) -> Vec<Brand> {
        self.brands
            .iter()
            .map(|b| Brand {
                name: b.name.clone(),
                version: format!("{}.0.0.0", b.version),
            })
            .collect()
    }
}

/// The bundled device catalog: Android-Chromium `mobile` templates + desktop-Chrome
/// `desktop` templates, parsed from [`DEVICE_TEMPLATES_JSON`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct DeviceCatalog {
    mobile: Vec<DeviceTemplate>,
    desktop: Vec<DeviceTemplate>,
}

impl DeviceCatalog {
    /// Parse a catalog from JSON (the vendored asset, or a test fixture).
    fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The template list + `pick` domain for a form factor.
    fn options(&self, form_factor: FormFactor) -> &[DeviceTemplate] {
        match form_factor {
            FormFactor::Mobile => &self.mobile,
            FormFactor::Desktop => &self.desktop,
        }
    }

    /// The template a persona resolves to for `form_factor`, selected by hashing the
    /// persona id into the form factor's option list.
    fn template_for(&self, persona: &SyntheticPersona, form_factor: FormFactor) -> &DeviceTemplate {
        let options = self.options(form_factor);
        // The catalog is non-empty by construction (asserted in tests); the fallback
        // catalog is likewise non-empty, so `pick`'s size is always > 0.
        let index = pick(&persona.id, form_factor.domain(), 0, options.len());
        &options[index]
    }

    /// Derive the persona's device identity for `form_factor`, materialized at the
    /// persona's creation-time Chrome major.
    fn device_for(&self, persona: &SyntheticPersona, form_factor: FormFactor) -> DeviceProfile {
        let major = chrome_major(persona.created_at);
        self.template_for(persona, form_factor)
            .materialize(form_factor, major)
    }

    /// A minimal, non-empty fallback used ONLY if the bundled asset ever failed to
    /// parse (it cannot in a shipped build — the SHA-256 test and the parse test
    /// both guard it — but this keeps derivation TOTAL, mirroring the Android
    /// `DeviceDeriver.FALLBACK`). If this were ever reached, the golden-vector test
    /// would fail loudly, so it can never silently ship as the live catalog.
    fn fallback() -> Self {
        DeviceCatalog {
            mobile: vec![DeviceTemplate {
                model: "Pixel 8".to_string(),
                platform: "Android".to_string(),
                platform_version: "14.0.0".to_string(),
                ua_template: "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/{MAJOR}.0.0.0 Mobile Safari/537.36"
                    .to_string(),
                is_mobile: true,
                brands: grease_brands(),
                screen_width: 412,
                screen_height: 915,
                device_pixel_ratio: 2.625,
                hardware_concurrency: 8,
                device_memory: 8,
            }],
            desktop: vec![DeviceTemplate {
                model: String::new(),
                platform: "Windows".to_string(),
                platform_version: "15.0.0".to_string(),
                ua_template: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/{MAJOR}.0.0.0 Safari/537.36"
                    .to_string(),
                is_mobile: false,
                brands: grease_brands(),
                screen_width: 1920,
                screen_height: 1080,
                device_pixel_ratio: 1.0,
                hardware_concurrency: 8,
                device_memory: 16,
            }],
        }
    }
}

/// The Chromium brand triple (`Chromium`, `Google Chrome`, GREASE `Not?A_Brand`)
/// used by the fallback templates, with `{MAJOR}` unresolved.
fn grease_brands() -> Vec<Brand> {
    vec![
        Brand {
            name: "Chromium".to_string(),
            version: MAJOR_TOKEN.to_string(),
        },
        Brand {
            name: "Google Chrome".to_string(),
            version: MAJOR_TOKEN.to_string(),
        },
        Brand {
            name: "Not?A_Brand".to_string(),
            version: "24".to_string(),
        },
    ]
}

/// The parsed bundled catalog, resolved once on first use. On a parse failure (which
/// the pinned, tested asset makes impossible in a shipped build) it degrades to the
/// non-empty [`DeviceCatalog::fallback`] so derivation stays total — never a panic,
/// never an `unwrap`.
static BUNDLED_CATALOG: LazyLock<DeviceCatalog> = LazyLock::new(|| {
    DeviceCatalog::parse(DEVICE_TEMPLATES_JSON).unwrap_or_else(|_| DeviceCatalog::fallback())
});

/// The Chrome major a persona claims, pinned to its creation time: stable for the
/// persona's ~7-day life, monotonic across personas (`created_at` only increases),
/// and tracking real calendar time. Floors at [`BASELINE_MAJOR`] so a persona minted
/// before the baseline never claims a version below it. Matches the Android
/// `DeviceDeriver.chromeMajor` exactly.
pub fn chrome_major(created_at_ms: i64) -> u32 {
    let elapsed = created_at_ms - BASELINE_EPOCH_MS;
    if elapsed <= 0 {
        BASELINE_MAJOR
    } else {
        // `elapsed / RELEASE_INTERVAL_MS` is a small non-negative count; the cast is
        // lossless for any realistic `created_at`, matching Kotlin's `.toInt()`.
        BASELINE_MAJOR + (elapsed / RELEASE_INTERVAL_MS) as u32
    }
}

/// Deterministic index into `size` options from `SHA-256(id | domain | index)`, the
/// portable determinism primitive shared with Android (only a hash + canonical
/// bytes, no PRNG, no floats). Takes the first 4 digest bytes as a big-endian `u32`
/// and returns `n % size`. `size` must be non-empty (guaranteed by the catalog).
fn pick(id: &str, domain: &str, index: u32, size: usize) -> usize {
    debug_assert!(size > 0, "pick requires a non-empty option list");
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    hasher.update([SEPARATOR]);
    hasher.update(domain.as_bytes());
    hasher.update([SEPARATOR]);
    hasher.update(index.to_string().as_bytes());
    let digest = hasher.finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    // `size` fits in a u32 (the catalog is tiny), so the modulo matches Kotlin's
    // `(n % size).toInt()` bit-for-bit.
    (n % size as u32) as usize
}

/// The lowercase-hex SHA-256 of the vendored [`DEVICE_TEMPLATES_JSON`] bytes. This
/// is the pinned cross-repo checksum: the Android app asserts the identical value
/// over the same bytes, so a change to the shared catalog that is not mirrored in
/// both repos (and both pinned values) is caught by the checksum test on each side.
/// Also suitable for surfacing in an "about"/diagnostics view.
pub fn device_templates_sha256() -> String {
    let digest = Sha256::digest(DEVICE_TEMPLATES_JSON.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Two lowercase hex digits per byte; matches the Kotlin
        // `"%02x".format(it)` join.
        hex.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        hex.push(char::from_digit((byte & 0x0F) as u32, 16).unwrap_or('0'));
    }
    hex
}

/// The persona's DESKTOP device identity — the one this companion presents on the
/// decoy browser over its real desktop TLS. This is the primary entry point (#47).
pub fn desktop_for(persona: &SyntheticPersona) -> DeviceProfile {
    BUNDLED_CATALOG.device_for(persona, FormFactor::Desktop)
}

/// The persona's MOBILE device identity — what the phone's WebView presents. The
/// desktop NEVER emits this onto its browser; it is exposed only for the
/// cross-language parity assertion and any UI that wants to show the paired phone's
/// identity.
pub fn mobile_for(persona: &SyntheticPersona) -> DeviceProfile {
    BUNDLED_CATALOG.device_for(persona, FormFactor::Mobile)
}

/// The persona's full device set: exactly one MOBILE + one DESKTOP identity, in that
/// order (matching the Android `devicesFor`).
pub fn devices_for(persona: &SyntheticPersona) -> [DeviceProfile; 2] {
    [mobile_for(persona), desktop_for(persona)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::SyntheticPersona;

    fn persona(id: &str, created_at: i64) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            "n".to_string(),
            "AGE_35_44".to_string(),
            "ENGINEER".to_string(),
            "US_MIDWEST".to_string(),
            vec!["TECHNOLOGY".to_string()],
            created_at,
            created_at + 7 * 24 * 60 * 60 * 1000,
        )
    }

    /// The pinned SHA-256 of the vendored `device_templates.json`. The Android
    /// `DeviceDeriverTest` asserts the identical value over the identical bytes; the
    /// committed `device_interop_vector.json` carries it too. Changing the shared
    /// catalog means updating BOTH repos' pinned values in lockstep.
    const DEVICE_TEMPLATES_SHA256: &str =
        "3059247b5e83ea09b3ec69d8ed68577c4ceff27d3ca09f0842dd6db0b1e7a3dd";

    #[test]
    fn bundled_templates_match_the_pinned_checksum() {
        assert_eq!(
            device_templates_sha256(),
            DEVICE_TEMPLATES_SHA256,
            "device_templates.json changed — update this checksum AND the vendored copy \
             + the pinned value in the Android DeviceDeriverTest + device_interop_vector.json"
        );
    }

    // --- the bundled catalog parses and has the expected shape ---

    #[test]
    fn bundled_catalog_parses_and_is_non_empty() {
        let catalog = match DeviceCatalog::parse(DEVICE_TEMPLATES_JSON) {
            Ok(c) => c,
            Err(e) => panic!("bundled device_templates.json must parse: {e}"),
        };
        assert_eq!(catalog.mobile.len(), 6, "mobile template count is frozen");
        assert_eq!(catalog.desktop.len(), 4, "desktop template count is frozen");
        // Every mobile template is Android-Chromium; every desktop template is not.
        for m in &catalog.mobile {
            assert!(m.is_mobile);
            assert_eq!(m.platform, "Android");
        }
        for d in &catalog.desktop {
            assert!(!d.is_mobile);
            assert!(["Windows", "macOS", "Linux"].contains(&d.platform.as_str()));
        }
    }

    #[test]
    fn live_derivation_uses_the_bundled_catalog_not_the_fallback() {
        // If the bundled asset silently failed to parse, `BUNDLED_CATALOG` would be
        // the 1-desktop fallback and every persona would resolve to the same Windows
        // device. Two personas that hit DIFFERENT desktop templates prove the real
        // (4-template) catalog is live.
        let a = desktop_for(&persona(
            "11111111-1111-4111-8111-111111111111",
            BASELINE_EPOCH_MS,
        ));
        let b = desktop_for(&persona(
            "22222222-2222-4222-8222-222222222222",
            BASELINE_EPOCH_MS,
        ));
        assert_eq!(a.platform, "macOS");
        assert_eq!(b.platform, "Windows");
    }

    // --- chrome_major (pure) ---

    #[test]
    fn chrome_major_floors_at_baseline() {
        assert_eq!(chrome_major(BASELINE_EPOCH_MS), BASELINE_MAJOR);
        assert_eq!(chrome_major(BASELINE_EPOCH_MS - 5_000), BASELINE_MAJOR);
        assert_eq!(chrome_major(0), BASELINE_MAJOR);
    }

    #[test]
    fn chrome_major_advances_one_per_interval() {
        assert_eq!(
            chrome_major(BASELINE_EPOCH_MS + RELEASE_INTERVAL_MS),
            BASELINE_MAJOR + 1
        );
        assert_eq!(
            chrome_major(BASELINE_EPOCH_MS + 3 * RELEASE_INTERVAL_MS + 1),
            BASELINE_MAJOR + 3
        );
    }

    #[test]
    fn chrome_major_is_monotonic_non_decreasing() {
        let mut prev = chrome_major(0);
        let mut t = BASELINE_EPOCH_MS;
        for _ in 0..40 {
            t += 9 * 24 * 60 * 60 * 1000; // 9-day steps
            let cur = chrome_major(t);
            assert!(cur >= prev, "major must never go backwards");
            prev = cur;
        }
    }

    // --- pick (pure) ---

    #[test]
    fn pick_is_deterministic_and_within_range() {
        for i in 0..20 {
            let id = format!("persona-{i}");
            let a = pick(&id, DOMAIN_MOBILE, 0, 6);
            let b = pick(&id, DOMAIN_MOBILE, 0, 6);
            assert_eq!(a, b);
            assert!(a < 6);
        }
    }

    #[test]
    fn pick_varies_across_persona_ids() {
        let distinct: std::collections::HashSet<usize> = (0..60)
            .map(|i| pick(&format!("id-{i}"), DOMAIN_MOBILE, 0, 6))
            .collect();
        assert!(distinct.len() > 1, "selection must not be degenerate");
    }

    // --- derivation shape ---

    #[test]
    fn desktop_is_never_mobile_and_mobile_is_android() {
        let p = persona("shape-id", BASELINE_EPOCH_MS);
        let desktop = desktop_for(&p);
        let mobile = mobile_for(&p);

        assert_eq!(desktop.form_factor, FormFactor::Desktop);
        assert!(!desktop.is_mobile, "desktop must never be mobile");
        assert!(
            !desktop.user_agent.contains("Mobile"),
            "no Mobile token on desktop"
        );
        assert!(
            !desktop.user_agent.contains("Android"),
            "desktop is not Android"
        );
        assert!(["Windows", "macOS", "Linux"].contains(&desktop.platform.as_str()));

        assert_eq!(mobile.form_factor, FormFactor::Mobile);
        assert!(mobile.is_mobile);
        assert_eq!(mobile.platform, "Android");
        assert!(mobile.user_agent.contains("Mobile"));
    }

    #[test]
    fn no_headless_token_and_no_unresolved_major_leaks() {
        // The whole point of the desktop identity is a clean, coherent UA. Nothing
        // derived may carry the headless tell or an unresolved template token.
        for i in 0..40 {
            let p = persona(
                &format!("id-{i}"),
                BASELINE_EPOCH_MS + i as i64 * RELEASE_INTERVAL_MS,
            );
            for d in devices_for(&p) {
                assert!(
                    !d.user_agent.contains("HeadlessChrome"),
                    "UA leaked headless: {}",
                    d.user_agent
                );
                assert!(
                    !d.user_agent.contains(MAJOR_TOKEN),
                    "unresolved token: {}",
                    d.user_agent
                );
                for b in &d.brands {
                    assert!(!b.version.contains(MAJOR_TOKEN), "unresolved brand token");
                }
                assert!(!d
                    .full_version_brands()
                    .iter()
                    .any(|b| b.version.contains(MAJOR_TOKEN)));
            }
        }
    }

    #[test]
    fn derivation_is_stable_across_calls() {
        let p = persona("stable-id", BASELINE_EPOCH_MS + 42 * RELEASE_INTERVAL_MS);
        assert_eq!(desktop_for(&p), desktop_for(&p));
        assert_eq!(mobile_for(&p), mobile_for(&p));
    }

    #[test]
    fn major_is_substituted_into_ua_and_brands() {
        // A persona 5 intervals past baseline claims 147.
        let p = persona("v-id", BASELINE_EPOCH_MS + 5 * RELEASE_INTERVAL_MS);
        let major = (BASELINE_MAJOR + 5).to_string();
        let d = desktop_for(&p);
        assert!(
            d.user_agent.contains(&format!("Chrome/{major}.")),
            "UA: {}",
            d.user_agent
        );
        let chrome = d
            .brands
            .iter()
            .find(|b| b.name == "Google Chrome")
            .map(|b| b.version.as_str());
        assert_eq!(chrome, Some(major.as_str()));
        let full = d.full_version_brands();
        let chrome_full = full
            .iter()
            .find(|b| b.name == "Google Chrome")
            .map(|b| b.version.clone());
        assert_eq!(chrome_full, Some(format!("{major}.0.0.0")));
    }

    #[test]
    fn architecture_and_bitness_are_coherent() {
        let p = persona("arch-id", BASELINE_EPOCH_MS);
        let d = desktop_for(&p);
        // All current desktop templates are Intel/x86_64.
        assert_eq!(d.architecture(), "x86");
        assert_eq!(d.bitness(), "64");
        let m = mobile_for(&p);
        assert_eq!(m.architecture(), "arm");
    }

    #[test]
    fn navigator_platform_is_the_legacy_token_not_the_client_hint() {
        // Every desktop template resolves to a legacy navigator.platform token that
        // is coherent with (but textually distinct from) its userAgentData.platform.
        for i in 0..40 {
            let d = desktop_for(&persona(&format!("np-{i}"), BASELINE_EPOCH_MS));
            let np = d.navigator_platform();
            assert!(
                np.is_some(),
                "known desktop platform must map: {}",
                d.platform
            );
            assert_ne!(
                np,
                Some(d.platform.as_str()),
                "navigator.platform must not equal the client-hint platform"
            );
            match d.platform.as_str() {
                "macOS" => assert_eq!(np, Some("MacIntel")),
                "Windows" => assert_eq!(np, Some("Win32")),
                "Linux" => assert_eq!(np, Some("Linux x86_64")),
                other => panic!("unexpected desktop platform {other}"),
            }
        }
    }

    #[test]
    fn fallback_catalog_is_non_empty_and_total() {
        // The fallback must never yield an empty option list (that would panic in
        // `pick`); derivation over it stays total.
        let fb = DeviceCatalog::fallback();
        assert!(!fb.mobile.is_empty());
        assert!(!fb.desktop.is_empty());
        let p = persona("fb-id", BASELINE_EPOCH_MS);
        let d = fb.device_for(&p, FormFactor::Desktop);
        assert!(!d.is_mobile);
        assert!(!d.user_agent.contains(MAJOR_TOKEN));
    }
}
