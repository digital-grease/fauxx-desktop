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

//! Encrypted LAN persona sync (C1 #7): QR + mDNS, no cloud.
//!
//! This module is the desktop-authoritative half of the cross-device sync
//! contract that the Android side (issue E13) implements against. It is 100%
//! local: discovery is link-local mDNS multicast, pairing is out of band via a
//! QR, and persona payloads travel inside an authenticated public-key
//! crypto_box. No backend, no telemetry, no internet access.
//!
//! Layout:
//! - [`crypto`]: the device X25519 identity and the sealed channel.
//! - [`wire`]: the versioned [`SyncMessage`] envelope, the QR
//!   [`PairingPayload`], and the public-key fingerprint.
//! - [`qr`]: rendering the pairing QR (unicode and SVG).
//! - [`peer`]: discovered (untrusted) and paired (trusted) peer records.
//! - [`transport`]: the object-safe [`Discovery`] / [`SealedTransport`] traits
//!   the engine talks through, so the sealed round-trip, the pairing handshake,
//!   and "unpaired peer is rejected" are unit-tested fully in memory.
//! - [`discovery`]: the live mDNS implementation of [`Discovery`].
//!
//! Security model (the wire format and full spec live in [`wire`] and the rest
//! of [`crate::sync`]):
//! - Confidentiality + authenticity: every payload is sealed with
//!   `crypto_box_easy` (X25519 + XSalsa20-Poly1305), sender = this device,
//!   recipient = a paired peer, fresh random nonce per message. Captured
//!   traffic contains zero plaintext persona fields.
//! - Access control: the engine seals only to, and opens only from, *paired*
//!   peers. An unpaired peer cannot read a message (wrong recipient key) nor
//!   forge one the engine will accept (the MAC will not authenticate against a
//!   paired sender key). This is the "unpaired peer cannot sync" rule.
//! - Secret hygiene: the device secret key is zeroized on drop and never
//!   appears in any public API, `Debug`, or log; it persists only in the OS
//!   keystore.

pub mod crypto;
pub mod discovery;
pub mod peer;
pub mod qr;
pub mod tcp;
pub mod transport;
pub mod wire;

use std::sync::Arc;

use tokio::sync::Mutex;

pub use crypto::{DeviceIdentity, SealedEnvelope, MAC_LEN, NONCE_LEN, PUBLIC_KEY_LEN};
pub use discovery::{AdvertisedDevice, MdnsDiscovery};
pub use peer::{DiscoveredPeer, PairedPeer};
pub use qr::PairingQr;
pub use tcp::{routing_table, RoutingTable, TcpTransport};
pub use transport::{Discovery, NullDiscovery, SealedTransport};
pub use wire::{
    decode_public_key, encode_public_key, fingerprint, CoordinationState, PairingPayload,
    PersonaPackBody, PublicIpReport, SyncBody, SyncMessage, SERVICE_TYPE, SYNC_PROTOCOL_VERSION,
};

use crate::error::{CoreError, Result};
use crate::persona::SyntheticPersona;
use crate::store::{EncryptedStore, KeySource};

/// The sealed wire frame: a tiny versioned header, the nonce, and the
/// ciphertext. This is exactly what the transport carries (and what the phone
/// frames against). The persona JSON is wholly inside `ciphertext`, never in
/// the clear.
///
/// Binary layout: `magic(4) "FXS1" || version(u16 LE) || nonce(24) ||
/// ciphertext(..)`. Length-delimited framing (e.g. a 4-byte prefix) is the
/// transport's concern; this is the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedFrame {
    /// The crypto layer's sealed envelope (nonce + ciphertext+MAC).
    pub envelope: SealedEnvelope,
}

/// 4-byte magic marking a Fauxx sealed sync frame.
const FRAME_MAGIC: &[u8; 4] = b"FXS1";

impl SealedFrame {
    /// Serialize the frame to the bytes the transport ships.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(FRAME_MAGIC.len() + 2 + NONCE_LEN + self.envelope.ciphertext.len());
        out.extend_from_slice(FRAME_MAGIC);
        out.extend_from_slice(&SYNC_PROTOCOL_VERSION.to_le_bytes());
        out.extend_from_slice(&self.envelope.nonce);
        out.extend_from_slice(&self.envelope.ciphertext);
        out
    }

    /// Parse a frame from transport bytes, failing closed on a bad magic, a
    /// short buffer, or an unsupported version.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let header = FRAME_MAGIC.len() + 2 + NONCE_LEN;
        if bytes.len() < header + MAC_LEN {
            return Err(CoreError::Sync("sealed frame too short".to_string()));
        }
        if &bytes[..FRAME_MAGIC.len()] != FRAME_MAGIC {
            return Err(CoreError::Sync("not a fauxx sealed frame".to_string()));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version > SYNC_PROTOCOL_VERSION {
            return Err(CoreError::Sync(format!(
                "unsupported sync frame version {version} (this build speaks {SYNC_PROTOCOL_VERSION})"
            )));
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[6..6 + NONCE_LEN]);
        let ciphertext = bytes[header..].to_vec();
        Ok(Self {
            envelope: SealedEnvelope { nonce, ciphertext },
        })
    }
}

/// The headless LAN sync engine.
///
/// Holds this device's pairing identity, the encrypted store (for the paired-
/// peer set and the persona cache), and the transport seam. Cheap to clone:
/// shared state is behind an `Arc`. Created by [`Core`](crate::Core) and
/// reached through its async accessors.
#[derive(Clone)]
pub struct LanSync {
    inner: Arc<LanSyncInner>,
}

struct LanSyncInner {
    /// This device's long-lived pairing identity (secret zeroized on drop).
    identity: DeviceIdentity,
    /// A human-readable name for this device (the mDNS instance name).
    device_name: String,
    /// The sync transport port advertised in the QR and mDNS TXT.
    port: u16,
    /// The encrypted store, shared with the rest of the core. `None` for a
    /// store-less engine (smoke tests); persistence then errors.
    store: Option<Arc<Mutex<EncryptedStore>>>,
    /// The byte transport for sealed frames (in-memory in tests, a real
    /// network transport in production once one is wired).
    transport: Option<Arc<dyn SealedTransport>>,
    /// LAN discovery. `None` until advertising starts.
    discovery: Option<Arc<dyn Discovery>>,
}

impl std::fmt::Debug for LanSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanSync")
            .field("device_name", &self.inner.device_name)
            .field("port", &self.inner.port)
            .field("public_key_fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

/// The default TCP port the sync transport advertises. (The byte transport
/// itself is pluggable; this is the hint carried in the QR and TXT record.)
pub const DEFAULT_SYNC_PORT: u16 = 45_999;

impl LanSync {
    /// Load the device identity from the OS keystore (creating and persisting
    /// one on first run) and build an engine. The identity's secret key never
    /// leaves the keystore boundary except into this in-memory identity.
    ///
    /// `store` is the shared encrypted store; the engine persists paired peers
    /// and the persona cache through it.
    pub fn open(
        key_source: &KeySource,
        device_name: String,
        port: u16,
        store: Arc<Mutex<EncryptedStore>>,
    ) -> Result<Self> {
        let identity = load_or_create_identity(key_source)?;
        Ok(Self {
            inner: Arc::new(LanSyncInner {
                identity,
                device_name,
                port,
                store: Some(store),
                transport: None,
                discovery: None,
            }),
        })
    }

    /// Attach live transport + discovery seams to a freshly-opened engine,
    /// consuming it. Used by [`Core`](crate::Core) when LAN sync is enabled
    /// (opt-in): the engine opens first (loading the keystore identity), then the
    /// concrete [`TcpTransport`](crate::sync::tcp::TcpTransport) and
    /// [`MdnsDiscovery`](crate::sync::discovery::MdnsDiscovery) are bolted on.
    ///
    /// Errors if the engine has already been shared (cloned), since the identity
    /// cannot be moved out of a shared `Arc`; the core attaches seams before the
    /// orchestrator clones the engine, so the refcount is one here.
    pub fn with_seams(
        self,
        transport: Arc<dyn SealedTransport>,
        discovery: Arc<dyn Discovery>,
    ) -> Result<Self> {
        let inner = Arc::try_unwrap(self.inner).map_err(|_| {
            CoreError::Sync("cannot attach sync seams: engine already shared".to_string())
        })?;
        Ok(Self {
            inner: Arc::new(LanSyncInner {
                transport: Some(transport),
                discovery: Some(discovery),
                ..inner
            }),
        })
    }

    /// Describe how this device advertises over mDNS, derived from its identity
    /// and configured name/port. Feeds
    /// [`MdnsDiscovery::new`](crate::sync::discovery::MdnsDiscovery::new).
    pub fn advertised_device(&self) -> AdvertisedDevice {
        let pk = self.inner.identity.public_key();
        AdvertisedDevice {
            instance_name: self.inner.device_name.clone(),
            host_name: format!("{}.local.", sanitize_host(&self.inner.device_name)),
            port: self.inner.port,
            public_key_b64: encode_public_key(pk),
            fingerprint: fingerprint(pk),
        }
    }

    /// Build an engine from an explicit identity and pluggable seams. Used by
    /// tests (with the in-memory transport) and by callers that wire their own
    /// transport/discovery.
    pub fn with_parts(
        identity: DeviceIdentity,
        device_name: String,
        port: u16,
        store: Option<Arc<Mutex<EncryptedStore>>>,
        transport: Option<Arc<dyn SealedTransport>>,
        discovery: Option<Arc<dyn Discovery>>,
    ) -> Self {
        Self {
            inner: Arc::new(LanSyncInner {
                identity,
                device_name,
                port,
                store,
                transport,
                discovery,
            }),
        }
    }

    /// This device's public key (safe to share).
    pub fn public_key(&self) -> &[u8; PUBLIC_KEY_LEN] {
        self.inner.identity.public_key()
    }

    /// This device's public-key fingerprint (for display).
    pub fn fingerprint(&self) -> String {
        fingerprint(self.inner.identity.public_key())
    }

    /// This device's name (mDNS instance name).
    pub fn device_name(&self) -> &str {
        &self.inner.device_name
    }

    /// The sync transport port this device advertises and listens on.
    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Begin advertising and browsing over the attached discovery backend, if any
    /// is attached. A no-op (Ok) when no backend is present, so a caller can
    /// always invoke it regardless of whether LAN sync was enabled.
    pub async fn advertise_if_enabled(&self) -> Result<()> {
        match &self.inner.discovery {
            Some(d) => d.advertise().await,
            None => Ok(()),
        }
    }

    /// Build the pairing payload this device shows to a peer.
    pub fn pairing_payload(&self) -> PairingPayload {
        PairingPayload::new(
            self.inner.device_name.clone(),
            self.inner.identity.public_key(),
            Some(format!("{}.local.", sanitize_host(&self.inner.device_name))),
            self.inner.port,
        )
    }

    /// Render the pairing QR (unicode + SVG) for this device.
    pub fn pairing_qr(&self) -> Result<PairingQr> {
        qr::render(&self.pairing_payload())
    }

    /// Begin advertising and browsing over the attached discovery backend. Errors
    /// if no discovery backend is attached.
    pub async fn advertise(&self) -> Result<()> {
        match &self.inner.discovery {
            Some(d) => d.advertise().await,
            None => Err(CoreError::Sync("no discovery backend attached".to_string())),
        }
    }

    /// Stop advertising over the attached discovery backend.
    pub async fn stop_advertising(&self) -> Result<()> {
        match &self.inner.discovery {
            Some(d) => d.stop_advertising().await,
            None => Err(CoreError::Sync("no discovery backend attached".to_string())),
        }
    }

    /// List peers seen by discovery (untrusted). Empty if no backend is
    /// attached.
    pub async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
        match &self.inner.discovery {
            Some(d) => d.discovered_peers().await,
            None => Ok(Vec::new()),
        }
    }

    /// Complete pairing with a peer described by a scanned pairing payload.
    ///
    /// The handshake is two-sided: this records the *peer's* public key (so we
    /// can seal to and authenticate from it), and the peer, having scanned (or
    /// been told) *our* payload, records ours. Both ends thus hold each other's
    /// public key, which is what unlocks the sealed channel between them. The
    /// record is persisted in the encrypted store.
    pub async fn complete_pairing(&self, scanned: &PairingPayload) -> Result<PairedPeer> {
        let pk = scanned.public_key_bytes()?;
        // Refuse to "pair with self": that would let our own broadcast loop back.
        if &pk == self.inner.identity.public_key() {
            return Err(CoreError::Sync(
                "refusing to pair a device with itself".to_string(),
            ));
        }
        let peer = PairedPeer::new(
            scanned.name.clone(),
            &pk,
            scanned.host.clone(),
            scanned.port,
            now_millis(),
        );
        self.with_store(|store| store.save_paired_peer(&peer))
            .await?;
        tracing::info!(
            peer = %peer.name,
            fingerprint = %peer.fingerprint,
            "paired with peer over LAN sync"
        );
        Ok(peer)
    }

    /// The set of paired peers (trusted). Empty if no store is attached.
    pub async fn paired_peers(&self) -> Result<Vec<PairedPeer>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_paired_peers(),
            None => Ok(Vec::new()),
        }
    }

    /// Revoke a paired peer by its base64url public key, removing its ability to
    /// sync. Returns `true` if a record was removed.
    pub async fn unpair(&self, public_key: &str) -> Result<bool> {
        self.with_store(|store| store.delete_paired_peer(public_key))
            .await
    }

    /// Seal a persona for one paired peer and frame it for the transport.
    ///
    /// Errors if the peer is not paired (the engine never seals to an unpaired
    /// key). The returned bytes are the on-wire sealed frame.
    pub async fn seal_persona_for(
        &self,
        peer_public_key: &str,
        persona: &SyntheticPersona,
    ) -> Result<Vec<u8>> {
        let peer = self.require_paired(peer_public_key).await?;
        let recipient = peer.public_key_bytes()?;
        let message = SyncMessage::persona_upsert(persona.clone());
        let plaintext = message.to_plaintext()?;
        let envelope = self.inner.identity.seal(&recipient, &plaintext)?;
        Ok(SealedFrame { envelope }.to_bytes())
    }

    /// Push a persona to a single paired peer over the attached transport.
    /// Errors if the peer is unpaired or no transport is attached.
    pub async fn push_persona_to(
        &self,
        peer_public_key: &str,
        persona: &SyntheticPersona,
    ) -> Result<()> {
        let peer = self.require_paired(peer_public_key).await?;
        let recipient = peer.public_key_bytes()?;
        let frame = self.seal_persona_for(peer_public_key, persona).await?;
        let transport = self
            .inner
            .transport
            .as_ref()
            .ok_or_else(|| CoreError::Sync("no transport attached".to_string()))?;
        transport.send(&recipient, &frame).await
    }

    /// Push a persona to every paired peer. Returns the count of peers it was
    /// sealed and sent to. A store-less or transport-less engine errors.
    pub async fn push_persona_to_all(&self, persona: &SyntheticPersona) -> Result<usize> {
        let peers = self.paired_peers().await?;
        let mut sent = 0usize;
        for peer in &peers {
            self.push_persona_to(&peer.public_key, persona).await?;
            sent += 1;
        }
        Ok(sent)
    }

    /// Receive a sealed frame attributed to a paired sender, open + verify it,
    /// and upsert the carried persona into the store. Returns the upserted
    /// persona.
    ///
    /// `sender_public_key` MUST be the claimed sender's base64url key. If that
    /// peer is not paired, the message is rejected before any crypto (and even
    /// if it were attempted, the MAC would not authenticate). This is the
    /// receive-side enforcement of "unpaired peers cannot sync".
    pub async fn receive_frame(
        &self,
        sender_public_key: &str,
        frame_bytes: &[u8],
    ) -> Result<SyntheticPersona> {
        let (sender, message) = self.open_frame(sender_public_key, frame_bytes).await?;
        match message.body {
            SyncBody::PersonaUpsert(persona) => {
                self.with_store(|store| store.save_persona(&persona))
                    .await?;
                tracing::info!(
                    persona = %persona.id,
                    sender = %sender.name,
                    "received and upserted persona over LAN sync"
                );
                Ok(persona)
            }
            // `receive_frame` is the persona-upsert path; other kinds (C1 #8/#9)
            // are routed through `receive_sync_message`. Fail closed here rather
            // than silently dropping a coordination frame on the wrong path.
            other => Err(CoreError::Sync(format!(
                "receive_frame expected PersonaUpsert; got a different kind ({}); use receive_sync_message",
                other.kind_name()
            ))),
        }
    }

    /// Receive a sealed frame attributed to a paired sender, open + verify it,
    /// and return the opened [`SyncMessage`] WITHOUT applying it. This is the
    /// general path the orchestration layer (C1 #8/#9) uses to route the
    /// non-persona kinds ([`SyncBody::PublicIpReport`],
    /// [`SyncBody::CoordinationState`]); a `PersonaUpsert` is also returned and
    /// the caller decides whether to persist it. Enforcement is identical to
    /// [`Self::receive_frame`]: an unpaired sender, a forged/tampered frame, or
    /// an unsupported version is rejected.
    pub async fn receive_sync_message(
        &self,
        sender_public_key: &str,
        frame_bytes: &[u8],
    ) -> Result<SyncMessage> {
        let (_sender, message) = self.open_frame(sender_public_key, frame_bytes).await?;
        Ok(message)
    }

    /// Seal an arbitrary [`SyncMessage`] for one paired peer and frame it for
    /// the transport. Errors if the peer is not paired. Used by the
    /// orchestration layer to ship the C1 #8/#9 coordination kinds over the
    /// same sealed channel as persona upserts.
    pub async fn seal_message_for(
        &self,
        peer_public_key: &str,
        message: &SyncMessage,
    ) -> Result<Vec<u8>> {
        let peer = self.require_paired(peer_public_key).await?;
        let recipient = peer.public_key_bytes()?;
        let plaintext = message.to_plaintext()?;
        let envelope = self.inner.identity.seal(&recipient, &plaintext)?;
        Ok(SealedFrame { envelope }.to_bytes())
    }

    /// Push an arbitrary [`SyncMessage`] to a single paired peer over the
    /// attached transport. Errors if the peer is unpaired or no transport is
    /// attached.
    pub async fn push_message_to(
        &self,
        peer_public_key: &str,
        message: &SyncMessage,
    ) -> Result<()> {
        let peer = self.require_paired(peer_public_key).await?;
        let recipient = peer.public_key_bytes()?;
        let frame = self.seal_message_for(peer_public_key, message).await?;
        let transport = self
            .inner
            .transport
            .as_ref()
            .ok_or_else(|| CoreError::Sync("no transport attached".to_string()))?;
        transport.send(&recipient, &frame).await
    }

    /// Push an arbitrary [`SyncMessage`] to every paired peer. Returns the count
    /// of peers it was sealed and sent to.
    pub async fn push_message_to_all(&self, message: &SyncMessage) -> Result<usize> {
        let peers = self.paired_peers().await?;
        let mut sent = 0usize;
        for peer in &peers {
            self.push_message_to(&peer.public_key, message).await?;
            sent += 1;
        }
        Ok(sent)
    }

    /// Open a sealed frame attributed to a paired sender into its
    /// [`SyncMessage`], enforcing pairing and authentication. The shared core of
    /// the receive paths.
    async fn open_frame(
        &self,
        sender_public_key: &str,
        frame_bytes: &[u8],
    ) -> Result<(PairedPeer, SyncMessage)> {
        let sender = self.require_paired(sender_public_key).await?;
        let sender_pk = sender.public_key_bytes()?;
        let frame = SealedFrame::from_bytes(frame_bytes)?;
        let plaintext = self.inner.identity.open(&sender_pk, &frame.envelope)?;
        let message = SyncMessage::from_plaintext(&plaintext)?;
        Ok((sender, message))
    }

    /// Look up a paired peer by public key, erroring if it is not paired. This
    /// is the single choke point that enforces "only paired peers sync".
    async fn require_paired(&self, public_key: &str) -> Result<PairedPeer> {
        match &self.inner.store {
            Some(store) => store
                .lock()
                .await
                .get_paired_peer(public_key)?
                .ok_or_else(|| {
                    CoreError::Sync(format!("peer {public_key} is not paired; refusing to sync"))
                }),
            None => Err(CoreError::Sync(
                "no store attached; cannot verify pairing".to_string(),
            )),
        }
    }

    /// Run a closure against the locked store, erroring if none is attached.
    async fn with_store<T>(&self, f: impl FnOnce(&EncryptedStore) -> Result<T>) -> Result<T> {
        match &self.inner.store {
            Some(store) => f(&*store.lock().await),
            None => Err(CoreError::Sync("no store attached".to_string())),
        }
    }
}

/// Load this device's long-lived pairing identity from the OS keystore, creating
/// and persisting one on first run. The secret key never leaves the keystore
/// boundary except into the in-memory identity (and is scrubbed from the
/// transient buffer here). Shared by [`LanSync::open`].
fn load_or_create_identity(key_source: &KeySource) -> Result<DeviceIdentity> {
    match crate::store::load_device_keypair(key_source)? {
        Some(blob) => DeviceIdentity::from_bytes(&blob[..PUBLIC_KEY_LEN], &blob[PUBLIC_KEY_LEN..]),
        None => {
            let identity = DeviceIdentity::generate();
            let mut blob = Vec::with_capacity(crate::store::DEVICE_KEYPAIR_LEN);
            blob.extend_from_slice(identity.public_key());
            blob.extend_from_slice(identity.secret_key_bytes());
            crate::store::store_device_keypair(key_source, &blob)?;
            // Scrub the transient secret-bearing buffer.
            use zeroize::Zeroize as _;
            blob.zeroize();
            Ok(identity)
        }
    }
}

/// Sanitize a device name into a host-label fragment (ASCII alphanumerics and
/// hyphens only), so the advertised host hint is a plausible mDNS label.
fn sanitize_host(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "fauxx".to_string()
    } else {
        cleaned
    }
}

/// Current wall-clock time in epoch milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};
    use crate::store::EncryptedStore;
    use crate::sync::transport::testing::FakeLan;
    use std::path::Path;

    fn passphrase_source(dir: &Path) -> KeySource {
        KeySource::EncryptedFile {
            path: dir.join("key.bin"),
            passphrase: "sync-test-passphrase".to_string(),
        }
    }

    fn open_store(dir: &Path) -> Result<Arc<Mutex<EncryptedStore>>> {
        let store = EncryptedStore::open_at(&dir.join("fauxx.db"), &passphrase_source(dir))?;
        Ok(Arc::new(Mutex::new(store)))
    }

    fn sample_persona() -> SyntheticPersona {
        let mut p = SyntheticPersona::new(
            "66666666-6666-4666-8666-666666666666".to_string(),
            "Sync Persona".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            vec![
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
                CategoryPool::GAMING.as_name().to_string(),
            ],
            1_700_000_000_321,
            1_700_600_000_654,
        );
        p.note = Some("synced note".to_string());
        p
    }

    /// Build two engines that share a FakeLan transport but have distinct
    /// stores and identities, modelling a desktop and a phone.
    async fn paired_pair(
        desktop_dir: &Path,
        phone_dir: &Path,
    ) -> Result<(LanSync, LanSync, FakeLan)> {
        let lan = FakeLan::new();
        let desktop = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(desktop_dir)?),
            Some(Arc::new(lan.clone())),
            None,
        );
        let phone = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Phone".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(phone_dir)?),
            Some(Arc::new(lan.clone())),
            None,
        );
        // Two-sided pairing: each records the other's payload.
        desktop.complete_pairing(&phone.pairing_payload()).await?;
        phone.complete_pairing(&desktop.pairing_payload()).await?;
        Ok((desktop, phone, lan))
    }

    #[test]
    fn sealed_frame_round_trips() -> Result<()> {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let env = alice.seal(bob.public_key(), b"hello frame")?;
        let frame = SealedFrame { envelope: env };
        let bytes = frame.to_bytes();
        let back = SealedFrame::from_bytes(&bytes)?;
        assert_eq!(back, frame);
        let opened = bob.open(alice.public_key(), &back.envelope)?;
        assert_eq!(opened, b"hello frame");
        Ok(())
    }

    #[test]
    fn sealed_frame_rejects_bad_magic() {
        assert!(SealedFrame::from_bytes(&[0u8; 64]).is_err());
        assert!(SealedFrame::from_bytes(b"short").is_err());
    }

    #[tokio::test]
    async fn pairing_persists_and_is_two_sided() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, _lan) = paired_pair(dd.path(), pd.path()).await?;

        // Each side holds exactly the other.
        let desktop_peers = desktop.paired_peers().await?;
        assert_eq!(desktop_peers.len(), 1);
        assert_eq!(
            desktop_peers[0].public_key,
            encode_public_key(phone.public_key())
        );

        let phone_peers = phone.paired_peers().await?;
        assert_eq!(phone_peers.len(), 1);
        assert_eq!(
            phone_peers[0].public_key,
            encode_public_key(desktop.public_key())
        );
        Ok(())
    }

    #[tokio::test]
    async fn refuses_to_pair_with_self() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let desktop = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(dd.path())?),
            None,
            None,
        );
        assert!(matches!(
            desktop.complete_pairing(&desktop.pairing_payload()).await,
            Err(CoreError::Sync(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn push_and_receive_round_trips_persona() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, lan) = paired_pair(dd.path(), pd.path()).await?;
        let persona = sample_persona();

        // Desktop seals + sends to the phone.
        let sent = desktop.push_persona_to_all(&persona).await?;
        assert_eq!(sent, 1);

        // The phone drains its inbox and opens the frame.
        let inbox = lan.take_inbox(phone.public_key())?;
        assert_eq!(inbox.len(), 1);
        let received = phone
            .receive_frame(&encode_public_key(desktop.public_key()), &inbox[0])
            .await?;

        // Every field, including rotation timing, survives.
        assert_eq!(received, persona);
        // And it landed in the phone's store.
        let stored = phone.paired_peers().await?;
        assert_eq!(stored.len(), 1);
        let from_store = {
            let s = pd.path();
            let store = open_store(s)?;
            let g = store.lock().await;
            g.get_persona(&persona.id)?
        };
        assert_eq!(from_store, Some(persona));
        Ok(())
    }

    #[tokio::test]
    async fn no_plaintext_persona_fields_on_the_wire() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, lan) = paired_pair(dd.path(), pd.path()).await?;
        let persona = sample_persona();

        desktop.push_persona_to_all(&persona).await?;
        let inbox = lan.take_inbox(phone.public_key())?;
        let frame = &inbox[0];

        // The sealed bytes must not contain any plaintext persona field value.
        let haystack = frame.as_slice();
        for needle in [
            persona.name.as_bytes(),
            persona.region.as_bytes(),
            persona.id.as_bytes(),
            persona.profession.as_bytes(),
            b"PersonaUpsert".as_slice(),
            b"synced note".as_slice(),
        ] {
            assert!(
                !contains_subslice(haystack, needle),
                "plaintext leak: found {:?} in sealed frame",
                String::from_utf8_lossy(needle)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn unpaired_peer_cannot_be_pushed_to() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let desktop = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(dd.path())?),
            Some(Arc::new(FakeLan::new())),
            None,
        );
        let stranger = DeviceIdentity::generate();
        // Never paired => seal/push refused at the API boundary.
        let stranger_pk = encode_public_key(stranger.public_key());
        assert!(matches!(
            desktop
                .seal_persona_for(&stranger_pk, &sample_persona())
                .await,
            Err(CoreError::Sync(_))
        ));
        assert!(matches!(
            desktop
                .push_persona_to(&stranger_pk, &sample_persona())
                .await,
            Err(CoreError::Sync(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn frame_from_unpaired_sender_is_rejected_on_receive() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, _lan) = paired_pair(dd.path(), pd.path()).await?;

        // A stranger seals a valid-looking frame for the phone, but the phone
        // has not paired the stranger, so receive refuses before crypto.
        let stranger = DeviceIdentity::generate();
        let msg = SyncMessage::persona_upsert(sample_persona());
        let env = stranger.seal(phone.public_key(), &msg.to_plaintext()?)?;
        let frame = SealedFrame { envelope: env }.to_bytes();
        let stranger_pk = encode_public_key(stranger.public_key());
        assert!(matches!(
            phone.receive_frame(&stranger_pk, &frame).await,
            Err(CoreError::Sync(_))
        ));

        // Even if the phone *claims* a paired sender (the desktop) for the
        // stranger's frame, the MAC fails to authenticate => rejected.
        assert!(matches!(
            phone
                .receive_frame(&encode_public_key(desktop.public_key()), &frame)
                .await,
            Err(CoreError::Sync(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn signed_artifact_round_trips_through_sealed_channel() -> Result<()> {
        use crate::generate::{
            sign_artifact_with, verify_parsed_artifact, ArtifactContent, ArtifactPayload,
            WeightMap, DEFAULT_FRESHNESS_MS,
        };
        use crate::personapack::{PackSigningKey, PACK_SEED_LEN};

        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, lan) = paired_pair(dd.path(), pd.path()).await?;

        // Build a signed C6 weight-map artifact (a separate artifact-signing key
        // from the device's sealed-channel identity).
        let pack_key = PackSigningKey::from_seed(&[9u8; PACK_SEED_LEN]);
        let mut map = WeightMap::new();
        for c in crate::persona::CategoryPool::all() {
            map.insert(
                c.as_name().to_string(),
                1.0 / crate::persona::CategoryPool::all().len() as f64,
            );
        }
        let content = ArtifactContent::new(
            "66666666-6666-4666-8666-666666666666",
            ArtifactPayload::WeightMap(map),
            1_700_000_000_000,
            DEFAULT_FRESHNESS_MS,
        );
        let artifact =
            sign_artifact_with(content, &pack_key).map_err(|e| CoreError::Sync(e.to_string()))?;

        // Desktop seals the SignedArtifact kind and sends to the phone.
        let msg = SyncMessage::signed_artifact(artifact.clone());
        let sent = desktop.push_message_to_all(&msg).await?;
        assert_eq!(sent, 1);

        // The phone drains its inbox, opens the frame, and recovers the artifact.
        let inbox = lan.take_inbox(phone.public_key())?;
        assert_eq!(inbox.len(), 1);
        let received = phone
            .receive_sync_message(&encode_public_key(desktop.public_key()), &inbox[0])
            .await?;
        match received.body {
            SyncBody::SignedArtifact(got) => {
                assert_eq!(got, artifact);
                // The embedded signature still verifies after the sealed round
                // trip, so the phone would replay it (fresh artifact).
                verify_parsed_artifact(&got).map_err(|e| CoreError::Sync(e.to_string()))?;
            }
            other => {
                return Err(CoreError::Sync(format!(
                    "expected SignedArtifact, got {other:?}"
                )))
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn persona_pack_round_trips_through_sealed_channel() -> Result<()> {
        // The C6 #29 PersonaPack wire kind: a signed minted pack rides the same
        // sealed channel; the receiver verifies (P4) before importing.
        use crate::mint::{mint_pack, mint_personas, PersonaDistribution};
        use crate::personapack::{verify_pack, PackSigningKey, PACK_SEED_LEN};

        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (desktop, phone, lan) = paired_pair(dd.path(), pd.path()).await?;

        let dist = PersonaDistribution::bundled().map_err(|e| CoreError::Sync(e.to_string()))?;
        let minted = mint_personas(&dist, 2, 11, 1_700_000_000_000)
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let pack_key = PackSigningKey::from_seed(&[8u8; PACK_SEED_LEN]);
        let pack = mint_pack(&minted, 1_700_000_000_000, &pack_key)
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let pack_bytes = pack
            .to_bytes()
            .map_err(|e| CoreError::Sync(e.to_string()))?;

        // Desktop seals the PersonaPack kind and sends to the phone.
        let msg = SyncMessage::persona_pack(&pack_bytes);
        let sent = desktop.push_message_to_all(&msg).await?;
        assert_eq!(sent, 1);

        // The phone drains its inbox, opens the frame, recovers + verifies the pack.
        let inbox = lan.take_inbox(phone.public_key())?;
        assert_eq!(inbox.len(), 1);
        let received = phone
            .receive_sync_message(&encode_public_key(desktop.public_key()), &inbox[0])
            .await?;
        match received.body {
            SyncBody::PersonaPack(body) => {
                let decoded = body.pack_bytes()?;
                let verified = verify_pack(&decoded).map_err(|e| CoreError::Sync(e.to_string()))?;
                assert_eq!(verified.content.personas, minted.personas);
            }
            other => {
                return Err(CoreError::Sync(format!(
                    "expected PersonaPack, got {other:?}"
                )))
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn identity_persists_across_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let src = passphrase_source(dir.path());
        let store = open_store(dir.path())?;

        let first = LanSync::open(
            &src,
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            store.clone(),
        )?;
        let pk1 = *first.public_key();

        let second = LanSync::open(&src, "Desktop".to_string(), DEFAULT_SYNC_PORT, store)?;
        // The keypair was persisted and reloaded, not regenerated.
        assert_eq!(*second.public_key(), pk1);
        Ok(())
    }

    #[test]
    fn debug_does_not_leak_secret() {
        let id = DeviceIdentity::generate();
        let sync = LanSync::with_parts(
            id,
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            None,
            None,
            None,
        );
        let rendered = format!("{sync:?}");
        assert!(rendered.contains("Desktop"));
        assert!(rendered.contains("fingerprint"));
        // The secret bytes are never formatted; only the public fingerprint is.
    }

    /// Naive substring search over bytes (no extra dependency).
    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || needle.len() > haystack.len() {
            return false;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
