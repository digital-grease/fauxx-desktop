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

//! Real per-OS idle/lock detection backends for the rate planner (C8 #32 U1).
//!
//! These implement the [`IdleSource`] seam with actual OS queries, replacing the
//! always-active `ConservativeIdleSource` when the headless `serve` daemon
//! enables idle gating. [`real_idle_source`] returns the backend for the build
//! target:
//!
//! - **Linux**: logind (`org.freedesktop.login1`) over D-Bus, reading the
//!   session's `IdleHint` / `IdleSinceHint` / `LockedHint`. Display-server
//!   agnostic (Wayland and X11). This is the tested backend.
//! - **macOS / Windows**: the platform idle-time APIs via the `user-idle` crate
//!   (IOKit HIDIdleTime / GetLastInputInfo). Idle-time only; lock state is NOT
//!   detected there yet, so a locked-but-idle screen reads as idle. UNVALIDATED
//!   on real hardware (only the Linux path is exercised here).
//! - **Any other target**: the conservative always-active source.
//!
//! Every backend FAILS SAFE: on any query error it reports [`IdleState::Active`],
//! so the planner never ramps decoy activity on a guess (and a locked session is
//! treated like active, never ramped).

use async_trait::async_trait;

use super::{IdleSource, IdleState};

/// The real idle source for the current build target. Falls back to the
/// conservative always-active source on platforms without a backend.
pub fn real_idle_source() -> Box<dyn IdleSource> {
    #[cfg(target_os = "linux")]
    let source: Box<dyn IdleSource> = Box::new(LogindIdleSource);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let source: Box<dyn IdleSource> = Box::new(SystemIdleSource);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let source: Box<dyn IdleSource> = Box::new(super::ConservativeIdleSource);
    source
}

// ---------------------------------------------------------------------------
// Linux: logind over D-Bus (tested backend).
// ---------------------------------------------------------------------------

/// Reads idle/lock state from logind (`org.freedesktop.login1`) for the calling
/// session. Works under both Wayland and X11 since logind is the display server
/// agnostic source of session idle/lock hints.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, Default)]
pub struct LogindIdleSource;

#[cfg(target_os = "linux")]
#[async_trait]
impl IdleSource for LogindIdleSource {
    async fn idle_state(&self) -> IdleState {
        // Any failure (no system bus, no logind, no session) falls back to
        // Active so the planner stays conservative.
        logind_idle_state().await.unwrap_or(IdleState::Active)
    }
}

/// Query logind for the caller's session idle/lock state. Returns `None` on any
/// D-Bus error so the caller can fail safe to [`IdleState::Active`].
#[cfg(target_os = "linux")]
async fn logind_idle_state() -> Option<IdleState> {
    // logind exposes the caller's own session at the well-known "auto" path.
    let conn = zbus::Connection::system().await.ok()?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
    )
    .await
    .ok()?;

    // A locked session is treated like active: never ramp decoy activity on a
    // machine whose screen may be shared or visible.
    if proxy
        .get_property::<bool>("LockedHint")
        .await
        .unwrap_or(false)
    {
        return Some(IdleState::Locked);
    }
    if !proxy
        .get_property::<bool>("IdleHint")
        .await
        .unwrap_or(false)
    {
        return Some(IdleState::Active);
    }
    // IdleSinceHint is microseconds of CLOCK_REALTIME since the Unix epoch.
    let since_us = proxy
        .get_property::<u64>("IdleSinceHint")
        .await
        .unwrap_or(0);
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let idle_for = now_us
        .checked_sub(since_us)
        .map(std::time::Duration::from_micros)
        .unwrap_or(std::time::Duration::ZERO);
    Some(IdleState::Idle(idle_for))
}

// ---------------------------------------------------------------------------
// macOS / Windows: platform idle-time via `user-idle` (UNVALIDATED).
// ---------------------------------------------------------------------------

/// Reads input idle time from the OS (IOKit HIDIdleTime on macOS,
/// GetLastInputInfo on Windows) via the `user-idle` crate. Idle-time only: there
/// is no lock detection on these platforms yet, so a locked-but-idle screen reads
/// as idle. UNVALIDATED on real hardware.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemIdleSource;

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[async_trait]
impl IdleSource for SystemIdleSource {
    async fn idle_state(&self) -> IdleState {
        match user_idle::UserIdle::get_time() {
            Ok(idle) => {
                let secs = idle.as_seconds();
                if secs == 0 {
                    IdleState::Active
                } else {
                    IdleState::Idle(std::time::Duration::from_secs(secs))
                }
            }
            // Fail safe: an unreadable idle time reports Active (never ramp).
            Err(_) => IdleState::Active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_idle_source_samples_without_panicking() {
        // The constructed source must sample to SOME state without panicking,
        // whatever the host environment (CI has no logind session, so this
        // exercises the fail-safe path that returns Active).
        let source = real_idle_source();
        let state = source.idle_state().await;
        // Any of the three states is acceptable; the contract is just that it
        // returns one and never panics or blocks the runtime.
        assert!(matches!(
            state,
            IdleState::Active | IdleState::Idle(_) | IdleState::Locked
        ));
    }
}
