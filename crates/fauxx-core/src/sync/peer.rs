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

//! Peer records: discovered (untrusted) and paired (trusted).
//!
//! A [`DiscoveredPeer`] is what mDNS browsing surfaces: a name, address, port,
//! and the advertised public-key fingerprint. It is *not* trusted; discovery
//! alone never grants sync.
//!
//! A [`PairedPeer`] is a device whose public key this device has accepted out
//! of band (via the QR handshake). Only paired peers can be sealed to or
//! opened from, which is the API-level half of "an unpaired peer cannot sync";
//! the cryptographic half is enforced by the sealed channel.

use serde::{Deserialize, Serialize};

use crate::sync::crypto::PUBLIC_KEY_LEN;
use crate::sync::wire::{encode_public_key, fingerprint};

/// A peer surfaced by mDNS discovery. Untrusted until paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPeer {
    /// The mDNS instance name (human-readable device name).
    pub name: String,
    /// The resolved host name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Socket addresses (IP:port) the peer advertised.
    pub addresses: Vec<String>,
    /// The advertised sync port.
    pub port: u16,
    /// The advertised public-key fingerprint (from the TXT record). `None` if
    /// the peer did not advertise one (then it cannot be paired from discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// The advertised full public key (base64url), if present in the TXT record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// The protocol version the peer advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u16>,
}

/// A device this one has paired with: its public key plus identifying metadata.
///
/// Persisted in the encrypted store. The public key is not secret, but the
/// *set* of paired peers is access-control state, so it lives behind SQLCipher
/// alongside the persona cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedPeer {
    /// Human-readable device name captured at pairing time.
    pub name: String,
    /// The peer's X25519 public key, base64url (no padding). This is the
    /// trust anchor: messages are sealed to and authenticated from this key.
    pub public_key: String,
    /// The fingerprint derived from `public_key`, for display and lookup.
    pub fingerprint: String,
    /// Last known connection hint (host), best-effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Last known sync port.
    pub port: u16,
    /// Epoch milliseconds when pairing was completed.
    pub paired_at: i64,
}

impl PairedPeer {
    /// Build a paired-peer record from its public-key bytes and metadata,
    /// stamping the fingerprint.
    pub fn new(
        name: String,
        public_key: &[u8; PUBLIC_KEY_LEN],
        host: Option<String>,
        port: u16,
        paired_at: i64,
    ) -> Self {
        Self {
            name,
            public_key: encode_public_key(public_key),
            fingerprint: fingerprint(public_key),
            host,
            port,
            paired_at,
        }
    }

    /// Decode this peer's public key into fixed-size bytes.
    pub fn public_key_bytes(&self) -> crate::error::Result<[u8; PUBLIC_KEY_LEN]> {
        crate::sync::wire::decode_public_key(&self.public_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_peer_round_trips_and_stamps_fingerprint() -> crate::error::Result<()> {
        let pk = [11u8; PUBLIC_KEY_LEN];
        let peer = PairedPeer::new("Phone".to_string(), &pk, None, 45_999, 1_700_000_000_000);
        assert_eq!(peer.public_key_bytes()?, pk);
        assert_eq!(peer.fingerprint, fingerprint(&pk));

        let json = serde_json::to_string(&peer)?;
        assert!(json.contains("\"publicKey\""));
        assert!(json.contains("\"pairedAt\""));
        let back: PairedPeer = serde_json::from_str(&json)?;
        assert_eq!(back, peer);
        Ok(())
    }

    #[test]
    fn discovered_peer_serializes_camelcase() -> crate::error::Result<()> {
        let peer = DiscoveredPeer {
            name: "Phone".to_string(),
            host: Some("phone.local.".to_string()),
            addresses: vec!["192.168.1.7:45999".to_string()],
            port: 45_999,
            fingerprint: Some("1a2b:3c4d:5e6f:7081".to_string()),
            public_key: None,
            protocol_version: Some(1),
        };
        let json = serde_json::to_string(&peer)?;
        assert!(json.contains("\"protocolVersion\""));
        let back: DiscoveredPeer = serde_json::from_str(&json)?;
        assert_eq!(back, peer);
        Ok(())
    }
}
