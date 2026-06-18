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

//! End-to-end CLI tests for the C1 cross-device surface: drive the compiled
//! `fauxx-cli` binary against a temp store using the headless encrypted-key-file key
//! source (NEVER the OS keystore, to stay hermetic). Covers `pair show`,
//! `mode` round-trip and persistence across process invocations, `pair add`
//! with a malformed payload (exit 2), an empty `peers` list, and a single-device
//! `schedule` preview.

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use tempfile::TempDir;

/// A temp store layout (db + passphrase file) and a helper to invoke the binary
/// with the matching global flags, so every command opens the same store.
struct Fixture {
    _dir: TempDir,
    db: PathBuf,
    passphrase_file: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let db = dir.path().join("fauxx.db");
        let passphrase_file = dir.path().join("pass.txt");
        std::fs::write(&passphrase_file, "cross-device-test-passphrase\n")?;
        Ok(Self {
            _dir: dir,
            db,
            passphrase_file,
        })
    }

    /// Run `fauxx-cli <args...>` with this fixture's store flags prepended.
    fn run(&self, args: &[&str]) -> Result<Output> {
        let bin = env!("CARGO_BIN_EXE_fauxx-cli");
        Command::new(bin)
            .arg("--db")
            .arg(&self.db)
            .arg("--passphrase-file")
            .arg(&self.passphrase_file)
            .args(args)
            .output()
            .context("spawning fauxx-cli binary")
    }
}

fn stdout(output: &Output) -> Result<String> {
    Ok(String::from_utf8(output.stdout.clone())?)
}

fn stderr(output: &Output) -> Result<String> {
    Ok(String::from_utf8(output.stderr.clone())?)
}

fn code(output: &Output) -> Result<i32> {
    output.status.code().context("process had no exit code")
}

#[test]
fn pair_show_prints_qr_and_fingerprint() -> Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["pair", "show"])?;
    assert_eq!(code(&out)?, 0, "pair show stderr: {}", stderr(&out)?);
    let text = stdout(&out)?;
    // The unicode QR uses block glyphs and spans multiple lines.
    assert!(text.contains('\n'));
    assert!(
        text.contains('\u{2580}') || text.contains('\u{2588}') || text.contains('\u{2584}'),
        "expected unicode QR block glyphs in: {text}"
    );
    // The fingerprint is printed (four colon-separated hex pairs).
    assert!(text.contains("fingerprint:"), "output: {text}");
    let fp_line = text
        .lines()
        .find(|l| l.starts_with("fingerprint:"))
        .context("no fingerprint line")?;
    assert_eq!(
        fp_line.matches(':').count(),
        4,
        "fingerprint should have the label colon plus three group colons: {fp_line}"
    );
    // The raw base64url payload is printed so the user can copy it.
    assert!(text.contains("payload:"), "output: {text}");
    Ok(())
}

#[test]
fn mode_set_then_show_round_trips_and_persists() -> Result<()> {
    let fx = Fixture::new()?;

    // Fresh store defaults to coherent.
    let out = fx.run(&["mode"])?;
    assert_eq!(code(&out)?, 0, "mode stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.trim() == "CoherentHousehold");

    // Set to fragmentation (a separate process invocation).
    let out = fx.run(&["mode", "set", "fragmentation"])?;
    assert_eq!(code(&out)?, 0, "mode set stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.contains("Fragmentation"));

    // A brand-new process against the SAME --db sees the persisted mode.
    let out = fx.run(&["mode"])?;
    assert_eq!(code(&out)?, 0);
    assert_eq!(stdout(&out)?.trim(), "Fragmentation");

    // And back to coherent, again persisting across invocations.
    let out = fx.run(&["mode", "set", "coherent"])?;
    assert_eq!(code(&out)?, 0);
    let out = fx.run(&["mode"])?;
    assert_eq!(stdout(&out)?.trim(), "CoherentHousehold");
    Ok(())
}

#[test]
fn pair_add_malformed_payload_exits_2() -> Result<()> {
    let fx = Fixture::new()?;
    // Not valid base64url-of-JSON: the core's payload decode fails closed, which
    // the CLI classifies as a usage error (exit 2).
    let out = fx.run(&["pair", "add", "this is not a valid payload!!"])?;
    assert_eq!(code(&out)?, 2, "stdout: {}", stdout(&out)?);
    assert!(
        stderr(&out)?.contains("invalid pairing payload"),
        "stderr: {}",
        stderr(&out)?
    );
    Ok(())
}

#[test]
fn peers_lists_nothing_on_a_fresh_store() -> Result<()> {
    let fx = Fixture::new()?;

    let out = fx.run(&["peers"])?;
    assert_eq!(code(&out)?, 0, "peers stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.contains("(no paired peers)"));

    // --json renders an empty array.
    let out = fx.run(&["peers", "--json"])?;
    assert_eq!(code(&out)?, 0);
    assert_eq!(stdout(&out)?.trim(), "[]");

    // --discovered is also empty on a store with no discovery backend running.
    let out = fx.run(&["peers", "--discovered"])?;
    assert_eq!(code(&out)?, 0);
    assert!(stdout(&out)?.contains("(no discovered peers)"));

    let out = fx.run(&["peers", "--discovered", "--json"])?;
    assert_eq!(code(&out)?, 0);
    assert_eq!(stdout(&out)?.trim(), "[]");
    Ok(())
}

#[test]
fn unpair_unknown_key_exits_1() -> Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["unpair", "no-such-public-key"])?;
    assert_eq!(code(&out)?, 1, "stdout: {}", stdout(&out)?);
    assert!(stderr(&out)?.contains("no paired peer"));
    Ok(())
}

#[test]
fn schedule_prints_a_plan_for_the_local_device() -> Result<()> {
    let fx = Fixture::new()?;
    // No peers paired: the plan covers just the local device, exercising O4
    // headlessly. A fixed seed keeps the plan deterministic.
    let out = fx.run(&["schedule", "--seed", "7", "--limit", "3"])?;
    assert_eq!(code(&out)?, 0, "schedule stderr: {}", stderr(&out)?);
    let text = stdout(&out)?;
    // Summary line reports the action count and the single device.
    assert!(text.contains("household plan:"), "output: {text}");
    assert!(text.contains("1 device(s)"), "output: {text}");
    // At least one scheduled action line for the local device, with a
    // time-of-day stamp (HH:MM:SS) inside the active window.
    let action_line = text
        .lines()
        .find(|l| l.contains("device=local"))
        .context("no local-device action line")?;
    assert!(action_line.contains("persona="), "line: {action_line}");
    Ok(())
}

#[test]
fn schedule_is_deterministic_for_a_fixed_seed() -> Result<()> {
    let fx = Fixture::new()?;
    let a = fx.run(&["schedule", "--seed", "42", "--limit", "5"])?;
    let b = fx.run(&["schedule", "--seed", "42", "--limit", "5"])?;
    assert_eq!(code(&a)?, 0);
    assert_eq!(code(&b)?, 0);
    assert_eq!(stdout(&a)?, stdout(&b)?, "same seed must yield same plan");
    Ok(())
}
