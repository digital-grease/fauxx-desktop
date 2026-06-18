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

//! `fauxx status`: open the resolved store and print core health.

use fauxx_core::Config;

/// Open the core and print its status, as a readable line or as JSON.
pub async fn run(config: Config, json: bool) -> anyhow::Result<()> {
    let core = fauxx_core::Core::open(config).await?;
    let status = core.status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "fauxx-core {} : {} (store_attached={}, persona_count={})",
            status.version, status.summary, status.store_attached, status.persona_count
        );
    }
    Ok(())
}
