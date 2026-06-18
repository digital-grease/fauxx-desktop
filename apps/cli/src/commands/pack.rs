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

//! `fauxx pack ...`: signed persona-pack import/export and the installed-pack
//! library (C5 #27). Thin shims over the core pack API; signing, verification,
//! and the verify-before-write import all live in the core.

use std::path::Path;

use anyhow::Context;
use fauxx_core::{Config, Core, PackProvenance};

use crate::cli::PackCommand;

/// Dispatch a `pack` subcommand against a freshly opened core.
pub async fn run(config: Config, command: PackCommand) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    match command {
        PackCommand::Export { out, persona, note } => export(&core, &out, &persona, note).await,
        PackCommand::Import { path } => import(&core, &path).await,
        PackCommand::List { json } => list(&core, json).await,
    }
}

/// Export selected personas to a signed pack file.
async fn export(
    core: &Core,
    out: &Path,
    persona_ids: &[String],
    note: Option<String>,
) -> anyhow::Result<()> {
    let mut provenance = PackProvenance::us("cli-export", "0", now_millis());
    provenance.note = note;
    let bytes = core.export_persona_pack(persona_ids, provenance).await?;
    std::fs::write(out, &bytes).with_context(|| format!("writing pack to {}", out.display()))?;
    println!(
        "exported {} persona(s) to {} ({} bytes)",
        persona_ids.len(),
        out.display(),
        bytes.len()
    );
    Ok(())
}

/// Import a signed pack file (verify, then land its personas).
async fn import(core: &Core, path: &Path) -> anyhow::Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading pack from {}", path.display()))?;
    let personas = core.import_persona_pack(&bytes).await?;
    println!("imported {} persona(s)", personas.len());
    for p in &personas {
        println!("  {}  {}", p.id, p.name);
    }
    Ok(())
}

/// List the installed persona packs (the library ledger).
async fn list(core: &Core, json: bool) -> anyhow::Result<()> {
    let packs = core.list_installed_packs().await?;
    if json {
        // `InstalledPack` is not itself serializable; its inner record is.
        let records: Vec<_> = packs.iter().map(|p| &p.record).collect();
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    if packs.is_empty() {
        println!("(no installed packs)");
        return Ok(());
    }
    for p in &packs {
        let r = &p.record;
        println!(
            "{}  source={}  signer={}  personas={}  imported_at={}",
            r.id,
            r.provenance.source_distribution,
            r.signer_public_key,
            r.persona_ids.len(),
            r.imported_at
        );
    }
    Ok(())
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
