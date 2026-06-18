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

//! Shared test fixture for the end-to-end CLI tests: a temp store (db +
//! passphrase file) using the headless encrypted-key-file key source (NEVER the
//! OS keystore, to keep the tests hermetic), and helpers to invoke the compiled
//! `fauxx-cli` binary with the matching global flags.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use tempfile::TempDir;

/// A temp store layout and a helper to invoke the binary with its store flags.
pub struct Fixture {
    _dir: TempDir,
    pub dir: PathBuf,
    pub db: PathBuf,
    pub passphrase_file: PathBuf,
}

impl Fixture {
    /// Build a fresh temp store fixture.
    pub fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let db = dir.path().join("fauxx.db");
        let passphrase_file = dir.path().join("pass.txt");
        std::fs::write(&passphrase_file, "cli-suite-test-passphrase\n")?;
        Ok(Self {
            dir: dir.path().to_path_buf(),
            _dir: dir,
            db,
            passphrase_file,
        })
    }

    /// Run `fauxx-cli <args...>` with this fixture's store flags prepended.
    pub fn run(&self, args: &[&str]) -> Result<Output> {
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

    /// Run `fauxx-cli <args...>` WITHOUT the global store flags (e.g. `serve`, which
    /// resolves its own store config from its config file).
    pub fn run_bare(&self, args: &[&str]) -> Result<Output> {
        let bin = env!("CARGO_BIN_EXE_fauxx-cli");
        Command::new(bin)
            .args(args)
            .output()
            .context("spawning fauxx-cli binary (bare)")
    }
}

/// The captured stdout as a string.
pub fn stdout(output: &Output) -> Result<String> {
    Ok(String::from_utf8(output.stdout.clone())?)
}

/// The captured stderr as a string.
pub fn stderr(output: &Output) -> Result<String> {
    Ok(String::from_utf8(output.stderr.clone())?)
}

/// The process exit code.
pub fn code(output: &Output) -> Result<i32> {
    output.status.code().context("process had no exit code")
}

/// Assert the command exited 0, surfacing stderr in the failure message.
pub fn assert_ok(output: &Output, what: &str) -> Result<()> {
    let code = code(output)?;
    if code != 0 {
        anyhow::bail!("{what} exited {code}; stderr: {}", stderr(output)?);
    }
    Ok(())
}
