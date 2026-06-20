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

//! Real TCP transport for sealed sync frames (C1 #7): the live counterpart to
//! the in-memory `FakeLan` double.
//!
//! The send side implements [`SealedTransport`]; the inbound accept loop is
//! driven by the core ([`Core::run_sync_listener`](crate::Core::run_sync_listener)),
//! which opens, authenticates, and routes each frame. Framing is a single
//! 4-byte big-endian length prefix followed by the opaque sealed frame bytes.
//!
//! The transport never sees plaintext: it ships and receives already-sealed
//! envelopes, exactly like the in-memory double, but over a real socket. No
//! internet is involved; connections are LAN peer-to-peer to addresses learned
//! from mDNS discovery (or, in tests, loopback).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::error::{CoreError, Result};
use crate::sync::crypto::PUBLIC_KEY_LEN;
use crate::sync::transport::SealedTransport;

/// Defensive cap on an inbound sealed frame (1 MiB). A persona-upsert frame is a
/// few KB; anything larger is rejected before allocation rather than trusted.
pub const MAX_FRAME_LEN: usize = 1 << 20;

/// A shared public-key -> socket-address routing table. The send side resolves a
/// recipient's address here; the serve loop refreshes it from mDNS-discovered
/// peers (and tests seed it directly with a loopback address).
pub type RoutingTable = Arc<Mutex<HashMap<[u8; PUBLIC_KEY_LEN], SocketAddr>>>;

/// Create an empty routing table.
pub fn routing_table() -> RoutingTable {
    Arc::new(Mutex::new(HashMap::new()))
}

/// A real TCP [`SealedTransport`]. Resolves the recipient's address from the
/// shared [`RoutingTable`], connects, and writes one length-prefixed frame.
#[derive(Clone)]
pub struct TcpTransport {
    routes: RoutingTable,
}

impl std::fmt::Debug for TcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpTransport").finish_non_exhaustive()
    }
}

impl TcpTransport {
    /// Build a transport over a shared routing table.
    pub fn new(routes: RoutingTable) -> Self {
        Self { routes }
    }

    /// Insert or update the address for a recipient public key.
    pub async fn set_route(&self, public_key: [u8; PUBLIC_KEY_LEN], addr: SocketAddr) {
        self.routes.lock().await.insert(public_key, addr);
    }
}

#[async_trait]
impl SealedTransport for TcpTransport {
    async fn send(&self, recipient_public_key: &[u8; PUBLIC_KEY_LEN], frame: &[u8]) -> Result<()> {
        let addr = {
            let routes = self.routes.lock().await;
            routes.get(recipient_public_key).copied()
        }
        .ok_or_else(|| {
            CoreError::Sync(
                "no LAN route to recipient (peer not discovered or not advertising)".to_string(),
            )
        })?;
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| CoreError::Sync(format!("connect {addr}: {e}")))?;
        write_frame(&mut stream, frame).await?;
        stream
            .shutdown()
            .await
            .map_err(|e| CoreError::Sync(format!("close {addr}: {e}")))?;
        Ok(())
    }
}

/// Write one length-prefixed frame to a stream.
pub async fn write_frame<W>(stream: &mut W, frame: &[u8]) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let len = u32::try_from(frame.len())
        .map_err(|_| CoreError::Sync("sealed frame exceeds u32 length".to_string()))?;
    stream
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| CoreError::Sync(format!("write frame length: {e}")))?;
    stream
        .write_all(frame)
        .await
        .map_err(|e| CoreError::Sync(format!("write frame body: {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| CoreError::Sync(format!("flush frame: {e}")))?;
    Ok(())
}

/// Read one length-prefixed frame from a stream, rejecting an out-of-bounds
/// length before allocating.
pub async fn read_frame<R>(stream: &mut R) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| CoreError::Sync(format!("read frame length: {e}")))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(CoreError::Sync(format!(
            "inbound frame length {len} out of bounds (1..={MAX_FRAME_LEN})"
        )));
    }
    let mut frame = vec![0u8; len];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(|e| CoreError::Sync(format!("read frame body: {e}")))?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn key(n: u8) -> [u8; PUBLIC_KEY_LEN] {
        [n; PUBLIC_KEY_LEN]
    }

    #[tokio::test]
    async fn frame_round_trips_over_loopback() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| CoreError::Sync(e.to_string()))?;

        let payload = b"sealed-bytes-stand-in".to_vec();
        let expected = payload.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|e| CoreError::Sync(e.to_string()))?;
            read_frame(&mut stream).await
        });

        let routes = routing_table();
        let transport = TcpTransport::new(routes);
        transport.set_route(key(7), addr).await;
        transport.send(&key(7), &payload).await?;

        let received = server.await.map_err(|e| CoreError::Sync(e.to_string()))??;
        assert_eq!(received, expected);
        Ok(())
    }

    #[tokio::test]
    async fn send_without_a_route_fails_closed() {
        let transport = TcpTransport::new(routing_table());
        assert!(transport.send(&key(9), b"x").await.is_err());
    }

    #[tokio::test]
    async fn oversize_length_prefix_is_rejected_before_allocation() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|e| CoreError::Sync(e.to_string()))?;
            read_frame(&mut stream).await
        });

        // Announce a frame far larger than the cap; the reader must reject it.
        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let bogus_len = (MAX_FRAME_LEN as u32) + 1;
        client
            .write_all(&bogus_len.to_be_bytes())
            .await
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        client
            .flush()
            .await
            .map_err(|e| CoreError::Sync(e.to_string()))?;

        let result = server.await.map_err(|e| CoreError::Sync(e.to_string()))?;
        assert!(result.is_err(), "oversize length must be rejected");
        Ok(())
    }
}
