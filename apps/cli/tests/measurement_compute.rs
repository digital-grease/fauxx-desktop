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

//! End-to-end CLI tests for the C4 measurement surface (`export`, `ab`,
//! `drift`) and the C6 heavy-compute surface (`generate`, `mint`). Hermetic: a
//! temp store with the headless encrypted-key-file key source.

mod common;
use common::Fixture;

const PERSONA: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";

fn add_persona(fx: &Fixture, id: &str) -> anyhow::Result<()> {
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "Measure Test",
        "--age-range",
        "AGE_25_34",
        "--profession",
        "ENGINEER",
        "--region",
        "US_WEST",
        "--interests",
        "TECHNOLOGY,SCIENCE,GAMING",
    ])?;
    common::assert_ok(&out, "persona add")?;
    Ok(())
}

#[test]
fn drift_prints_per_platform() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    // No read-backs yet, so the platforms report no data (no panic).
    let out = fx.run(&["drift", PERSONA])?;
    common::assert_ok(&out, "drift")?;
    let printed = common::stdout(&out)?;
    assert!(printed.contains("Google"));
    assert!(printed.contains("Meta"));

    let out = fx.run(&["drift", PERSONA, "--json"])?;
    common::assert_ok(&out, "drift --json")?;
    assert!(common::stdout(&out)?.contains("platform"));
    Ok(())
}

#[test]
fn export_writes_each_format() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    for (fmt, ext) in [("json", "json"), ("csv", "csv"), ("pdf", "pdf")] {
        let out_path = fx.dir.join(format!("snapshot.{ext}"));
        let out_str = out_path.to_string_lossy().to_string();
        let out = fx.run(&["export", PERSONA, "--out", &out_str, "--format", fmt])?;
        common::assert_ok(&out, &format!("export {fmt}"))?;
        assert!(out_path.exists(), "{fmt} export file should exist");
        let bytes = std::fs::read(&out_path)?;
        assert!(!bytes.is_empty(), "{fmt} export should be non-empty");
    }
    Ok(())
}

#[test]
fn ab_define_list_compare() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    let out = fx.run(&["ab", "define", "Treated A", PERSONA, "--arm", "treated"])?;
    common::assert_ok(&out, "ab define treated")?;
    let out = fx.run(&["ab", "define", "Control", PERSONA, "--arm", "control"])?;
    common::assert_ok(&out, "ab define control")?;

    let out = fx.run(&["ab", "list"])?;
    common::assert_ok(&out, "ab list")?;
    let listing = common::stdout(&out)?;
    assert!(listing.contains("Treated A"));
    assert!(listing.contains("Control"));

    // Compare runs even with no drift data (degenerate cohorts, no panic).
    let out = fx.run(&["ab", "compare", PERSONA, "--json"])?;
    common::assert_ok(&out, "ab compare")?;
    assert!(
        common::stdout(&out)?.contains("effectSize")
            || common::stdout(&out)?.contains("effect_size")
    );
    Ok(())
}

#[test]
fn generate_signs_artifacts() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    // No --push, so no peers needed; both artifacts are produced and signed.
    let out = fx.run(&["generate", PERSONA, "--seed", "7"])?;
    common::assert_ok(&out, "generate")?;
    let printed = common::stdout(&out)?;
    assert!(printed.contains("weight-map"));
    assert!(printed.contains("query-plan"));
    Ok(())
}

#[test]
fn mint_writes_signed_pack() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    // mint does not need a pre-existing persona; it draws from the PUMS seed.
    let out_path = fx.dir.join("minted.fauxxpack");
    let out_str = out_path.to_string_lossy().to_string();
    let out = fx.run(&["mint", "3", "--out", &out_str, "--seed", "11"])?;
    common::assert_ok(&out, "mint")?;
    assert!(out_path.exists(), "minted pack should exist");

    // The minted pack imports cleanly into a fresh store (verify-before-write).
    let fx2 = Fixture::new()?;
    let out = fx2.run(&["pack", "import", &out_str])?;
    common::assert_ok(&out, "import minted pack")?;
    assert!(common::stdout(&out)?.contains("imported 3 persona"));
    Ok(())
}
