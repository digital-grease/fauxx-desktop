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

//! `fauxx-cli pair ...`: show this device's pairing QR, or add a scanned peer.
//!
//! Thin shims over the core sync API. `show` renders the QR the phone scans
//! (unicode block form for the terminal) plus the fingerprint and the raw
//! base64url payload so the user can copy it. `add` completes pairing from a
//! scanned payload; a malformed or unsupported-version payload is a usage error
//! (exit 2), surfaced by returning a [`Failure::Usage`] directly.

use fauxx_core::{Config, Core, CoreError};

use crate::Failure;

/// Print this device's pairing QR (unicode), its fingerprint, and the raw
/// base64url payload string the QR encodes.
pub async fn show(config: Config) -> anyhow::Result<()> {
    let core = Core::open(config).await?;
    let qr = core.pairing_qr().await?;
    // The unicode block form is the scannable QR for a terminal; the fingerprint
    // lets the user eyeball it against the discovered peer; the raw payload is
    // the copyable fallback text (the exact QR contents).
    println!("{}", qr.unicode);
    println!("fingerprint: {}", qr.fingerprint);
    println!("payload: {}", qr.payload);
    // LAN-sync pairing is per-device and must be done BOTH ways (issue #42): the
    // phone scanning this QR only lets the phone authenticate US. For this device
    // to accept the phone's pushes, pair the phone here too.
    println!();
    println!(
        "Pairing must be done on BOTH devices. The phone scanning this code pairs THIS device on \
         the phone. To pair the phone back here, run: fauxx-cli pair add <phone-pairing-code>"
    );
    Ok(())
}

/// Complete pairing from a scanned payload string and print the recorded peer.
///
/// Owns its own exit-code split: a malformed or unsupported-version payload is
/// a usage error (exit 2); a store/transport failure is a runtime error
/// (exit 1). `complete_pairing` decodes the payload first, so a decode failure
/// surfaces as [`CoreError::Sync`], which we classify as a usage error.
pub async fn add(config: Config, payload: &str) -> std::result::Result<(), Failure> {
    let core = Core::open(config)
        .await
        .map_err(|err| Failure::Runtime(anyhow::Error::from(err)))?;
    match core.complete_pairing(payload).await {
        Ok(peer) => {
            println!("paired {} ({})", peer.name, peer.fingerprint);
            // Pairing is per-device: this only adds the peer to THIS device. The
            // other device must also pair this one (scan our QR, or `pair add`
            // our code) before sync works both ways (issue #42).
            println!(
                "This device now trusts {}. Make sure {} has also paired this device, \
                 or its pushes will be rejected.",
                peer.name, peer.name
            );
            Ok(())
        }
        // A decode/version failure is the user handing us a bad payload: usage.
        Err(err @ CoreError::Sync(_)) => Err(Failure::Usage(anyhow::anyhow!(
            "invalid pairing payload: {err}"
        ))),
        // Anything else (store write, transport) is a runtime failure.
        Err(other) => Err(Failure::Runtime(anyhow::Error::from(other))),
    }
}
