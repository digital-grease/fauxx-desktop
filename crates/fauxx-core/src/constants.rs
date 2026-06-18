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

//! Frozen behavioral constants shared with the Android app.
//!
//! These figures parameterize the persona-following and topic-weighting model
//! that the scheduler, browser, and measurement subsystems use in later
//! milestones (C1+). They are defined here now so the contract is pinned and
//! reviewable in one place; C0 does not consume them yet. The values must stay
//! in lockstep with the Android implementation so cross-device behavior agrees.

use std::ops::RangeInclusive;

/// Fraction of a persona's browsing that follows the persona's interest
/// profile; the remainder is uniform-baseline noise to blur the fingerprint.
pub const PERSONA_FOLLOW_FRACTION: f64 = 0.85;

/// Topic weight applied to categories aligned with a persona's interests.
pub const ALIGNED_WEIGHT: f64 = 2.0;

/// Topic weight applied to categories that contradict a persona's interests.
pub const MISALIGNED_WEIGHT: f64 = 0.3;

/// Topic weight applied to categories neither aligned nor misaligned.
pub const NEUTRAL_WEIGHT: f64 = 1.0;

/// Baseline weight for the uniform-noise component of browsing.
pub const UNIFORM_BASELINE_WEIGHT: f64 = 0.6;

/// Minimum interest overlap above which two personas are considered to collide
/// (and one is regenerated) to keep the synthetic population diverse.
pub const OVERLAP_THRESHOLD: f64 = 0.60;

/// Baseline number of days a persona stays active before rotation.
pub const BASE_ROTATION_DAYS: u32 = 7;

/// Asymmetric jitter (in days) added to [`BASE_ROTATION_DAYS`], yielding an 8-
/// to-10-day rotation window. Jitter is added (never subtracted), so the
/// effective window is `BASE_ROTATION_DAYS + ROTATION_JITTER_DAYS`.
pub const ROTATION_JITTER_DAYS: RangeInclusive<u32> = 1..=3;
