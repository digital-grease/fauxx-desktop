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

//! `fauxx-cli check-update`: the headless half of the user-initiated update
//! check.
//!
//! Deliberately its own command rather than a flag on `status`, so that nothing
//! a user runs routinely (or from a cron job, per the C8 #35 homelab mode) ever
//! makes a network request they did not ask for. See `fauxx_core::update` for
//! the disclosure rules this upholds.
//!
//! Opens no store: the check needs no persona, no key, and no database.

use fauxx_core::{UpdateStatus, RELEASES_URL};

/// Run the check and print the result.
pub async fn run(json: bool) -> anyhow::Result<()> {
    let check = fauxx_core::check_for_update().await?;

    if json {
        // Hand-built rather than derived: the shape is a CLI contract, and
        // deriving Serialize on the core type would let a field added for the
        // GUI silently become part of it.
        let status = match check.status {
            UpdateStatus::UpToDate => "up-to-date",
            UpdateStatus::UpdateAvailable => "update-available",
            UpdateStatus::Newer => "newer-than-release",
        };
        println!(
            "{}",
            serde_json::json!({
                "current": check.current,
                "latest": check.latest,
                "status": status,
                "releaseUrl": check.release_url,
            })
        );
        return Ok(());
    }

    println!("{}", check.summary());
    if check.status == UpdateStatus::UpdateAvailable {
        println!("Download: {}", check.release_url);
        println!(
            "Fauxx does not update itself. On Linux the AppImage carries update \
             information, so AppImageUpdate or AppImageLauncher can update it in place."
        );
    } else if check.release_url != RELEASES_URL {
        println!("Release notes: {}", check.release_url);
    }
    Ok(())
}
