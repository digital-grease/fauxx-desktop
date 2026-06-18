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

//! End-to-end CLI tests for the C8 orchestration surface: goal-driven
//! `campaign` lifecycle and the `serve` homelab mode. Hermetic: a temp store
//! with the headless encrypted-key-file key source, and the serve loop is
//! bounded with `--max-ticks` so the test does not run forever.

mod common;
use common::Fixture;

const PERSONA: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const CAMPAIGN: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

fn add_persona(fx: &Fixture, id: &str) -> anyhow::Result<()> {
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "Campaign Test",
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
fn campaign_create_start_tick_pause_adjust() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Create a campaign aiming to keep the TECHNOLOGY segment drift at most 0.5.
    let out = fx.run(&[
        "campaign",
        "create",
        "Cap TECH",
        PERSONA,
        "TECHNOLOGY",
        "--id",
        CAMPAIGN,
        "--comparator",
        "at-most",
        "--threshold",
        "0.5",
    ])?;
    common::assert_ok(&out, "campaign create")?;
    assert!(common::stdout(&out)?.contains("created campaign"));

    // List shows it (planned).
    let out = fx.run(&["campaign", "list"])?;
    common::assert_ok(&out, "campaign list")?;
    assert!(common::stdout(&out)?.contains("planned"));

    // Start it -> running.
    let out = fx.run(&["campaign", "start", CAMPAIGN])?;
    common::assert_ok(&out, "campaign start")?;
    assert!(common::stdout(&out)?.contains("running"));

    // Tick it once (no metric data yet, so the directive holds steady or idles).
    let out = fx.run(&["campaign", "tick", CAMPAIGN, "--json"])?;
    common::assert_ok(&out, "campaign tick")?;
    assert!(common::stdout(&out)?.contains("targetSegment"));

    // Adjust the threshold, then pause.
    let out = fx.run(&["campaign", "adjust", CAMPAIGN, "0.25"])?;
    common::assert_ok(&out, "campaign adjust")?;
    assert!(common::stdout(&out)?.contains("0.25"));

    let out = fx.run(&["campaign", "pause", CAMPAIGN])?;
    common::assert_ok(&out, "campaign pause")?;
    assert!(common::stdout(&out)?.contains("paused"));
    Ok(())
}

#[test]
fn campaign_create_nonfinite_threshold_exits_1() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    let out = fx.run(&[
        "campaign",
        "create",
        "Bad",
        PERSONA,
        "TECHNOLOGY",
        "--comparator",
        "at-least",
        "--threshold",
        "nan",
    ])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn campaign_start_unknown_exits_1() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["campaign", "start", "no-such-campaign"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

/// Write a serve config pointing at the fixture's store + passphrase file.
fn write_serve_config(fx: &Fixture, mqtt_enabled: bool) -> anyhow::Result<std::path::PathBuf> {
    let cfg_path = fx.dir.join("serve.json");
    let mqtt = if mqtt_enabled {
        r#", "mqtt": { "enabled": true, "host": "127.0.0.1" }"#
    } else {
        ""
    };
    let json = format!(
        r#"{{ "dbPath": {db}, "passphraseFile": {pass}, "tickIntervalSecs": 1 {mqtt} }}"#,
        db = serde_json::to_string(&fx.db.to_string_lossy())?,
        pass = serde_json::to_string(&fx.passphrase_file.to_string_lossy())?,
    );
    std::fs::write(&cfg_path, json)?;
    Ok(cfg_path)
}

#[test]
fn serve_check_prints_effective_config() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let cfg = write_serve_config(&fx, false)?;
    let cfg_str = cfg.to_string_lossy().to_string();
    // --check resolves and prints the config WITHOUT opening the store.
    let out = fx.run_bare(&["serve", "--config", &cfg_str, "--check"])?;
    common::assert_ok(&out, "serve --check")?;
    let printed = common::stdout(&out)?;
    assert!(printed.contains("tickIntervalSecs"));
    assert!(printed.contains("passphraseFile"));
    Ok(())
}

#[test]
fn serve_opens_store_and_runs_bounded_loop() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Create + start a campaign so the serve loop has something to resume/tick.
    let out = fx.run(&[
        "campaign",
        "create",
        "Resume Me",
        PERSONA,
        "TECHNOLOGY",
        "--id",
        CAMPAIGN,
        "--comparator",
        "at-most",
        "--threshold",
        "0.5",
    ])?;
    common::assert_ok(&out, "campaign create")?;
    let out = fx.run(&["campaign", "start", CAMPAIGN])?;
    common::assert_ok(&out, "campaign start")?;

    let cfg = write_serve_config(&fx, false)?;
    let cfg_str = cfg.to_string_lossy().to_string();

    // Run the bounded loop (a couple of ticks) and confirm it exits cleanly.
    let out = fx.run_bare(&["serve", "--config", &cfg_str, "--max-ticks", "2"])?;
    common::assert_ok(&out, "serve --max-ticks")?;

    // AC4 (#35): serve RESUMED the persisted Running campaign and recovered
    // without manual intervention - it is still present and Running after the
    // bounded run, not dropped or stranded by the restart.
    let out = fx.run(&["campaign", "list", "--json"])?;
    common::assert_ok(&out, "campaign list after serve")?;
    let listing = common::stdout(&out)?;
    assert!(
        listing.contains(CAMPAIGN),
        "the persisted campaign must survive a serve restart: {listing}"
    );
    assert!(
        listing.contains("\"running\""),
        "serve must resume the campaign as Running: {listing}"
    );
    Ok(())
}

#[test]
fn serve_with_lan_sync_binds_listener_and_exits_cleanly() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // A serve config with LAN sync enabled on a dedicated high port (avoids a
    // conflict with the default sync port / other instances).
    let cfg_path = fx.dir.join("serve-lan.json");
    let json = format!(
        r#"{{ "dbPath": {db}, "passphraseFile": {pass}, "tickIntervalSecs": 1, "lanSync": true, "syncPort": 46207 }}"#,
        db = serde_json::to_string(&fx.db.to_string_lossy())?,
        pass = serde_json::to_string(&fx.passphrase_file.to_string_lossy())?,
    );
    std::fs::write(&cfg_path, json)?;
    let cfg_str = cfg_path.to_string_lossy().to_string();

    // C1 #7: serve must bring up the inbound listener, tick, and shut it down
    // cleanly within the bounded run (it must not hang on the spawned listener).
    let out = fx.run_bare(&["serve", "--config", &cfg_str, "--max-ticks", "1"])?;
    common::assert_ok(&out, "serve --lan-sync --max-ticks")?;
    Ok(())
}

#[test]
fn serve_fails_closed_on_missing_config() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    let missing = fx.dir.join("nope.json").to_string_lossy().to_string();
    let out = fx.run_bare(&["serve", "--config", &missing])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    assert!(common::stderr(&out)?.contains("does not exist"));
    Ok(())
}

#[test]
fn serve_fails_closed_on_bad_passphrase() -> anyhow::Result<()> {
    // First create a store with the real passphrase.
    let fx = Fixture::new()?;
    let out = fx.run(&["status"])?;
    common::assert_ok(&out, "status creates store")?;

    // Now point serve at a DIFFERENT passphrase file: the store must fail to
    // open (fail closed), not start in a degraded state.
    let wrong_pass = fx.dir.join("wrong.txt");
    std::fs::write(&wrong_pass, "the-wrong-passphrase\n")?;
    let cfg_path = fx.dir.join("serve-wrong.json");
    let json = format!(
        r#"{{ "dbPath": {db}, "passphraseFile": {pass}, "tickIntervalSecs": 1 }}"#,
        db = serde_json::to_string(&fx.db.to_string_lossy())?,
        pass = serde_json::to_string(&wrong_pass.to_string_lossy())?,
    );
    std::fs::write(&cfg_path, json)?;
    let cfg_str = cfg_path.to_string_lossy().to_string();

    let out = fx.run_bare(&["serve", "--config", &cfg_str, "--max-ticks", "1"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn serve_mqtt_without_feature_fails_clearly() -> anyhow::Result<()> {
    // The default test binary is built WITHOUT the `mqtt` feature, so a config
    // (or flag) that enables the bridge must fail with a clear error, not a
    // silent no-op. (When built with --features mqtt this test is skipped.)
    if cfg!(feature = "mqtt") {
        return Ok(());
    }
    let fx = Fixture::new()?;
    let cfg = write_serve_config(&fx, true)?;
    let cfg_str = cfg.to_string_lossy().to_string();
    let out = fx.run_bare(&["serve", "--config", &cfg_str, "--max-ticks", "1"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    assert!(common::stderr(&out)?.contains("mqtt"));
    Ok(())
}
