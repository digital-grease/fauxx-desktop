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

//! Idle / lock-aware scheduling (C8 #32, U1).
//!
//! The headless core runs decoy browsing 24/7, so it must yield to the human:
//! run HEAVIER when the box is idle, and PAUSE (or throttle) the instant the
//! user is back or the session is locked. This module is the seam the rate
//! planner consumes to make that decision.
//!
//! ## State model
//!
//! [`IdleState`] is a three-way model:
//!
//! - [`IdleState::Active`]: the user is interacting now. Decoy activity is held
//!   to the configured active behavior (paused or a small throttle) so it never
//!   competes with real work for foreground focus or resources.
//! - [`IdleState::Idle`]: no input for the carried [`Duration`]. Once that
//!   duration crosses the configured threshold, decoy intensity SCALES UP.
//! - [`IdleState::Locked`]: the session is locked. Treated like active for
//!   gating (paused): a locked machine may be a shared/visible screen, and the
//!   conservative choice is not to ramp.
//!
//! ## Detection seam
//!
//! [`IdleSource`] is the injectable trait behind which OS idle/lock detection
//! sits. A [`StubIdleSource`] drives the unit tests across all three states.
//! The dep-free default concrete source is [`ConservativeIdleSource`], which
//! reports [`IdleState::Active`] whenever real per-OS detection is unavailable,
//! so the planner errs toward NOT over-running while the user might be active.
//!
//! ## Per-OS detection (documented gap, follow-up)
//!
//! Real per-OS idle/lock detection is a DELIBERATE follow-up, not built here:
//!
//! - Linux: `org.freedesktop.login1`'s `IdleHint` / `IdleSinceHint` over D-Bus,
//!   the Wayland `ext-idle-notify` protocol, or X11 `XScreenSaverQueryInfo`.
//! - Windows: `GetLastInputInfo` for idle time plus `WTSRegisterSessionNotification`
//!   (`WM_WTSSESSION_CHANGE`) for lock/unlock.
//! - macOS: `IOHIDSystem`'s `HIDIdleTime` plus `CGSessionCopyCurrentDictionary`
//!   (`kCGSessionOnConsoleKey`) for lock state.
//!
//! Each needs an OS-specific, optional dependency wired in a `target.'cfg(...)'`
//! block behind a feature, exactly as the keystore backends are. Until that
//! lands the conservative default keeps the contract sound: the planner is
//! correct (it just never ramps without a real idle signal). The trait is the
//! stable seam those backends slot into without changing the rate planner.

pub mod detect;
pub mod planner;

pub use detect::real_idle_source;
pub use planner::{ActiveBehavior, IdleScalingConfig, RateDecision, RatePlanner};

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The session activity state the rate planner gates on (C8 #32).
///
/// `Serialize`/`Deserialize` so the state can ride status payloads (e.g. the
/// U5 MQTT status sensor) without a separate wire type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "idle_ms")]
pub enum IdleState {
    /// The user is interacting now: hold decoy activity to the active behavior.
    Active,
    /// No input for the carried duration (serialized as milliseconds).
    #[serde(
        serialize_with = "ser_duration_ms",
        deserialize_with = "de_duration_ms"
    )]
    Idle(Duration),
    /// The session is locked: treated like active for gating (conservative).
    Locked,
}

impl IdleState {
    /// A convenience constructor for an idle state from whole seconds.
    pub fn idle_secs(secs: u64) -> Self {
        IdleState::Idle(Duration::from_secs(secs))
    }

    /// The idle duration this state represents (zero for `Active`/`Locked`).
    pub fn idle_duration(&self) -> Duration {
        match self {
            IdleState::Idle(d) => *d,
            IdleState::Active | IdleState::Locked => Duration::ZERO,
        }
    }

    /// Whether this state has crossed `threshold` of continuous idle time.
    /// Only [`IdleState::Idle`] can cross it; `Active`/`Locked` never do.
    pub fn is_idle_past(&self, threshold: Duration) -> bool {
        matches!(self, IdleState::Idle(d) if *d >= threshold)
    }
}

/// Serialize a [`Duration`] as whole milliseconds (saturating).
fn ser_duration_ms<S>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    s.serialize_u64(ms)
}

/// Deserialize a whole-millisecond count back into a [`Duration`].
fn de_duration_ms<'de, D>(d: D) -> std::result::Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ms = u64::deserialize(d)?;
    Ok(Duration::from_millis(ms))
}

/// The injectable idle/lock detection seam (C8 #32).
///
/// Real per-OS backends and the test [`StubIdleSource`] implement this. It is
/// object-safe (`async_trait`, `Send + Sync`) so the rate planner can hold a
/// `Box<dyn IdleSource>` and the GUI/headless builds share one wiring.
#[async_trait]
pub trait IdleSource: Send + Sync {
    /// Sample the current session activity state. Implementations must NEVER
    /// block the runtime or panic; on any detection failure they report the
    /// conservative [`IdleState::Active`] so the planner does not over-run.
    async fn idle_state(&self) -> IdleState;
}

/// The dep-free default concrete source: reports [`IdleState::Active`] always.
///
/// Used wherever real per-OS detection is not yet wired (every platform, until
/// the follow-up lands). Reporting `Active` is the conservative choice: the
/// rate planner holds decoy activity to the active behavior and never ramps on
/// a machine that might be in use. It makes no syscall and cannot fail.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConservativeIdleSource;

#[async_trait]
impl IdleSource for ConservativeIdleSource {
    async fn idle_state(&self) -> IdleState {
        IdleState::Active
    }
}

/// A test idle source whose state is set explicitly and read back on each
/// sample (C8 #32). Lets the unit tests drive the planner across `Active`,
/// `Idle(>threshold)`, and `Locked` deterministically.
#[derive(Debug, Clone)]
pub struct StubIdleSource {
    state: std::sync::Arc<std::sync::Mutex<IdleState>>,
}

impl StubIdleSource {
    /// A stub fixed at the given initial state.
    pub fn new(initial: IdleState) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(initial)),
        }
    }

    /// Overwrite the state the next [`IdleSource::idle_state`] call returns.
    /// A poisoned lock is ignored (the previous state stands); this is a test
    /// helper and never panics in non-test callers.
    pub fn set(&self, state: IdleState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = state;
        }
    }
}

impl Default for StubIdleSource {
    fn default() -> Self {
        Self::new(IdleState::Active)
    }
}

#[async_trait]
impl IdleSource for StubIdleSource {
    async fn idle_state(&self) -> IdleState {
        // A poisoned lock falls back to the conservative Active, never panics.
        self.state.lock().map(|g| *g).unwrap_or(IdleState::Active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_state_serializes_idle_as_milliseconds() -> crate::error::Result<()> {
        let json = serde_json::to_string(&IdleState::idle_secs(90))?;
        // 90s -> 90_000ms, tagged form.
        assert!(json.contains("idle"), "json was {json}");
        assert!(json.contains("90000"), "json was {json}");
        let back: IdleState = serde_json::from_str(&json)?;
        assert_eq!(back, IdleState::idle_secs(90));
        Ok(())
    }

    #[test]
    fn active_and_locked_round_trip() -> crate::error::Result<()> {
        for state in [IdleState::Active, IdleState::Locked] {
            let json = serde_json::to_string(&state)?;
            let back: IdleState = serde_json::from_str(&json)?;
            assert_eq!(back, state);
        }
        Ok(())
    }

    #[test]
    fn is_idle_past_only_for_idle_over_threshold() {
        let threshold = Duration::from_secs(60);
        assert!(IdleState::idle_secs(61).is_idle_past(threshold));
        assert!(IdleState::idle_secs(60).is_idle_past(threshold));
        assert!(!IdleState::idle_secs(59).is_idle_past(threshold));
        assert!(!IdleState::Active.is_idle_past(threshold));
        assert!(!IdleState::Locked.is_idle_past(threshold));
    }

    #[tokio::test]
    async fn conservative_source_is_always_active() {
        let src = ConservativeIdleSource;
        assert_eq!(src.idle_state().await, IdleState::Active);
    }

    #[tokio::test]
    async fn stub_source_reports_then_updates() {
        let stub = StubIdleSource::new(IdleState::Active);
        assert_eq!(stub.idle_state().await, IdleState::Active);
        stub.set(IdleState::idle_secs(120));
        assert_eq!(stub.idle_state().await, IdleState::idle_secs(120));
        stub.set(IdleState::Locked);
        assert_eq!(stub.idle_state().await, IdleState::Locked);
    }
}
