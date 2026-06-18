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

//! End-to-end CLI tests: drive the compiled `fauxx-cli` binary against a temp store
//! that uses the headless encrypted-key-file key source (NEVER the OS keystore,
//! to keep the tests hermetic). Exercises add -> list -> show -> delete plus
//! status and error/exit-code behavior.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};
use tempfile::TempDir;

/// A temp store layout (db + passphrase file) and a helper to invoke the
/// binary with the matching global flags.
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
        std::fs::write(&passphrase_file, "integration-test-passphrase\n")?;
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
fn add_list_show_delete_round_trip() -> Result<()> {
    let fx = Fixture::new()?;

    // List starts empty.
    let out = fx.run(&["persona", "list"])?;
    assert_eq!(code(&out)?, 0, "list stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.contains("(no personas)"));

    // Add a valid persona with a fixed id so we can show/delete it.
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "CLI Test",
        "--age-range",
        "AGE_25_34",
        "--profession",
        "ENGINEER",
        "--region",
        "US_WEST",
        "--interests",
        "TECHNOLOGY,GAMING,SCIENCE",
    ])?;
    assert_eq!(code(&out)?, 0, "add stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.contains(id));

    // List now shows the summary line (id, name, region, interest count).
    let out = fx.run(&["persona", "list"])?;
    assert_eq!(code(&out)?, 0);
    let listing = stdout(&out)?;
    assert!(listing.contains(id), "listing: {listing}");
    assert!(listing.contains("CLI Test"));
    assert!(listing.contains("region=US_WEST"));
    assert!(listing.contains("interests=3"));

    // List --json round-trips and includes the camelCase wire keys.
    let out = fx.run(&["persona", "list", "--json"])?;
    assert_eq!(code(&out)?, 0);
    let json = stdout(&out)?;
    assert!(json.contains("\"ageRange\""));
    assert!(json.contains(id));

    // Show prints the single persona as JSON.
    let out = fx.run(&["persona", "show", id])?;
    assert_eq!(code(&out)?, 0, "show stderr: {}", stderr(&out)?);
    let shown = stdout(&out)?;
    assert!(shown.contains(id));
    assert!(shown.contains("\"name\": \"CLI Test\""));

    // Delete removes it.
    let out = fx.run(&["persona", "delete", id])?;
    assert_eq!(code(&out)?, 0, "delete stderr: {}", stderr(&out)?);
    assert!(stdout(&out)?.contains("deleted"));

    // List is empty again.
    let out = fx.run(&["persona", "list"])?;
    assert_eq!(code(&out)?, 0);
    assert!(stdout(&out)?.contains("(no personas)"));
    Ok(())
}

#[test]
fn show_missing_persona_exits_1() -> Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["persona", "show", "does-not-exist"])?;
    assert_eq!(code(&out)?, 1, "stdout: {}", stdout(&out)?);
    assert!(stderr(&out)?.contains("not found"));
    Ok(())
}

#[test]
fn delete_missing_persona_exits_1() -> Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["persona", "delete", "does-not-exist"])?;
    assert_eq!(code(&out)?, 1);
    assert!(stderr(&out)?.contains("not found"));
    Ok(())
}

#[test]
fn status_human_and_json() -> Result<()> {
    let fx = Fixture::new()?;

    let out = fx.run(&["status"])?;
    assert_eq!(code(&out)?, 0, "status stderr: {}", stderr(&out)?);
    let line = stdout(&out)?;
    assert!(line.contains("fauxx-core"));
    assert!(line.contains("store_attached=true"));

    let out = fx.run(&["status", "--json"])?;
    assert_eq!(code(&out)?, 0);
    let json = stdout(&out)?;
    assert!(json.contains("\"storeAttached\"") || json.contains("\"store_attached\""));
    assert!(json.contains("\"version\""));
    Ok(())
}

#[test]
fn add_from_json_stdin() -> Result<()> {
    let fx = Fixture::new()?;
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let persona_json = format!(
        r#"{{
            "id": "{id}",
            "name": "From Stdin",
            "ageRange": "AGE_35_44",
            "profession": "TEACHER",
            "region": "CANADA",
            "interests": ["ACADEMIC", "HISTORY", "SCIENCE"],
            "createdAt": 1700000000000,
            "activeUntil": 1700600000000
        }}"#
    );

    let bin = env!("CARGO_BIN_EXE_fauxx-cli");
    let mut child = Command::new(bin)
        .arg("--db")
        .arg(&fx.db)
        .arg("--passphrase-file")
        .arg(&fx.passphrase_file)
        .args(["persona", "add", "--from-json", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawning fauxx-cli for stdin add")?;
    {
        use std::io::Write as _;
        let stdin = child.stdin.as_mut().context("child stdin not piped")?;
        stdin.write_all(persona_json.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    assert_eq!(code(&out)?, 0, "add-from-json stderr: {}", stderr(&out)?);

    let out = fx.run(&["persona", "show", id])?;
    assert_eq!(code(&out)?, 0);
    assert!(stdout(&out)?.contains("From Stdin"));
    Ok(())
}

#[test]
fn add_invalid_persona_warns_but_succeeds() -> Result<()> {
    let fx = Fixture::new()?;
    let id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    // Unknown enum values + too few interests: validation must warn, not fail.
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "Bad Values",
        "--age-range",
        "AGE_99_PLUS",
        "--profession",
        "ASTRONAUT",
        "--region",
        "MARS",
        "--interests",
        "SPACE",
    ])?;
    assert_eq!(code(&out)?, 0, "should warn, not fail: {}", stderr(&out)?);
    let warnings = stderr(&out)?;
    assert!(warnings.contains("validation issue"), "stderr: {warnings}");

    // It was still persisted (lossless round-trip).
    let out = fx.run(&["persona", "show", id])?;
    assert_eq!(code(&out)?, 0);
    assert!(stdout(&out)?.contains("AGE_99_PLUS"));
    Ok(())
}

#[test]
fn missing_required_flags_is_usage_error() -> Result<()> {
    let fx = Fixture::new()?;
    // No --name etc. and no --from-json: clap rejects with exit code 2.
    let out = fx.run(&["persona", "add"])?;
    assert_eq!(code(&out)?, 2, "stdout: {}", stdout(&out)?);
    Ok(())
}

#[test]
fn help_succeeds() -> Result<()> {
    let bin = env!("CARGO_BIN_EXE_fauxx-cli");
    let out = Command::new(bin).arg("--help").output()?;
    assert_eq!(code(&out)?, 0);
    let help = stdout(&out)?;
    assert!(help.contains("status"));
    assert!(help.contains("persona"));
    assert!(help.contains("run"));
    Ok(())
}

/// Helper kept to assert the temp db file actually materialized (a quick guard
/// that the EncryptedFile key source opened a real store).
fn assert_db_exists(db: &Path) -> Result<()> {
    if !db.exists() {
        bail!("expected db file at {} to exist", db.display());
    }
    Ok(())
}

#[test]
fn store_file_is_created_on_first_use() -> Result<()> {
    let fx = Fixture::new()?;
    let out = fx.run(&["status"])?;
    assert_eq!(code(&out)?, 0, "status stderr: {}", stderr(&out)?);
    assert_db_exists(&fx.db)?;
    Ok(())
}
