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

//! Persona-pack minting: the PUMS-microdata persona generator (C6 #29, H2).
//!
//! This module mints COHERENT synthetic personas from a US-only PUMS-style joint
//! distribution and bundles them into a SIGNED [`PersonaPack`] the phone imports.
//! It matches the FROZEN Android E7 PUMS model: a [`DemographicCell`] is a
//! co-occurring `(age, profession, region)` triple with a population `weight`,
//! and a persona's demographics are drawn JOINTLY (a weighted multinomial draw
//! over the cells), so the three fields co-occur realistically rather than being
//! picked independently across an impossible age x profession x region product.
//!
//! ## The bundled distribution (real ACS-PUMS)
//!
//! The distribution is loaded from a BUNDLED JSON
//! ([`persona_distribution.json`](DEFAULT_DISTRIBUTION_JSON)) embedded at compile
//! time via `include_str!`. It is the REAL joint `P(AgeRange, Profession, Region)`
//! over US adults, built OFFLINE from US Census ACS PUMS 2022 microdata
//! (PWGTP-weighted, with region marginals post-stratified to Census state
//! population estimates) by the Android `scripts/build_persona_distribution.py` -
//! it is byte-for-byte the same file the phone ships. US-only by design (315 cells
//! over 6 ages x 12 professions x 5 US regions). The minter validates every cell
//! on load: `weight > 0`, valid [`AgeRange`]/[`Profession`]/[`Region`] enum NAMES,
//! and US-only (the region NAME starts with `US_`).
//!
//! ## What minting guarantees
//!
//! Each minted [`SyntheticPersona`] carries: a fresh UUID v4 id; demographics from
//! one jointly-sampled cell; 3-to-5 interests; the frozen 8-to-10-day `activeUntil`
//! rotation window ([`crate::constants::BASE_ROTATION_DAYS`] +
//! [`crate::constants::ROTATION_JITTER_DAYS`]); and NO
//! [`Severity::HardImplausible`](crate::studio::Severity) finding from the C5
//! coherence linter ([`crate::studio::lint_persona`]). A draw that the linter flags
//! is rejected and re-sampled (bounded by [`MAX_RESAMPLE_ATTEMPTS`]) so minted
//! personas are coherent. Sampling is DETERMINISTIC for a fixed seed (a seedable
//! [`StdRng`]).
//!
//! ## Packs ride O1
//!
//! [`mint_pack`] bundles the minted personas into a signed [`PersonaPack`]
//! (REUSING the C5 P4 [`crate::personapack`] format) with a [`PackProvenance`]
//! recording the source distribution label and the generation seed. The pack
//! distributes to paired peers over the existing O1 sealed channel as the new
//! [`SyncBody::PersonaPack`](crate::sync::wire::SyncBody) wire kind, which the
//! receiver VERIFIES (P4 [`verify_pack`](crate::personapack::verify_pack)) before
//! importing (verify-before-write, fail closed). See
//! [`Core::mint_and_push_pack`](crate::Core::mint_and_push_pack),
//! [`Core::receive_pack_frame`](crate::Core::receive_pack_frame), and
//! `docs/SYNC_PROTOCOL.md`.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::persona::{AgeRange, CategoryPool, Profession, Region, SyntheticPersona};
use crate::personapack::{
    sign_pack_with, PackContent, PackProvenance, PackSigningKey, PersonaPack,
};
use crate::studio::lint_persona;

/// The bundled joint distribution, embedded at compile time: the REAL ACS-PUMS
/// 2022 export (the same file the phone ships), built offline by the Android
/// `scripts/build_persona_distribution.py`. See the module docs.
pub const DEFAULT_DISTRIBUTION_JSON: &str = include_str!("persona_distribution.json");

/// The number of interests every minted persona carries is drawn from this
/// inclusive range, matching the frozen [`crate::persona::INTEREST_COUNT`] rule
/// (3..=5). Defined here as the mint-side draw bound.
pub const MINT_INTEREST_COUNT: std::ops::RangeInclusive<usize> = 3..=5;

/// How many times the minter re-draws a persona the coherence linter flags as
/// [`Severity::HardImplausible`](crate::studio::Severity) before giving up. The
/// distribution is curated so a flag is rare; this bounds the loop so a pathological
/// distribution cannot spin forever (it errors with [`MintError::Incoherent`]
/// instead, fail closed).
pub const MAX_RESAMPLE_ATTEMPTS: u32 = 64;

/// Typed minting failures. Returned so callers can match the failure mode rather
/// than parsing a string. A failure is NEVER a silent partial mint.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MintError {
    /// The distribution JSON was not valid, or a field was malformed.
    #[error("malformed persona distribution: {0}")]
    Malformed(String),

    /// The distribution had no cells, so there is nothing to sample.
    #[error("persona distribution is empty (no cells to sample)")]
    Empty,

    /// A cell failed validation (a non-positive weight, an unknown enum NAME, or a
    /// non-US region). Carries the human-readable reason and the offending cell
    /// index. Fail closed: an invalid distribution never mints.
    #[error("invalid distribution cell at index {index}: {reason}")]
    InvalidCell {
        /// The zero-based index of the offending cell.
        index: usize,
        /// Why the cell is invalid.
        reason: String,
    },

    /// A coherent persona could not be drawn within [`MAX_RESAMPLE_ATTEMPTS`].
    /// Surfaced rather than emitting an incoherent persona (fail closed).
    #[error(
        "could not mint a coherent persona within {MAX_RESAMPLE_ATTEMPTS} attempts \
         (the distribution may be pathological)"
    )]
    Incoherent,

    /// Signing the minted pack failed (wraps the persona-pack error).
    #[error("minted-pack signing failed: {0}")]
    Sign(#[from] crate::personapack::PackError),
}

/// One PUMS-style demographic cell: a co-occurring `(age, profession, region)`
/// triple plus its population `weight`. This is the unit of the joint
/// distribution: the minter draws a WHOLE cell (so the three fields co-occur),
/// never the three fields independently. Aligned with the frozen Android E7
/// `DemographicCell`.
///
/// `age`/`profession`/`region` are the frozen enum NAMES ([`AgeRange`],
/// [`Profession`], [`Region`]); `region` is always US-only (starts with `US_`).
/// Serialized in snake_case to match the offline PUMS export file format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemographicCell {
    /// The [`AgeRange`] enum NAME (e.g. `"AGE_35_44"`).
    pub age: String,
    /// The [`Profession`] enum NAME (e.g. `"FINANCE_PROF"`).
    pub profession: String,
    /// The [`Region`] enum NAME (e.g. `"US_MIDWEST"`); US-only.
    pub region: String,
    /// The cell's relative population weight (strictly positive). The sampler
    /// normalizes over the sum of all cell weights, so weights need not sum to 1.
    pub weight: f64,
}

impl DemographicCell {
    /// Validate this cell: `weight` is finite and strictly positive, the three
    /// fields are valid enum NAMES, and the region is US-only (its NAME starts
    /// with `US_`). Returns the reason on the first problem found, else `None`.
    pub fn validation_error(&self) -> Option<String> {
        if !(self.weight.is_finite() && self.weight > 0.0) {
            return Some(format!("weight {} is not strictly positive", self.weight));
        }
        if AgeRange::from_name(&self.age).is_none() {
            return Some(format!("age {:?} is not a known AgeRange name", self.age));
        }
        if Profession::from_name(&self.profession).is_none() {
            return Some(format!(
                "profession {:?} is not a known Profession name",
                self.profession
            ));
        }
        match Region::from_name(&self.region) {
            None => Some(format!(
                "region {:?} is not a known Region name",
                self.region
            )),
            // US-only: the joint distribution is US PUMS microdata, so a cell whose
            // region is a valid enum but not a US_* region is rejected (fail closed).
            Some(_) if !self.region.starts_with("US_") => Some(format!(
                "region {:?} is not US-only (must start with US_)",
                self.region
            )),
            Some(_) => None,
        }
    }
}

/// A versioned PUMS-style joint distribution: a format `version`, a human-readable
/// `source_label` (e.g. a PUMS vintage), and the [`DemographicCell`] list the
/// minter samples. Loaded from the bundled JSON via `serde`.
///
/// Serialized in snake_case to match the offline PUMS export file format. The
/// export's documentation fields (`note`, `dimensions`) are ignored on load, and
/// its `source` field is read into [`source_label`](Self::source_label) via a
/// serde alias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaDistribution {
    /// The distribution format version. Bumped when the cell schema changes.
    pub version: u32,
    /// A human-readable label for this distribution's source (recorded into the
    /// pack provenance). Reads the export's `source` field (serde alias);
    /// defaults to a generic label when absent.
    #[serde(default = "default_source_label", alias = "source")]
    pub source_label: String,
    /// The demographic cells the minter draws from.
    pub cells: Vec<DemographicCell>,
}

/// The default source label when the distribution file omits one.
fn default_source_label() -> String {
    "US_PUMS_SEED".to_string()
}

impl PersonaDistribution {
    /// Load and VALIDATE the bundled distribution (the compile-time real
    /// ACS-PUMS [`DEFAULT_DISTRIBUTION_JSON`]). See [`Self::from_json`].
    pub fn bundled() -> std::result::Result<Self, MintError> {
        Self::from_json(DEFAULT_DISTRIBUTION_JSON)
    }

    /// Parse a distribution from JSON and VALIDATE every cell (`weight > 0`, valid
    /// enum NAMES, US-only region). Fails closed: malformed JSON is
    /// [`MintError::Malformed`], an empty cell list is [`MintError::Empty`], and a
    /// bad cell is [`MintError::InvalidCell`].
    pub fn from_json(json: &str) -> std::result::Result<Self, MintError> {
        let dist: PersonaDistribution =
            serde_json::from_str(json).map_err(|e| MintError::Malformed(e.to_string()))?;
        dist.validate()?;
        Ok(dist)
    }

    /// Validate the distribution: at least one cell, and every cell valid.
    pub fn validate(&self) -> std::result::Result<(), MintError> {
        if self.cells.is_empty() {
            return Err(MintError::Empty);
        }
        for (index, cell) in self.cells.iter().enumerate() {
            if let Some(reason) = cell.validation_error() {
                return Err(MintError::InvalidCell { index, reason });
            }
        }
        Ok(())
    }

    /// The total of all cell weights (the normalizer for the multinomial draw).
    /// Strictly positive on a validated distribution.
    fn total_weight(&self) -> f64 {
        self.cells.iter().map(|c| c.weight).sum()
    }

    /// Draw ONE cell JOINTLY via a weighted multinomial draw (cumulative search):
    /// sample `u` in `[0, total_weight)` and walk the cells accumulating weight,
    /// returning the cell whose cumulative band contains `u`. This keeps the three
    /// demographic fields CO-OCCURRING (a real cell's triple), never an independent
    /// cross-product. The caller supplies the RNG so the draw is deterministic for
    /// a fixed seed.
    fn sample_cell(&self, rng: &mut StdRng) -> &DemographicCell {
        let total = self.total_weight();
        let mut pick = rng.random::<f64>() * total;
        for cell in &self.cells {
            pick -= cell.weight;
            if pick < 0.0 {
                return cell;
            }
        }
        // Floating-point fallthrough: attribute to the last cell. A validated
        // distribution always has at least one cell, so this index is in range.
        &self.cells[self.cells.len() - 1]
    }
}

/// The outcome of a mint pass: the coherent personas and the seed they were drawn
/// with (so the draw is reproducible and the seed can be recorded into the pack
/// provenance).
#[derive(Debug, Clone)]
pub struct MintedPersonas {
    /// The source distribution label the personas were sampled from.
    pub source_label: String,
    /// The generation seed (deterministic re-draw with the same seed).
    pub seed: u64,
    /// The minted, coherent personas.
    pub personas: Vec<SyntheticPersona>,
}

/// Mint `count` coherent personas from `dist`, deterministically seeded by `seed`.
///
/// Each persona is drawn by: sampling ONE [`DemographicCell`] jointly (so the
/// age/profession/region co-occur), sampling 3-to-5 distinct interests, stamping a
/// fresh UUID and the frozen 8-to-10-day `activeUntil` window relative to
/// `created_at`, then accepting it only if the C5 coherence linter finds NO
/// [`Severity::HardImplausible`](crate::studio::Severity) issue (otherwise the
/// persona is re-drawn, bounded by [`MAX_RESAMPLE_ATTEMPTS`]). Deterministic: the
/// same `(dist, count, seed, created_at)` yields identical personas (a fresh UUID
/// is the one non-deterministic field, by design, since ids must be globally
/// unique).
///
/// `created_at` is the epoch-millis creation timestamp stamped on every persona;
/// the `activeUntil` is `created_at + (8..=10 days)`.
///
/// Fails closed: an empty `dist` cannot be passed (it is validated), and a
/// persistently incoherent draw is [`MintError::Incoherent`] rather than an
/// incoherent persona.
pub fn mint_personas(
    dist: &PersonaDistribution,
    count: usize,
    seed: u64,
    created_at: i64,
) -> std::result::Result<MintedPersonas, MintError> {
    dist.validate()?;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut personas = Vec::with_capacity(count);
    for _ in 0..count {
        personas.push(mint_one(dist, &mut rng, created_at)?);
    }
    Ok(MintedPersonas {
        source_label: dist.source_label.clone(),
        seed,
        personas,
    })
}

/// Mint one coherent persona, re-drawing on a hard-implausible draw.
fn mint_one(
    dist: &PersonaDistribution,
    rng: &mut StdRng,
    created_at: i64,
) -> std::result::Result<SyntheticPersona, MintError> {
    for _ in 0..MAX_RESAMPLE_ATTEMPTS {
        let cell = dist.sample_cell(rng);
        let interests = sample_interests(rng);
        let active_until = created_at.saturating_add(rotation_window_millis(rng));
        // A fresh UUID v4 (the one field that must be globally unique, so it is the
        // single non-deterministic element of an otherwise seed-reproducible draw).
        let persona = SyntheticPersona::new(
            uuid::Uuid::new_v4().to_string(),
            mint_name(cell),
            cell.age.clone(),
            cell.profession.clone(),
            cell.region.clone(),
            interests,
            created_at,
            active_until,
        );
        // Accept only a coherent draw: NO HardImplausible finding from the C5
        // linter. (Completeness rules cannot fire here: the demographics come from
        // a validated cell and the interest count is always 3..=5.)
        if !lint_persona(&persona).iter().any(|f| f.is_hard()) {
            return Ok(persona);
        }
    }
    Err(MintError::Incoherent)
}

/// Sample a count in [`MINT_INTEREST_COUNT`] (3..=5) of DISTINCT interest
/// categories via a partial Fisher-Yates shuffle over the frozen
/// [`CategoryPool`], so a persona carries the right number of unique interests.
fn sample_interests(rng: &mut StdRng) -> Vec<String> {
    let all = CategoryPool::all();
    let mut pool: Vec<&CategoryPool> = all.iter().collect();
    let count = rng
        .random_range(*MINT_INTEREST_COUNT.start()..=*MINT_INTEREST_COUNT.end())
        .min(pool.len());
    // Partial Fisher-Yates: select `count` distinct entries by swapping each
    // chosen index to the front, drawing from the unselected tail each time.
    for i in 0..count {
        let j = rng.random_range(i..pool.len());
        pool.swap(i, j);
    }
    pool[..count]
        .iter()
        .map(|c| c.as_name().to_string())
        .collect()
}

/// The frozen 8-to-10-day rotation window in milliseconds:
/// [`BASE_ROTATION_DAYS`](crate::constants::BASE_ROTATION_DAYS) (7) plus a jitter
/// drawn from [`ROTATION_JITTER_DAYS`](crate::constants::ROTATION_JITTER_DAYS)
/// (1..=3), added never subtracted, so the effective window is 8 to 10 days.
fn rotation_window_millis(rng: &mut StdRng) -> i64 {
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;
    let jitter = rng.random_range(
        *crate::constants::ROTATION_JITTER_DAYS.start()
            ..=*crate::constants::ROTATION_JITTER_DAYS.end(),
    );
    let days = i64::from(crate::constants::BASE_ROTATION_DAYS + jitter);
    days * MS_PER_DAY
}

/// Convert a SCREAMING_SNAKE_CASE wire name to a human Title Case label
/// (`"SOFTWARE_ENGINEER"` -> `"Software Engineer"`). Cosmetic only: it shapes the
/// minted persona's display `name`; the frozen wire fields keep the raw names.
fn humanize_name(raw: &str) -> String {
    raw.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A short, neutral display name for a minted persona, derived from its
/// demographics. Deliberately generic (no real personal data); the GUI can rename.
fn mint_name(cell: &DemographicCell) -> String {
    format!(
        "Minted {} {}",
        humanize_name(&cell.profession),
        humanize_name(&cell.region)
    )
}

/// Bundle minted personas into a SIGNED [`PersonaPack`] (REUSING the C5 P4 pack
/// format), recording the source distribution label and the generation seed in the
/// [`PackProvenance`], and signing the canonical content with `key`.
///
/// The pack round-trips to the phone's [`SyntheticPersona`] shape (guaranteed by
/// P4) and verifies with [`verify_pack`](crate::personapack::verify_pack). Hermetic
/// and low-level: it touches no store and no keystore, so a test can sign with a
/// fixed [`PackSigningKey`] and verify deterministically.
pub fn mint_pack(
    minted: &MintedPersonas,
    created_at: i64,
    key: &PackSigningKey,
) -> std::result::Result<PersonaPack, MintError> {
    let provenance = PackProvenance::us(
        minted.source_label.clone(),
        minted.seed.to_string(),
        created_at,
    );
    let content = PackContent::new(provenance, minted.personas.clone());
    Ok(sign_pack_with(content, key)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personapack::{verify_pack, PackError, PackSigningKey, PACK_SEED_LEN};
    use crate::studio::Severity;

    const FIXED_SEED: [u8; PACK_SEED_LEN] = [13u8; PACK_SEED_LEN];
    const CREATED_AT: i64 = 1_700_000_000_000;
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;

    #[test]
    fn humanize_name_titlecases_screaming_snake() {
        assert_eq!(humanize_name("SOFTWARE_ENGINEER"), "Software Engineer");
        assert_eq!(humanize_name("WEST_COAST"), "West Coast");
        assert_eq!(humanize_name("RETAIL"), "Retail");
        // Defensive: stray/edge separators never yield empty or doubled spaces.
        assert_eq!(humanize_name("A__B"), "A B");
        assert_eq!(humanize_name(""), "");
    }

    fn fixed_key() -> PackSigningKey {
        PackSigningKey::from_seed(&FIXED_SEED)
    }

    #[test]
    fn bundled_distribution_loads_and_every_cell_validates() -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        assert!(!dist.cells.is_empty());
        assert_eq!(dist.version, 1);
        assert!(!dist.source_label.is_empty());
        // This is the REAL ACS-PUMS export, not the old hand-authored seed: the
        // full joint over 6 ages x 12 professions x 5 US regions, weights forming
        // a probability distribution, with the PUMS provenance read from the
        // export's `source` field via the serde alias.
        assert!(
            dist.cells.len() > 100,
            "bundled distribution should be the full PUMS export, got {} cells",
            dist.cells.len()
        );
        assert!(
            dist.source_label.contains("PUMS"),
            "source label should record the PUMS provenance, got {:?}",
            dist.source_label
        );
        let weight_sum: f64 = dist.cells.iter().map(|c| c.weight).sum();
        assert!(
            (weight_sum - 1.0).abs() < 1e-6,
            "PUMS cell weights should sum to ~1.0, got {weight_sum}"
        );
        for (i, cell) in dist.cells.iter().enumerate() {
            assert!(
                cell.validation_error().is_none(),
                "cell {i} should validate: {cell:?}"
            );
            // US-only, weight > 0, valid enum names (the validation contract).
            assert!(cell.weight > 0.0);
            assert!(cell.region.starts_with("US_"));
            assert!(AgeRange::from_name(&cell.age).is_some());
            assert!(Profession::from_name(&cell.profession).is_some());
            assert!(Region::from_name(&cell.region).is_some());
        }
        Ok(())
    }

    #[test]
    fn empty_distribution_is_rejected() {
        let dist = PersonaDistribution {
            version: 1,
            source_label: "x".to_string(),
            cells: Vec::new(),
        };
        assert!(matches!(dist.validate(), Err(MintError::Empty)));
    }

    #[test]
    fn non_us_region_cell_is_rejected() {
        let json = r#"{
            "version": 1,
            "cells": [
                { "age": "AGE_35_44", "profession": "ENGINEER", "region": "CANADA", "weight": 1.0 }
            ]
        }"#;
        match PersonaDistribution::from_json(json) {
            Err(MintError::InvalidCell { index, reason }) => {
                assert_eq!(index, 0);
                assert!(reason.contains("US-only"), "reason was: {reason}");
            }
            other => panic!("expected InvalidCell for a non-US region, got {other:?}"),
        }
    }

    #[test]
    fn non_positive_weight_and_unknown_enum_are_rejected() {
        let zero_weight = r#"{
            "version": 1,
            "cells": [
                { "age": "AGE_35_44", "profession": "ENGINEER", "region": "US_WEST", "weight": 0.0 }
            ]
        }"#;
        assert!(matches!(
            PersonaDistribution::from_json(zero_weight),
            Err(MintError::InvalidCell { .. })
        ));

        let unknown_profession = r#"{
            "version": 1,
            "cells": [
                { "age": "AGE_35_44", "profession": "ASTRONAUT", "region": "US_WEST", "weight": 1.0 }
            ]
        }"#;
        match PersonaDistribution::from_json(unknown_profession) {
            Err(MintError::InvalidCell { reason, .. }) => {
                assert!(reason.contains("Profession"), "reason was: {reason}");
            }
            other => panic!("expected InvalidCell for an unknown profession, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_typed_not_silent() {
        assert!(matches!(
            PersonaDistribution::from_json("not json"),
            Err(MintError::Malformed(_))
        ));
    }

    #[test]
    fn joint_sampling_is_deterministic_for_a_fixed_seed() -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        let a = mint_personas(&dist, 5, 42, CREATED_AT)?;
        let b = mint_personas(&dist, 5, 42, CREATED_AT)?;
        // Same seed => identical demographics + interests + windows (the UUID id is
        // the one intentionally-fresh field, so compare everything else).
        assert_eq!(a.personas.len(), 5);
        for (pa, pb) in a.personas.iter().zip(b.personas.iter()) {
            assert_eq!(pa.age_range, pb.age_range);
            assert_eq!(pa.profession, pb.profession);
            assert_eq!(pa.region, pb.region);
            assert_eq!(pa.interests, pb.interests);
            assert_eq!(pa.active_until, pb.active_until);
        }
        // A different seed re-rolls the draw (overwhelmingly likely to differ on at
        // least one field across 5 personas).
        let c = mint_personas(&dist, 5, 7, CREATED_AT)?;
        let differs = a.personas.iter().zip(c.personas.iter()).any(|(pa, pc)| {
            pa.age_range != pc.age_range
                || pa.profession != pc.profession
                || pa.region != pc.region
                || pa.interests != pc.interests
        });
        assert!(differs, "a new seed should re-roll the draw");
        Ok(())
    }

    #[test]
    fn sampled_demographics_co_occur_as_a_cell_triple_not_a_cross_product(
    ) -> std::result::Result<(), MintError> {
        // Every minted (age, profession, region) MUST be one of the distribution's
        // actual cell triples, never an impossible cross-product (e.g. AGE_18_24 +
        // RETIRED). Mint a large batch over several seeds and assert membership.
        let dist = PersonaDistribution::bundled()?;
        let cell_triples: std::collections::HashSet<(String, String, String)> = dist
            .cells
            .iter()
            .map(|c| (c.age.clone(), c.profession.clone(), c.region.clone()))
            .collect();
        for seed in 0..16u64 {
            let minted = mint_personas(&dist, 8, seed, CREATED_AT)?;
            for p in &minted.personas {
                let triple = (p.age_range.clone(), p.profession.clone(), p.region.clone());
                assert!(
                    cell_triples.contains(&triple),
                    "minted triple {triple:?} is not a real cell (impossible cross-product)"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn minted_personas_pass_the_linter_and_carry_window_and_interests(
    ) -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        for seed in 0..16u64 {
            let minted = mint_personas(&dist, 6, seed, CREATED_AT)?;
            for p in &minted.personas {
                // No HardImplausible finding from the C5 coherence linter.
                let findings = lint_persona(p);
                assert!(
                    !findings
                        .iter()
                        .any(|f| f.severity == Severity::HardImplausible),
                    "minted persona should have no HardImplausible finding: {findings:?}"
                );
                // 3..=5 interests, all distinct, all known categories.
                assert!(MINT_INTEREST_COUNT.contains(&p.interests.len()));
                let unique: std::collections::HashSet<&String> = p.interests.iter().collect();
                assert_eq!(
                    unique.len(),
                    p.interests.len(),
                    "interests must be distinct"
                );
                for i in &p.interests {
                    assert!(CategoryPool::from_name(i).is_some());
                }
                // The frozen 8-to-10-day rotation window.
                let window = p.active_until - p.created_at;
                assert!(
                    (8 * MS_PER_DAY..=10 * MS_PER_DAY).contains(&window),
                    "rotation window {window} ms is outside 8..=10 days"
                );
                assert_eq!(p.created_at, CREATED_AT);
                // A fresh, parseable UUID id.
                assert!(uuid::Uuid::parse_str(&p.id).is_ok());
            }
        }
        Ok(())
    }

    #[test]
    fn mint_pack_signs_and_verifies() -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        let minted = mint_personas(&dist, 3, 99, CREATED_AT)?;
        let key = fixed_key();
        let pack = mint_pack(&minted, CREATED_AT, &key)?;

        // The pack signs and verifies (reusing P4).
        let bytes = pack.to_bytes()?;
        let verified = verify_pack(&bytes)?;
        assert_eq!(verified.content.personas, minted.personas);
        assert_eq!(verified.signer_public_key, key.public_key_base64());

        // Provenance records the source label + generation seed.
        assert_eq!(
            verified.content.provenance.source_distribution,
            dist.source_label
        );
        assert_eq!(verified.content.provenance.generation_seed, "99");
        assert_eq!(verified.content.provenance.country.as_deref(), Some("US"));
        Ok(())
    }

    #[test]
    fn tampered_minted_pack_is_rejected() -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        let minted = mint_personas(&dist, 2, 5, CREATED_AT)?;
        let key = fixed_key();
        let pack = mint_pack(&minted, CREATED_AT, &key)?;

        // Tamper a persona inside the signed content, keep the old signature.
        let mut tampered = pack.clone();
        tampered.content.personas[0].name = "Tampered".to_string();
        let bytes = tampered.to_bytes()?;
        match verify_pack(&bytes) {
            Err(PackError::BadSignature) => Ok(()),
            other => panic!("expected BadSignature, got {other:?}"),
        }
    }

    #[test]
    fn single_valid_cell_mints_coherent_personas() {
        // A single valid cell always mints: the random 3..=5 distinct-interest
        // draw clears the completeness rules, so the linter accepts it and the
        // resample bound is never hit. Reliably forcing MintError::Incoherent
        // would need a linter-injection seam; the bounded-resample-then-fail-closed
        // path itself is simple and documented in mint_personas.
        let json = r#"{
            "version": 1,
            "cells": [
                { "age": "AGE_35_44", "profession": "ENGINEER", "region": "US_WEST", "weight": 1.0 }
            ]
        }"#;
        let dist = match PersonaDistribution::from_json(json) {
            Ok(d) => d,
            Err(e) => panic!("single valid cell should load: {e}"),
        };
        let minted = match mint_personas(&dist, 4, 1, CREATED_AT) {
            Ok(m) => m,
            Err(e) => panic!("a coherent single-cell distribution should mint: {e}"),
        };
        assert_eq!(minted.personas.len(), 4);
        for p in &minted.personas {
            assert_eq!(p.age_range, "AGE_35_44");
            assert_eq!(p.profession, "ENGINEER");
            assert_eq!(p.region, "US_WEST");
        }
    }

    #[test]
    fn weighted_sampling_is_proportional() {
        // A heavy cell (weight 9) and a light cell (weight 1) with distinct
        // demographics: over many mints the heavy cell's demographic should
        // dominate roughly 9:1, proving the multinomial draw is weight-
        // proportional and not uniform.
        let json = r#"{
            "version": 1,
            "cells": [
                { "age": "AGE_25_34", "profession": "ENGINEER", "region": "US_WEST", "weight": 9.0 },
                { "age": "AGE_55_64", "profession": "RETAIL", "region": "US_SOUTHEAST", "weight": 1.0 }
            ]
        }"#;
        let dist = match PersonaDistribution::from_json(json) {
            Ok(d) => d,
            Err(e) => panic!("two-cell distribution should load: {e}"),
        };
        let minted = match mint_personas(&dist, 600, 7, CREATED_AT) {
            Ok(m) => m,
            Err(e) => panic!("two valid cells should mint: {e}"),
        };
        let heavy = minted
            .personas
            .iter()
            .filter(|p| p.age_range == "AGE_25_34")
            .count();
        let share = heavy as f64 / minted.personas.len() as f64;
        // Expected ~0.9; a generous band rules out uniform (0.5) while tolerating
        // sampling noise. Resampling on a (rare) linter rejection re-draws the
        // whole persona uniformly across cells, so it does not skew the ratio.
        assert!(
            (0.80..0.97).contains(&share),
            "heavy-cell share {share:.3} should be near the 9:1 weight ratio"
        );
    }

    #[test]
    fn distribution_round_trips_through_json() -> std::result::Result<(), MintError> {
        let dist = PersonaDistribution::bundled()?;
        let json = serde_json::to_string(&dist).map_err(|e| MintError::Malformed(e.to_string()))?;
        let back = PersonaDistribution::from_json(&json)?;
        assert_eq!(back.cells, dist.cells);
        assert_eq!(back.version, dist.version);
        assert_eq!(back.source_label, dist.source_label);
        Ok(())
    }
}
