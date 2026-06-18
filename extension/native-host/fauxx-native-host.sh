#!/usr/bin/env bash
# fauxx-desktop: Fauxx Desktop Companion
# Copyright (C) 2026 Digital Grease
# Licensed under the GNU Affero General Public License v3 or later.
# See the LICENSE file at the repository root.
#
# Native-messaging launcher wrapper for the Fauxx Core host.
#
# The browser launches the host by the absolute `path` in the native-messaging
# manifest, with NO control over arguments (it appends the calling extension's
# origin, and on Chromium a parent-window handle, as argv). The Fauxx host is the
# `native-host` SUBCOMMAND of the `fauxx-cli` binary, so this thin wrapper invokes
# it. The browser's appended argv is ignored by the subcommand (clap only reads
# the global store flags and the subcommand name), which is the documented and
# expected native-messaging behavior.
#
# Edit the two variables below for your install, then point the manifest `path`
# at THIS script (chmod +x it). Keep it on a path with no spaces.

set -euo pipefail

# Absolute path to the installed `fauxx-cli` binary (the apps/cli build).
FAUXX_BIN="/REPLACE/WITH/ABSOLUTE/PATH/TO/fauxx-cli"

# The encrypted store key source. The host runs UNATTENDED (the browser starts
# it with no TTY), so it must use the headless encrypted-key-file key source:
# a passphrase file unlocking an Argon2id-wrapped key. Do NOT use the OS keystore
# here unless your keystore unlocks without an interactive prompt.
#
# These map to the `fauxx-cli` global store flags (`--db`, `--passphrase-file`).
# Lock the passphrase file down to your user (chmod 600).
export FAUXX_DB="${FAUXX_DB:-/REPLACE/WITH/ABSOLUTE/PATH/TO/fauxx.db}"
export FAUXX_PASSPHRASE_FILE="${FAUXX_PASSPHRASE_FILE:-/REPLACE/WITH/ABSOLUTE/PATH/TO/fauxx.passphrase}"

# Exec so the host owns the stdio the browser handed us (no extra process layer).
exec "$FAUXX_BIN" native-host
