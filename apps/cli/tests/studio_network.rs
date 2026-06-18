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

//! End-to-end CLI tests for the C5 persona-pack surface (`pack`) and the C7
//! per-persona network surface (`egress`, `dns`). Hermetic: a temp store with
//! the headless encrypted-key-file key source.

mod common;
use common::Fixture;

const PERSONA: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

fn add_persona(fx: &Fixture, id: &str) -> anyhow::Result<()> {
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "Studio Net",
        "--age-range",
        "AGE_35_44",
        "--profession",
        "TEACHER",
        "--region",
        "CANADA",
        "--interests",
        "ACADEMIC,HISTORY,SCIENCE",
    ])?;
    common::assert_ok(&out, "persona add")?;
    Ok(())
}

#[test]
fn pack_export_import_list_round_trip() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    let pack_path = fx.dir.join("export.fauxxpack");

    // Export the persona to a signed pack file.
    let pack_str = pack_path.to_string_lossy().to_string();
    let out = fx.run(&["pack", "export", "--out", &pack_str, "--persona", PERSONA])?;
    common::assert_ok(&out, "pack export")?;
    assert!(pack_path.exists(), "pack file should exist");

    // Import the pack into a SECOND, distinct store (verify-before-write path).
    let fx2 = Fixture::new()?;
    let out = fx2.run(&["pack", "import", &pack_str])?;
    common::assert_ok(&out, "pack import")?;
    assert!(common::stdout(&out)?.contains("imported 1 persona"));

    // The imported persona is now present in the second store.
    let out = fx2.run(&["persona", "show", PERSONA])?;
    common::assert_ok(&out, "persona show after import")?;
    assert!(common::stdout(&out)?.contains("Studio Net"));

    // The installed-pack ledger lists the imported pack.
    let out = fx2.run(&["pack", "list", "--json"])?;
    common::assert_ok(&out, "pack list")?;
    assert!(common::stdout(&out)?.contains("signerPublicKey"));
    Ok(())
}

#[test]
fn pack_import_tampered_exits_1() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let bad = fx.dir.join("bad.fauxxpack");
    std::fs::write(&bad, b"{ not a valid signed pack }")?;
    let bad_str = bad.to_string_lossy().to_string();
    let out = fx.run(&["pack", "import", &bad_str])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn egress_set_get_clear() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Default egress is direct (the exit indicator reports it, never paused).
    let out = fx.run(&["egress", "get", PERSONA])?;
    common::assert_ok(&out, "egress get default")?;
    assert!(common::stdout(&out)?.contains("paused=false"));

    // Bind a Tor egress, then read it back as JSON.
    let out = fx.run(&["egress", "set", PERSONA, "tor"])?;
    common::assert_ok(&out, "egress set tor")?;
    let out = fx.run(&["egress", "get", PERSONA, "--json"])?;
    common::assert_ok(&out, "egress get json")?;
    assert!(common::stdout(&out)?.contains("Tor"));

    // Clear it back to direct.
    let out = fx.run(&["egress", "clear", PERSONA])?;
    common::assert_ok(&out, "egress clear")?;
    assert!(common::stdout(&out)?.contains("now direct"));
    Ok(())
}

#[test]
fn egress_http_requires_host_and_port() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    // An http egress with no --host is a fail-closed runtime error (exit 1).
    let out = fx.run(&["egress", "set", PERSONA, "http"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn dns_set_get_with_observer_note() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Default is the system resolver; the observer note is always surfaced.
    let out = fx.run(&["dns", "get", PERSONA])?;
    common::assert_ok(&out, "dns get default")?;
    assert!(common::stdout(&out)?.contains("system default"));

    // Bind a DoH strategy with an explicit resolver.
    let out = fx.run(&[
        "dns",
        "set",
        PERSONA,
        "doh",
        "--resolver",
        "https://dns.example/dns-query",
    ])?;
    common::assert_ok(&out, "dns set doh")?;
    assert!(common::stdout(&out)?.contains("dns.example"));

    // Read it back as JSON.
    let out = fx.run(&["dns", "get", PERSONA, "--json"])?;
    common::assert_ok(&out, "dns get json")?;
    assert!(common::stdout(&out)?.contains("dns.example"));

    // Verify (#31) reports the resolver + the actual Chromium secure-DNS flags
    // the decoy launch applies.
    let out = fx.run(&["dns", "verify", PERSONA])?;
    common::assert_ok(&out, "dns verify")?;
    let report = common::stdout(&out)?;
    assert!(
        report.contains("dns.example"),
        "verify names the resolver: {report}"
    );
    assert!(
        report.contains("--dns-over-https") || report.to_lowercase().contains("secure-dns"),
        "verify shows the secure-DNS flags: {report}"
    );
    Ok(())
}

#[test]
fn dns_verify_system_default_reports_no_isolation() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    // A persona left on the system resolver: verify says so (no per-persona DNS).
    let out = fx.run(&["dns", "verify", PERSONA])?;
    common::assert_ok(&out, "dns verify default")?;
    assert!(common::stdout(&out)?.contains("system default"));
    Ok(())
}

#[test]
fn dns_doh_requires_resolver() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    let out = fx.run(&["dns", "set", PERSONA, "doh"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}
