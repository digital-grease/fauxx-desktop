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

//! C1 #7 end-to-end LAN sync over the REAL TCP transport, on loopback.
//!
//! This is the live counterpart to the in-memory `FakeLan` unit tests: two
//! cores (with LAN sync enabled) pair out of band, one binds the real inbound
//! listener on an ephemeral loopback port, and the other pushes a sealed persona
//! frame to it over an actual TCP socket. The receiver's listener opens,
//! authenticates (attributing the sender by trying its paired keys), and
//! persists the persona into its encrypted store.
//!
//! Hermetic w.r.t. the network beyond loopback: routing is seeded explicitly via
//! `add_sync_route` (no mDNS dependency), so it runs anywhere TCP loopback works
//! (including CI), unlike the multicast-dependent discovery path.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::Notify;

use fauxx_core::error::{CoreError, Result};
use fauxx_core::store::KeySource;
use fauxx_core::{Config, Core, SyntheticPersona};

fn key_source(dir: &Path, label: &str) -> KeySource {
    KeySource::EncryptedFile {
        path: dir.join("key.bin"),
        passphrase: format!("lan-sync-tcp-{label}"),
    }
}

/// A LAN-sync-enabled `Core` over a temp dir with the headless passphrase key.
async fn open_core(dir: &Path, label: &str, port: u16) -> Result<Core> {
    let config = Config::new()
        .with_path(dir.join("fauxx.db"))
        .with_key_source(key_source(dir, label))
        .with_device_name(label)
        .with_sync_port(port)
        .with_lan_sync(true);
    Core::open(config).await
}

/// Mutually pair two cores by exchanging their pairing payloads (the QR
/// contents), as the wizard would after a scan.
async fn pair(a: &Core, b: &Core) -> Result<()> {
    let a_payload = a.pairing_payload().await?.encode()?;
    let b_payload = b.pairing_payload().await?.encode()?;
    a.complete_pairing(&b_payload).await?;
    b.complete_pairing(&a_payload).await?;
    Ok(())
}

#[tokio::test]
async fn persona_round_trips_from_one_core_to_another_over_real_tcp() -> Result<()> {
    let sender_dir = tempfile::tempdir()?;
    let receiver_dir = tempfile::tempdir()?;

    // Distinct sync ports so the two cores on this host do not collide.
    let sender = open_core(sender_dir.path(), "Sender", 46_101).await?;
    let receiver = open_core(receiver_dir.path(), "Receiver", 46_102).await?;

    pair(&sender, &receiver).await?;

    // Bind the receiver's inbound listener on an ephemeral loopback port and
    // drive it on a background task until we shut it down.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| CoreError::Sync(e.to_string()))?;
    let recv_addr = listener
        .local_addr()
        .map_err(|e| CoreError::Sync(e.to_string()))?;
    let shutdown = Arc::new(Notify::new());
    let listener_core = receiver.clone();
    let listener_shutdown = Arc::clone(&shutdown);
    let handle = tokio::spawn(async move {
        listener_core
            .serve_inbound(listener, listener_shutdown)
            .await
    });

    // Tell the sender how to reach the receiver (in production this comes from
    // mDNS discovery; here we route explicitly to the loopback listener).
    let receiver_key = receiver.sync_public_key()?;
    sender.add_sync_route(&receiver_key, recv_addr).await?;

    // The persona the sender owns and pushes. Includes every SyntheticPersona
    // field plus rotation timing, which the wire schema must round-trip.
    let persona = SyntheticPersona::new(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
        "Round Trip".to_string(),
        "AGE_35_44".to_string(),
        "TEACHER".to_string(),
        "CANADA".to_string(),
        vec!["ACADEMIC".to_string(), "HISTORY".to_string()],
        1_700_000_000_000,
        1_800_000_000_000,
    );
    sender.save_persona(&persona).await?;

    let pushed = sender.sync_persona_to_paired(&persona).await?;
    assert_eq!(
        pushed, 1,
        "persona should be sealed and sent to the one peer"
    );

    // Poll the receiver's store until the listener has applied the inbound frame
    // (the apply happens on a spawned task), with a bounded timeout.
    let mut applied = None;
    for _ in 0..50 {
        if let Ok(p) = receiver.get_persona(&persona.id).await {
            applied = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.notify_waiters();
    let _ = handle.await;

    let received = applied.ok_or_else(|| {
        CoreError::Sync("receiver should have persisted the synced persona".to_string())
    })?;
    assert_eq!(received.id, persona.id);
    assert_eq!(received.name, persona.name);
    assert_eq!(received.age_range, persona.age_range);
    assert_eq!(received.profession, persona.profession);
    assert_eq!(received.region, persona.region);
    assert_eq!(received.interests, persona.interests);
    assert_eq!(received.created_at, persona.created_at);
    assert_eq!(
        received.active_until, persona.active_until,
        "rotation timing must survive the sync"
    );
    Ok(())
}

#[tokio::test]
async fn unpaired_sender_frame_is_rejected_by_the_listener() -> Result<()> {
    let sender_dir = tempfile::tempdir()?;
    let receiver_dir = tempfile::tempdir()?;

    let sender = open_core(sender_dir.path(), "Stranger", 46_103).await?;
    let receiver = open_core(receiver_dir.path(), "Guarded", 46_104).await?;

    // NOTE: deliberately do NOT pair. The receiver has no paired peers, so an
    // inbound frame cannot authenticate against anyone.
    assert!(
        receiver.ingest_inbound_frame(b"junk").await.is_err(),
        "an inbound frame with no paired peers must be rejected"
    );

    // And even a real sealed frame from an unpaired sender is rejected, because
    // the receiver has no record of that sender's key to authenticate against.
    // (Pair only one direction so the sender can seal, but the receiver cannot
    // attribute.)
    let recv_payload = receiver.pairing_payload().await?.encode()?;
    sender.complete_pairing(&recv_payload).await?;
    let persona = SyntheticPersona::new(
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
        "No Trust".to_string(),
        "AGE_25_34".to_string(),
        "ENGINEER".to_string(),
        "USA".to_string(),
        vec!["TECHNOLOGY".to_string()],
        1,
        2,
    );
    let sealed = sender
        .seal_persona_for(&receiver.sync_public_key()?, &persona)
        .await?;
    assert!(
        receiver.ingest_inbound_frame(&sealed).await.is_err(),
        "a frame from an unpaired sender must not authenticate"
    );
    assert!(
        receiver.get_persona(&persona.id).await.is_err(),
        "the unpaired persona must not be persisted"
    );
    Ok(())
}

/// Extract the message from a `CoreError::Sync`, panicking on any other variant.
fn sync_message(err: &CoreError) -> String {
    match err {
        CoreError::Sync(m) => m.clone(),
        other => panic!("expected CoreError::Sync, got {other:?}"),
    }
}

#[tokio::test]
async fn rejection_when_not_paired_back_explains_both_ways_pairing() -> Result<()> {
    // #42 D2 (the #38 scenario): the phone (sender) scanned and paired the
    // desktop (receiver), but the desktop never paired the phone back. The
    // desktop must reject the phone's push with a clear, actionable both-ways
    // message rather than a bare auth failure.
    let sender_dir = tempfile::tempdir()?;
    let receiver_dir = tempfile::tempdir()?;
    let sender = open_core(sender_dir.path(), "Phone", 46_111).await?;
    let receiver = open_core(receiver_dir.path(), "Desktop", 46_112).await?;

    // One-way pairing: the phone pairs the desktop; the desktop does NOT pair back.
    let recv_payload = receiver.pairing_payload().await?.encode()?;
    sender.complete_pairing(&recv_payload).await?;

    let persona = SyntheticPersona::new(
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc".to_string(),
        "Push".to_string(),
        "AGE_35_44".to_string(),
        "TEACHER".to_string(),
        "CANADA".to_string(),
        vec!["ACADEMIC".to_string(), "HISTORY".to_string()],
        1_700_000_000_000,
        1_800_000_000_000,
    );
    let sealed = sender
        .seal_persona_for(&receiver.sync_public_key()?, &persona)
        .await?;

    // Case A: the receiver has NO paired peers at all -> the "no paired device"
    // branch, with the both-ways remediation hint.
    let Err(err) = receiver.ingest_inbound_frame(&sealed).await else {
        panic!("a not-paired-back push must be rejected");
    };
    let msg = sync_message(&err);
    assert!(
        msg.contains("BOTH"),
        "rejection must explain both-ways pairing, got: {msg}"
    );
    assert!(
        msg.contains("pair add") && msg.contains("Devices"),
        "rejection must point to the GUI/CLI fix, got: {msg}"
    );
    // The bare pre-#42 wording must be gone.
    assert!(
        !msg.contains("nothing can authenticate it. inbound"),
        "must not be the bare auth failure, got: {msg}"
    );

    // Case B: the receiver has paired a DIFFERENT device, so the attribution loop
    // runs but nothing authenticates -> the "not paired the sender back" branch.
    let other_dir = tempfile::tempdir()?;
    let other = open_core(other_dir.path(), "Tablet", 46_113).await?;
    let other_payload = other.pairing_payload().await?.encode()?;
    receiver.complete_pairing(&other_payload).await?;

    let Err(err2) = receiver.ingest_inbound_frame(&sealed).await else {
        panic!("a push from a peer not paired back must be rejected");
    };
    let msg2 = sync_message(&err2);
    assert!(
        msg2.contains("paired the sender back") && msg2.contains("BOTH"),
        "rejection must explain the sender was not paired back, got: {msg2}"
    );
    Ok(())
}

#[tokio::test]
async fn manual_numeric_ip_push_works_without_mdns() -> Result<()> {
    // #42 D3: when mDNS discovery is blocked (a VPN/proxy app, or a guest /
    // client-isolated Wi-Fi), a push must still work via a manually entered
    // numeric IP. The receiver binds by address and authenticates by key, so a
    // frame delivered to a numeric host (no discovery) is accepted exactly like a
    // discovered one, as long as the peer is paired both ways.
    let sender_dir = tempfile::tempdir()?;
    let receiver_dir = tempfile::tempdir()?;
    let sender = open_core(sender_dir.path(), "Sender", 46_121).await?;
    let receiver = open_core(receiver_dir.path(), "Receiver", 46_122).await?;
    pair(&sender, &receiver).await?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| CoreError::Sync(e.to_string()))?;
    let bound = listener
        .local_addr()
        .map_err(|e| CoreError::Sync(e.to_string()))?;
    let shutdown = Arc::new(Notify::new());
    let listener_core = receiver.clone();
    let listener_shutdown = Arc::clone(&shutdown);
    let handle = tokio::spawn(async move {
        listener_core
            .serve_inbound(listener, listener_shutdown)
            .await
    });

    // Route by a NUMERIC IP:port string (as a user would type under "Connect by
    // IP"), parsed to a SocketAddr -- no mDNS discovery involved.
    let manual = format!("127.0.0.1:{}", bound.port());
    let addr: std::net::SocketAddr = manual
        .parse()
        .map_err(|e| CoreError::Sync(format!("parse {manual}: {e}")))?;
    let receiver_key = receiver.sync_public_key()?;
    sender.add_sync_route(&receiver_key, addr).await?;

    let persona = SyntheticPersona::new(
        "dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_string(),
        "Manual IP".to_string(),
        "AGE_35_44".to_string(),
        "TEACHER".to_string(),
        "CANADA".to_string(),
        vec!["ACADEMIC".to_string(), "HISTORY".to_string()],
        1_700_000_000_000,
        1_800_000_000_000,
    );
    sender.save_persona(&persona).await?;
    let pushed = sender.sync_persona_to_paired(&persona).await?;
    assert_eq!(pushed, 1, "the paired peer should receive the push");

    let mut applied = None;
    for _ in 0..50 {
        if let Ok(p) = receiver.get_persona(&persona.id).await {
            applied = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.notify_waiters();
    let _ = handle.await;

    let received = applied.ok_or_else(|| {
        CoreError::Sync("a manual-IP push should have been received and persisted".to_string())
    })?;
    assert_eq!(received.id, persona.id);
    assert_eq!(received.name, persona.name);
    Ok(())
}
