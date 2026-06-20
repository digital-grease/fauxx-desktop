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

//! Persona-driven browsing cadence (C2 #11, R1).
//!
//! The dwell duration and scroll depth of a synthetic session are derived from
//! the persona so two different personas browse with visibly different rhythms,
//! while the SAME persona browsing the same page is reproducible under a fixed
//! seed (which is what the hermetic tests assert).
//!
//! The [`SyntheticPersona`] carries no explicit
//! browsing-style fields yet, so this module derives *plausible defaults* keyed
//! off the persona id: the id seeds a [`StdRng`], and the cadence is sampled
//! from that. C5 adds explicit browsing-style fields to the persona; when it
//! lands, [`BrowsingCadence::for_persona`] should read those instead of the
//! derived defaults (the seam is intentionally narrow so the swap is local).

use std::time::Duration;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::persona::SyntheticPersona;

/// Minimum dwell on a page (a quick skim). Bounds keep a headless run brisk and
/// keep the derived timing inside a human-plausible band.
const DWELL_MIN: Duration = Duration::from_millis(600);
/// Maximum dwell on a page (a thorough read).
const DWELL_MAX: Duration = Duration::from_millis(4_000);
/// Fewest scroll steps (barely engaged).
const SCROLL_STEPS_MIN: u32 = 2;
/// Most scroll steps (reads to the bottom).
const SCROLL_STEPS_MAX: u32 = 12;
/// Pixels advanced per scroll step.
const SCROLL_STEP_PX: i64 = 320;
/// Shortest pause between scroll steps.
const SCROLL_PAUSE_MIN: Duration = Duration::from_millis(150);
/// Longest pause between scroll steps.
const SCROLL_PAUSE_MAX: Duration = Duration::from_millis(900);

/// A persona's derived browsing rhythm for one page: how long it lingers, how
/// far it scrolls, and how it paces the scroll. Deterministic for a given
/// persona id + seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowsingCadence {
    /// How long the session dwells on the page before moving on.
    pub dwell: Duration,
    /// Number of discrete scroll steps performed.
    pub scroll_steps: u32,
    /// Pixels advanced per scroll step.
    pub scroll_step_px: i64,
    /// Pause between consecutive scroll steps.
    pub scroll_pause: Duration,
}

impl BrowsingCadence {
    /// Derive a cadence for `persona`, mixed with `seed` so a caller can vary
    /// runs (the household scheduler's per-action seed) while keeping a fixed
    /// (persona, seed) pair reproducible.
    ///
    /// The persona id is hashed into the RNG seed so DIFFERENT personas get
    /// distinct rhythms even at the same `seed`. When C5 adds explicit
    /// browsing-style fields, fold them in here instead of (or alongside) the
    /// id-derived defaults.
    pub fn for_persona(persona: &SyntheticPersona, seed: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed ^ stable_hash(&persona.id));
        Self::sample(&mut rng)
    }

    /// Sample a cadence from an already-seeded RNG (the deterministic core, so
    /// tests can drive it with a known seed without a persona).
    fn sample(rng: &mut StdRng) -> Self {
        let dwell_ms =
            rng.random_range(DWELL_MIN.as_millis() as u64..=DWELL_MAX.as_millis() as u64);
        let scroll_steps = rng.random_range(SCROLL_STEPS_MIN..=SCROLL_STEPS_MAX);
        let pause_ms = rng.random_range(
            SCROLL_PAUSE_MIN.as_millis() as u64..=SCROLL_PAUSE_MAX.as_millis() as u64,
        );
        Self {
            dwell: Duration::from_millis(dwell_ms),
            scroll_steps,
            scroll_step_px: SCROLL_STEP_PX,
            scroll_pause: Duration::from_millis(pause_ms),
        }
    }

    /// Total scroll depth in pixels this cadence will traverse.
    pub fn scroll_depth_px(&self) -> i64 {
        self.scroll_step_px * i64::from(self.scroll_steps)
    }
}

/// A small, stable (build-independent) hash of a string into a `u64`, used to
/// turn a persona id into an RNG seed. FNV-1a: deterministic across runs and
/// platforms (unlike `DefaultHasher`, whose output is not contractually
/// stable), so a persona's rhythm is reproducible everywhere.
fn stable_hash(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn persona(id: &str) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            "Cadence Test".to_string(),
            AgeRange::AGE_25_34.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            vec![
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::GAMING.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    #[test]
    fn cadence_is_deterministic_for_fixed_persona_and_seed() {
        let p = persona("11111111-1111-4111-8111-111111111111");
        let a = BrowsingCadence::for_persona(&p, 7);
        let b = BrowsingCadence::for_persona(&p, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn different_personas_get_different_rhythms() {
        // Overwhelmingly likely to differ; the id is folded into the seed.
        let a = BrowsingCadence::for_persona(&persona("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"), 0);
        let b = BrowsingCadence::for_persona(&persona("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"), 0);
        assert_ne!(a, b);
    }

    #[test]
    fn cadence_stays_within_human_plausible_bounds() {
        for seed in 0..256u64 {
            let p = persona("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
            let c = BrowsingCadence::for_persona(&p, seed);
            assert!((DWELL_MIN..=DWELL_MAX).contains(&c.dwell));
            assert!((SCROLL_STEPS_MIN..=SCROLL_STEPS_MAX).contains(&c.scroll_steps));
            assert!((SCROLL_PAUSE_MIN..=SCROLL_PAUSE_MAX).contains(&c.scroll_pause));
            assert!(c.scroll_depth_px() > 0);
        }
    }

    #[test]
    fn stable_hash_is_deterministic() {
        assert_eq!(stable_hash("persona-x"), stable_hash("persona-x"));
        assert_ne!(stable_hash("persona-x"), stable_hash("persona-y"));
    }
}
