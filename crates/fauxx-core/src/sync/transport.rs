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

//! Transport and discovery abstractions.
//!
//! The sync engine talks to the LAN through two object-safe async traits so the
//! sealed round-trip, the pairing handshake, and the "unpaired peer is
//! rejected" rule can all be unit-tested in memory with no real sockets or
//! multicast (CI has neither). The concrete mDNS/UDP wiring lives in
//! [`super::discovery`] and sits behind these same traits; any live integration
//! test is `#[ignore]`d so CI stays hermetic.

use async_trait::async_trait;

use crate::error::Result;
use crate::sync::crypto::PUBLIC_KEY_LEN;
use crate::sync::peer::DiscoveredPeer;

/// One byte-addressed delivery of a sealed frame to a peer.
///
/// The transport is deliberately framed at the sealed-envelope level (opaque
/// bytes): it never sees plaintext and never inspects payloads. The recipient
/// is identified by its public key so the engine can attribute and
/// authenticate the message on open.
#[async_trait]
pub trait SealedTransport: Send + Sync {
    /// Deliver an already-sealed frame to the peer identified by
    /// `recipient_public_key`. The bytes are opaque to the transport.
    async fn send(&self, recipient_public_key: &[u8; PUBLIC_KEY_LEN], frame: &[u8]) -> Result<()>;
}

/// LAN discovery: advertise this device and browse for peers.
///
/// Kept separate from [`SealedTransport`] so the in-memory test harness can
/// implement only what a test exercises, and so the concrete mDNS daemon can
/// own discovery while a future TCP/QUIC transport owns delivery.
#[async_trait]
pub trait Discovery: Send + Sync {
    /// Begin advertising this device on the LAN (idempotent).
    async fn advertise(&self) -> Result<()>;

    /// Stop advertising this device.
    async fn stop_advertising(&self) -> Result<()>;

    /// Snapshot the peers seen so far. Browsing runs continuously once started;
    /// this returns the current view.
    async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>>;
}

/// A discovery backend that advertises nothing and finds no peers.
///
/// Used when the live mDNS daemon cannot start (some sandboxed hosts cannot open
/// the multicast sockets `mdns-sd` creates eagerly) but the TCP transport seam is
/// still wanted: the engine degrades to "peers reachable only via stored
/// paired-peer addresses" rather than failing [`Core::open`](crate::Core::open).
#[derive(Debug, Default)]
pub struct NullDiscovery;

#[async_trait]
impl Discovery for NullDiscovery {
    async fn advertise(&self) -> Result<()> {
        Ok(())
    }

    async fn stop_advertising(&self) -> Result<()> {
        Ok(())
    }

    async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! In-memory transport and discovery doubles used by the sync unit tests.
    //! They model a shared LAN as a process-local table, with no sockets.

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{Discovery, SealedTransport};
    use crate::error::{CoreError, Result};
    use crate::sync::crypto::PUBLIC_KEY_LEN;
    use crate::sync::peer::DiscoveredPeer;

    /// Per-recipient queues of sealed frames, keyed by recipient public key.
    type Inboxes = Arc<Mutex<HashMap<[u8; PUBLIC_KEY_LEN], Vec<Vec<u8>>>>>;

    /// A fake LAN: maps a recipient public key to the queue of frames sent to
    /// it, and holds the set of advertised peers. Shared (cloned `Arc`) by the
    /// transports plugged into each simulated device.
    #[derive(Clone, Default)]
    pub struct FakeLan {
        inboxes: Inboxes,
        advertised: Arc<Mutex<Vec<DiscoveredPeer>>>,
    }

    impl FakeLan {
        pub fn new() -> Self {
            Self::default()
        }

        /// Drain the frames delivered to `recipient`.
        pub fn take_inbox(&self, recipient: &[u8; PUBLIC_KEY_LEN]) -> Result<Vec<Vec<u8>>> {
            let mut guard = self
                .inboxes
                .lock()
                .map_err(|_| CoreError::Sync("fake lan inbox lock poisoned".to_string()))?;
            Ok(guard.remove(recipient).unwrap_or_default())
        }

        /// Register a peer as advertised on the fake LAN.
        pub fn advertise_peer(&self, peer: DiscoveredPeer) -> Result<()> {
            let mut guard = self
                .advertised
                .lock()
                .map_err(|_| CoreError::Sync("fake lan advertised lock poisoned".to_string()))?;
            guard.push(peer);
            Ok(())
        }
    }

    #[async_trait]
    impl SealedTransport for FakeLan {
        async fn send(
            &self,
            recipient_public_key: &[u8; PUBLIC_KEY_LEN],
            frame: &[u8],
        ) -> Result<()> {
            let mut guard = self
                .inboxes
                .lock()
                .map_err(|_| CoreError::Sync("fake lan inbox lock poisoned".to_string()))?;
            guard
                .entry(*recipient_public_key)
                .or_default()
                .push(frame.to_vec());
            Ok(())
        }
    }

    /// A transport whose `send` ALWAYS fails, for exercising the fail-closed push
    /// paths (e.g. coherent election must NOT record household convergence if the
    /// push to peers fails). Pairing does not go through the transport, so a
    /// device built with this can still pair and then fail only on push.
    #[derive(Clone, Default)]
    pub struct FailingTransport;

    #[async_trait]
    impl SealedTransport for FailingTransport {
        async fn send(
            &self,
            _recipient_public_key: &[u8; PUBLIC_KEY_LEN],
            _frame: &[u8],
        ) -> Result<()> {
            Err(CoreError::Sync(
                "simulated transport send failure".to_string(),
            ))
        }
    }

    #[async_trait]
    impl Discovery for FakeLan {
        async fn advertise(&self) -> Result<()> {
            Ok(())
        }

        async fn stop_advertising(&self) -> Result<()> {
            Ok(())
        }

        async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
            let guard = self
                .advertised
                .lock()
                .map_err(|_| CoreError::Sync("fake lan advertised lock poisoned".to_string()))?;
            Ok(guard.clone())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn fake_lan_surfaces_advertised_peers() -> Result<()> {
            let lan = FakeLan::new();
            assert!(lan.discovered_peers().await?.is_empty());
            lan.advertise_peer(DiscoveredPeer {
                name: "Phone".to_string(),
                host: Some("phone.local.".to_string()),
                addresses: vec!["192.168.1.7:45999".to_string()],
                port: 45_999,
                fingerprint: Some("1a2b:3c4d:5e6f:7081".to_string()),
                public_key: None,
                protocol_version: Some(1),
            })?;
            let peers = lan.discovered_peers().await?;
            assert_eq!(peers.len(), 1);
            assert_eq!(peers[0].name, "Phone");
            Ok(())
        }
    }
}
