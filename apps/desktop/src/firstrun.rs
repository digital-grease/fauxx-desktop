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

//! The C8 #34 U3 first-run-completed marker.
//!
//! The wizard is shown once, on first run, and a flag records that it has been
//! completed (or skipped). `fauxx-core` exposes no generic GUI-setting KV (its
//! store is reachable only through typed domain methods, and this is GUI-only
//! desktop state), so the marker lives in a tiny file under the OS config dir,
//! the same place per-OS app config conventionally lives. It is intentionally
//! NOT in the encrypted store: it carries no secret and must be readable before
//! the store is opened to decide whether to show the wizard at boot.
//!
//! Both reads and writes are best-effort and never panic: an unreadable config
//! dir simply means "treat as not-first-run" (so a permissions hiccup never
//! traps the user in the wizard), and a failed write is logged, not fatal.

/// The marker filename written under the app config directory.
const MARKER_FILE: &str = "first-run-complete";

/// The OS qualifier/org/app triple, matching the core's store path derivation
/// so the desktop config lands beside (not inside) the core data dir.
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";

/// The absolute path to the first-run marker file, or `None` if no OS config
/// directory can be determined (in which case first-run handling is skipped).
fn marker_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .map(|dirs| dirs.config_dir().join(MARKER_FILE))
}

/// Whether this is the first run (the wizard should be shown). `true` when the
/// marker is absent; `false` when it is present OR the config dir is
/// undeterminable (fail safe: never trap the user in the wizard).
pub fn is_first_run() -> bool {
    match marker_path() {
        Some(path) => !path.exists(),
        None => false,
    }
}

/// Record that the first-run wizard has been completed or skipped. Best-effort:
/// a failure is logged, not surfaced, so a write hiccup never blocks startup
/// (the cost is the wizard re-appearing next launch, which is harmless).
pub fn mark_complete() {
    let Some(path) = marker_path() else {
        tracing::warn!("no OS config dir; cannot persist first-run flag");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!("could not create config dir for first-run flag: {err}");
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, b"1") {
        tracing::warn!("could not write first-run flag: {err}");
    }
}
