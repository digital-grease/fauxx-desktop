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

//! Cross-device persona orchestration (C1 milestone C1, issues #8/#9/#10).
//!
//! This module sits on top of the O1 [`sync`](crate::sync) foundation and adds
//! the three household-coordination layers:
//!
//! - [`mode`] (O2, #8): explicit, persisted [`CoordinationMode`] plus per-device
//!   persona assignment. Coherent pins ONE persona and propagates it to every
//!   paired device over the O1 sealed channel; Fragmentation assigns each device
//!   a DISTINCT persona with independent timing.
//! - [`wan_ip`] (O3, #9): shared-public-IP detection over the O1 channel, with a
//!   [`PublicIpSource`] seam (default [`UnknownPublicIp`]); shared IP is
//!   reinforcing under Coherent and a linkage RISK under Fragmentation.
//! - [`scheduler`] (O4, #10): a household timeline scheduler that aggregates
//!   per-device intents into ONE Poisson-like, circadian stream and prevents
//!   cross-device collisions, degrading to local-only when a peer is offline.
//!
//! All coordination state travels over the O1 sealed channel (new
//! [`SyncBody`](crate::sync::SyncBody) variants; the wire format and security
//! model live in [`crate::sync::wire`] and [`crate::sync`]) and persists in the
//! encrypted store. The engine
//! reuses the frozen [`SyntheticPersona`] unit and its 8-to-10-day rotation
//! window; it never invents a new persona schema or rotation cadence.

pub mod mode;
pub mod scheduler;
pub mod wan_ip;

use std::sync::Arc;

use tokio::sync::Mutex;

pub use mode::{CoordinationMode, DeviceAssignment};
pub use scheduler::{
    has_cross_device_collision, is_active_window, plan_household_day, DeviceIntent, IntensityLevel,
    PlanConfig, ScheduledAction,
};
pub use wan_ip::{
    assess, recommend, shared_ip_state, FixedPublicIp, IpRecommendation, PublicIpSource,
    SharedIpState, UnknownPublicIp, WanIpAssessment,
};

use crate::error::{CoreError, Result};
use crate::persona::SyntheticPersona;
use crate::store::EncryptedStore;
use crate::sync::{encode_public_key, LanSync};

use self::mode::{MODE_KEY, SELF_DEVICE_KEY};

/// The household orchestrator: the engine behind the C1 #8/#9/#10 API.
///
/// Cheap to clone (shared state is behind an `Arc`). Reaches the persona cache,
/// the paired-peer set, and the orchestration tables through the same encrypted
/// store the rest of the core uses; propagates coordination state over the O1
/// [`LanSync`] sealed channel; and observes this device's public IP through an
/// injectable [`PublicIpSource`] (default [`UnknownPublicIp`], so the core makes
/// no network call).
#[derive(Clone)]
pub struct HouseholdOrchestrator {
    inner: Arc<OrchestratorInner>,
}

struct OrchestratorInner {
    store: Arc<Mutex<EncryptedStore>>,
    sync: LanSync,
    ip_source: Arc<dyn PublicIpSource>,
}

impl std::fmt::Debug for HouseholdOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HouseholdOrchestrator")
            .field("self_key", &self.self_key())
            .finish_non_exhaustive()
    }
}

impl HouseholdOrchestrator {
    /// Build an orchestrator over a shared store and the O1 sync engine, with
    /// the default (unknown) public-IP source.
    pub fn new(store: Arc<Mutex<EncryptedStore>>, sync: LanSync) -> Self {
        Self::with_ip_source(store, sync, Arc::new(UnknownPublicIp))
    }

    /// Build an orchestrator with an explicit public-IP source. Tests inject a
    /// [`FixedPublicIp`]; production wires STUN or the C7 egress layer here.
    pub fn with_ip_source(
        store: Arc<Mutex<EncryptedStore>>,
        sync: LanSync,
        ip_source: Arc<dyn PublicIpSource>,
    ) -> Self {
        Self {
            inner: Arc::new(OrchestratorInner {
                store,
                sync,
                ip_source,
            }),
        }
    }

    /// This device's own base64url public key (the identity it presents to
    /// peers). Used to attribute the local device in assignments and intents.
    pub fn self_key(&self) -> String {
        encode_public_key(self.inner.sync.public_key())
    }

    // --- O2 (#8): coordination mode ----------------------------------------

    /// The active [`CoordinationMode`], defaulting to
    /// [`CoordinationMode::CoherentHousehold`] when none has been set yet.
    pub async fn coordination_mode(&self) -> Result<CoordinationMode> {
        let raw = self
            .with_store(|s| s.get_orchestration_value(MODE_KEY))
            .await?;
        match raw {
            Some(s) => CoordinationMode::from_str_strict(&s),
            None => Ok(CoordinationMode::default()),
        }
    }

    /// Set the active coordination mode (persisted; survives restart).
    pub async fn set_coordination_mode(&self, mode: CoordinationMode) -> Result<()> {
        let value = mode.as_str().to_string();
        self.with_store(|s| s.put_orchestration_value(MODE_KEY, &value))
            .await
    }

    // --- O2 (#8): per-device persona assignment ----------------------------

    /// Pin a persona id to a device (the empty key is this device). The persona
    /// MUST already exist in the store; assigning a missing persona fails.
    pub async fn assign_persona(&self, device_key: &str, persona_id: &str) -> Result<()> {
        let now = now_millis();
        let persona_id = persona_id.to_string();
        let device_key = device_key.to_string();
        self.with_store(move |s| {
            if s.get_persona(&persona_id)?.is_none() {
                return Err(CoreError::Orchestration(format!(
                    "cannot assign unknown persona {persona_id}"
                )));
            }
            s.put_device_assignment(&device_key, &persona_id, now)
        })
        .await
    }

    /// The persona id assigned to a device (empty key = this device), or `None`.
    pub async fn assigned_persona(&self, device_key: &str) -> Result<Option<String>> {
        let device_key = device_key.to_string();
        self.with_store(move |s| s.get_device_assignment(&device_key))
            .await
    }

    /// Every device-to-persona assignment, with the local device flagged.
    pub async fn assignments(&self) -> Result<Vec<DeviceAssignment>> {
        let self_key = self.self_key();
        let raw = self.with_store(|s| s.list_device_assignments()).await?;
        Ok(raw
            .into_iter()
            .map(|(device_key, persona_id)| {
                let is_self = device_key == SELF_DEVICE_KEY || device_key == self_key;
                DeviceAssignment {
                    device_key,
                    is_self,
                    persona_id,
                }
            })
            .collect())
    }

    /// Elect ONE persona for the whole household (Coherent mode) and propagate
    /// it to every paired device over the O1 sealed channel.
    ///
    /// This is the coherent-converge operation: it records the elected persona
    /// as the assignment for this device AND every paired peer (so a query
    /// reports the household as converged), then pushes the persona to all
    /// paired peers over the sealed channel so they cache the same unit and
    /// advance together on rotation (the frozen `activeUntil` cadence is mirrored
    /// verbatim; nothing here changes the 8-to-10-day window). Returns the number
    /// of peers the persona was sealed and sent to. Errors unless the active mode
    /// is Coherent.
    pub async fn elect_coherent_persona(&self, persona_id: &str) -> Result<usize> {
        if self.coordination_mode().await? != CoordinationMode::CoherentHousehold {
            return Err(CoreError::Orchestration(
                "elect_coherent_persona requires CoherentHousehold mode".to_string(),
            ));
        }
        // The persona must exist; load it for propagation.
        let persona_id_owned = persona_id.to_string();
        let persona = self
            .with_store(move |s| s.get_persona(&persona_id_owned))
            .await?
            .ok_or_else(|| {
                CoreError::Orchestration(format!("cannot elect unknown persona {persona_id}"))
            })?;

        // Propagate the elected persona to every paired peer over O1 FIRST, then
        // record the converged assignments ONLY if every push succeeded. Order
        // matters: recording first (the prior bug) meant a push failure left local
        // state falsely claiming the household had converged while peers still ran
        // the old persona and would rotate on a different schedule, breaking the
        // coherent contract. We read the peer set ONCE and use it for both the
        // push and the assignment write, so the two cannot drift.
        let peers = self.inner.sync.paired_peers().await?;
        for peer in &peers {
            // `push_persona_to` errors on the first unreachable peer; on error we
            // return WITHOUT recording any assignment (fail closed: no false
            // convergence). Peers reached before a later failure simply
            // re-converge on the next successful election.
            self.inner
                .sync
                .push_persona_to(&peer.public_key, &persona)
                .await?;
        }

        // Every paired peer received the elected persona: record the converged
        // assignment for this device and each peer, so an `assignments()` query
        // shows the household pinned to one persona.
        let now = now_millis();
        let persona_id_s = persona_id.to_string();
        let peer_keys: Vec<String> = peers.iter().map(|p| p.public_key.clone()).collect();
        self.with_store(move |s| {
            s.put_device_assignment(SELF_DEVICE_KEY, &persona_id_s, now)?;
            for key in &peer_keys {
                s.put_device_assignment(key, &persona_id_s, now)?;
            }
            Ok(())
        })
        .await?;

        Ok(peers.len())
    }

    /// Reconcile the elected coherent persona to its rotated successor across
    /// the household: cache the (already-rotated) persona locally and propagate
    /// it to every paired device so they advance together at the phone's
    /// frozen 8-to-10-day cadence.
    ///
    /// The caller supplies the rotated [`SyntheticPersona`] (with its new
    /// `createdAt`/`activeUntil`); this engine does not re-derive the rotation
    /// window, it mirrors what the persona carries. Errors unless Coherent.
    pub async fn reconcile_coherent_rotation(&self, rotated: &SyntheticPersona) -> Result<usize> {
        if self.coordination_mode().await? != CoordinationMode::CoherentHousehold {
            return Err(CoreError::Orchestration(
                "reconcile_coherent_rotation requires CoherentHousehold mode".to_string(),
            ));
        }
        // Cache the rotated unit locally, then re-elect it household-wide.
        let persona = rotated.clone();
        self.with_store(move |s| s.save_persona(&persona)).await?;
        self.elect_coherent_persona(&rotated.id).await
    }

    /// Assign a DISTINCT persona to a paired device (Fragmentation mode). The
    /// persona must already exist. This is the per-device-divergence operation
    /// that O3/O4 consume; it does NOT propagate the persona to other devices
    /// (each device keeps its own). Errors unless the active mode is
    /// Fragmentation, or if the persona is already assigned to another device
    /// (distinctness is the whole point).
    pub async fn assign_fragmented_persona(
        &self,
        device_key: &str,
        persona_id: &str,
    ) -> Result<()> {
        if self.coordination_mode().await? != CoordinationMode::Fragmentation {
            return Err(CoreError::Orchestration(
                "assign_fragmented_persona requires Fragmentation mode".to_string(),
            ));
        }
        let device_key_owned = device_key.to_string();
        let persona_id_owned = persona_id.to_string();
        self.with_store(move |s| {
            if s.get_persona(&persona_id_owned)?.is_none() {
                return Err(CoreError::Orchestration(format!(
                    "cannot assign unknown persona {persona_id_owned}"
                )));
            }
            // Distinctness: no other device may already hold this persona.
            for (other_device, other_persona) in s.list_device_assignments()? {
                if other_persona == persona_id_owned && other_device != device_key_owned {
                    return Err(CoreError::Orchestration(format!(
                        "persona {persona_id_owned} is already assigned to another device; fragmentation requires distinct personas"
                    )));
                }
            }
            s.put_device_assignment(&device_key_owned, &persona_id_owned, now_millis())
        })
        .await
    }

    // --- O3 (#9): WAN-IP awareness -----------------------------------------

    /// Observe this device's public IP through the injected [`PublicIpSource`]
    /// and record it under this device's key in the store. Returns the observed
    /// IP (`None` when the source reports unknown).
    pub async fn observe_local_public_ip(&self) -> Result<Option<String>> {
        let ip = self.inner.ip_source.observe_public_ip().await?;
        let self_key = self.self_key();
        let now = now_millis();
        let ip_for_store = ip.clone();
        self.with_store(move |s| s.put_device_ip(&self_key, ip_for_store.as_deref(), now))
            .await?;
        Ok(ip)
    }

    /// Record a peer's reported public IP (received over the O1 channel as a
    /// [`PublicIpReport`](crate::sync::SyncBody) kind, then handed here).
    pub async fn record_peer_public_ip(&self, peer_key: &str, ip: Option<&str>) -> Result<()> {
        let peer_key = peer_key.to_string();
        let ip = ip.map(|s| s.to_string());
        let now = now_millis();
        self.with_store(move |s| s.put_device_ip(&peer_key, ip.as_deref(), now))
            .await
    }

    /// Observe this device's public IP and SHARE it with every paired peer over
    /// the O1 sealed channel (a [`SyncBody::PublicIpReport`] frame). Returns the
    /// number of peers it was sent to. The report is peer-to-peer; no third
    /// party sees it. Degrades to a no-op (0 peers) when no peer is reachable.
    pub async fn share_public_ip_with_peers(&self) -> Result<usize> {
        let ip = self.observe_local_public_ip().await?;
        let message = crate::sync::SyncMessage::public_ip_report(ip, now_millis());
        self.inner.sync.push_message_to_all(&message).await
    }

    /// Receive a sealed coordination frame from a paired sender, open and verify
    /// it (same enforcement as persona sync: unpaired/forged/tampered frames are
    /// rejected), and APPLY it: a [`SyncBody::PublicIpReport`] records the
    /// sender's IP for linkage assessment; a [`SyncBody::CoordinationState`]
    /// records the asserted assignment; a [`SyncBody::PersonaUpsert`] caches the
    /// persona. Returns the kind name applied.
    pub async fn apply_sync_frame(&self, sender_key: &str, frame: &[u8]) -> Result<&'static str> {
        let message = self
            .inner
            .sync
            .receive_sync_message(sender_key, frame)
            .await?;
        match message.body {
            crate::sync::SyncBody::PublicIpReport(report) => {
                self.record_peer_public_ip(sender_key, report.public_ip.as_deref())
                    .await?;
                Ok("PublicIpReport")
            }
            crate::sync::SyncBody::CoordinationState(state) => {
                // Validate the mode string fails closed on an unknown value.
                let _ = CoordinationMode::from_str_strict(&state.mode)?;
                let sender_key = sender_key.to_string();
                self.with_store(move |s| {
                    s.put_device_assignment(&sender_key, &state.persona_id, now_millis())
                })
                .await?;
                Ok("CoordinationState")
            }
            crate::sync::SyncBody::PersonaUpsert(persona) => {
                self.with_store(move |s| s.save_persona(&persona)).await?;
                Ok("PersonaUpsert")
            }
            // The C6 signed generated artifact (#28 H1) and signed persona pack
            // (#29 H2) are NOT orchestration concerns: each is verified and applied
            // through its own dedicated path (`Core::receive_artifact_frame` checks
            // the artifact signature + freshness before replay;
            // `Core::receive_pack_frame` verifies the pack signature before import).
            // Reaching either here means the frame was routed to the wrong handler;
            // fail closed rather than silently dropping it.
            other @ (crate::sync::SyncBody::SignedArtifact(_)
            | crate::sync::SyncBody::PersonaPack(_)) => Err(CoreError::Orchestration(format!(
                "apply_sync_frame does not handle {}; route it through its dedicated receive path",
                other.kind_name()
            ))),
        }
    }

    /// Assess shared-public-IP linkage between this device and a paired peer,
    /// under the active coordination mode. Uses the last observed IPs in the
    /// store (so the caller must have observed locally and recorded the peer's
    /// report). A missing observation reads as unknown (fail open to
    /// "undetermined", never a false "shared").
    pub async fn assess_shared_ip(&self, peer_key: &str) -> Result<WanIpAssessment> {
        let mode = self.coordination_mode().await?;
        let self_key = self.self_key();
        let peer_key = peer_key.to_string();
        let (local, peer) = self
            .with_store(move |s| {
                let local = s.get_device_ip(&self_key)?.flatten();
                let peer = s.get_device_ip(&peer_key)?.flatten();
                Ok((local, peer))
            })
            .await?;
        Ok(wan_ip::assess(mode, local, peer))
    }

    // --- O4 (#10): household timeline scheduler ----------------------------

    /// Plan the household's action timeline for one active day from the given
    /// per-device intents, under the active mode and the supplied collision
    /// window. Deterministic for a fixed `seed`. Degrades to local-only (just
    /// the intents present) when a peer is offline, with no stall.
    pub async fn plan_household(
        &self,
        intents: &[DeviceIntent],
        collision_window_secs: i64,
        seed: u64,
    ) -> Result<Vec<ScheduledAction>> {
        let mode = self.coordination_mode().await?;
        let config = PlanConfig::new(mode).with_collision_window_secs(collision_window_secs);
        plan_household_day(intents, config, seed)
    }

    // --- internals ----------------------------------------------------------

    /// Run a closure against the locked store.
    async fn with_store<T>(&self, f: impl FnOnce(&EncryptedStore) -> Result<T>) -> Result<T> {
        f(&*self.inner.store.lock().await)
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
    use crate::store::KeySource;
    use crate::sync::transport::testing::{FailingTransport, FakeLan};
    use crate::sync::{DeviceIdentity, DEFAULT_SYNC_PORT};
    use std::path::Path;

    fn passphrase_source(dir: &Path) -> KeySource {
        KeySource::EncryptedFile {
            path: dir.join("key.bin"),
            passphrase: "orch-test-passphrase".to_string(),
        }
    }

    fn open_store(dir: &Path) -> Result<Arc<Mutex<EncryptedStore>>> {
        let store = EncryptedStore::open_at(&dir.join("fauxx.db"), &passphrase_source(dir))?;
        Ok(Arc::new(Mutex::new(store)))
    }

    fn persona(id: &str, name: &str, created: i64, active_until: i64) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            name.to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            vec![
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
                CategoryPool::GAMING.as_name().to_string(),
            ],
            created,
            active_until,
        )
    }

    /// Build a desktop + phone pair sharing a FakeLan, both paired, with an
    /// orchestrator over the desktop.
    async fn paired(
        desktop_dir: &Path,
        phone_dir: &Path,
        ip: Arc<dyn PublicIpSource>,
    ) -> Result<(HouseholdOrchestrator, LanSync, LanSync, FakeLan)> {
        let lan = FakeLan::new();
        let desktop_store = open_store(desktop_dir)?;
        let desktop_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::clone(&desktop_store)),
            Some(Arc::new(lan.clone())),
            None,
        );
        let phone_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Phone".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(phone_dir)?),
            Some(Arc::new(lan.clone())),
            None,
        );
        desktop_sync
            .complete_pairing(&phone_sync.pairing_payload())
            .await?;
        phone_sync
            .complete_pairing(&desktop_sync.pairing_payload())
            .await?;
        let orch = HouseholdOrchestrator::with_ip_source(desktop_store, desktop_sync.clone(), ip);
        Ok((orch, desktop_sync, phone_sync, lan))
    }

    #[tokio::test]
    async fn mode_defaults_to_coherent_and_persists() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, desktop_sync, _phone, _lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        assert_eq!(
            orch.coordination_mode().await?,
            CoordinationMode::CoherentHousehold
        );
        orch.set_coordination_mode(CoordinationMode::Fragmentation)
            .await?;
        assert_eq!(
            orch.coordination_mode().await?,
            CoordinationMode::Fragmentation
        );

        // Reopen over the SAME store and identity: the mode survives restart.
        let reopened_store = open_store(dd.path())?;
        let reopened = HouseholdOrchestrator::new(reopened_store, desktop_sync);
        assert_eq!(
            reopened.coordination_mode().await?,
            CoordinationMode::Fragmentation
        );
        Ok(())
    }

    #[tokio::test]
    async fn coherent_elect_converges_and_propagates() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, desktop_sync, phone_sync, lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;

        let p = persona(
            "10000000-0000-4000-8000-000000000001",
            "Shared",
            1_700_000_000_000,
            1_700_600_000_000,
        );
        orch.with_store({
            let p = p.clone();
            move |s| s.save_persona(&p)
        })
        .await?;

        let sent = orch.elect_coherent_persona(&p.id).await?;
        assert_eq!(sent, 1);

        // Both this device and the paired peer are pinned to the one persona.
        let assigns = orch.assignments().await?;
        assert_eq!(assigns.len(), 2);
        assert!(assigns.iter().all(|a| a.persona_id == p.id));
        assert!(assigns.iter().any(|a| a.is_self));

        // The phone received and cached the same unit (advances together).
        let phone_key = encode_public_key(desktop_sync.public_key());
        let inbox = lan.take_inbox(phone_sync.public_key())?;
        assert_eq!(inbox.len(), 1);
        let received = phone_sync.receive_frame(&phone_key, &inbox[0]).await?;
        assert_eq!(received.active_until, p.active_until);
        assert_eq!(received, p);
        Ok(())
    }

    #[tokio::test]
    async fn coherent_elect_fails_closed_when_push_fails() -> Result<()> {
        // Regression: a push failure must NOT leave local state claiming the
        // household converged. The desktop's transport always fails to send, so
        // `elect_coherent_persona` must error AND record no device assignment.
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let desktop_store = open_store(dd.path())?;
        let desktop_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::clone(&desktop_store)),
            Some(Arc::new(FailingTransport)),
            None,
        );
        let phone_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Phone".to_string(),
            DEFAULT_SYNC_PORT,
            Some(open_store(pd.path())?),
            Some(Arc::new(FailingTransport)),
            None,
        );
        // Pairing does not go through the transport, so both still pair.
        desktop_sync
            .complete_pairing(&phone_sync.pairing_payload())
            .await?;
        phone_sync
            .complete_pairing(&desktop_sync.pairing_payload())
            .await?;
        let orch = HouseholdOrchestrator::new(Arc::clone(&desktop_store), desktop_sync);

        // Coherent mode is the default. Save a persona to elect.
        let p = persona(
            "10000000-0000-4000-8000-0000000000ff",
            "Shared",
            1_700_000_000_000,
            1_700_600_000_000,
        );
        orch.with_store({
            let p = p.clone();
            move |s| s.save_persona(&p)
        })
        .await?;

        // The push to the peer fails, so the election fails closed...
        let result = orch.elect_coherent_persona(&p.id).await;
        assert!(
            result.is_err(),
            "elect must fail when the push to peers fails"
        );

        // ...and CRUCIALLY records no convergence: no device is pinned to the
        // elected persona (the prior bug recorded assignments BEFORE the push).
        let assigns = orch.assignments().await?;
        assert!(
            assigns.iter().all(|a| a.persona_id != p.id),
            "no device may be pinned to the elected persona after a failed push"
        );
        Ok(())
    }

    #[tokio::test]
    async fn coherent_rotation_reconciles_all_devices() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _desktop, phone_sync, lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;

        let p = persona(
            "10000000-0000-4000-8000-000000000002",
            "Rot",
            1_700_000_000_000,
            1_700_600_000_000,
        );
        orch.with_store({
            let p = p.clone();
            move |s| s.save_persona(&p)
        })
        .await?;
        orch.elect_coherent_persona(&p.id).await?;
        let _ = lan.take_inbox(phone_sync.public_key())?; // drain initial push

        // Rotate: same id, advanced window (mirrors the phone's frozen cadence).
        let mut rotated = p.clone();
        rotated.active_until = p.active_until + 9 * 86_400_000;
        rotated.created_at = p.active_until;
        let sent = orch.reconcile_coherent_rotation(&rotated).await?;
        assert_eq!(sent, 1);

        // The peer gets the rotated unit, advancing in lockstep.
        let inbox = lan.take_inbox(phone_sync.public_key())?;
        assert_eq!(inbox.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn fragmentation_assigns_distinct_personas() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, desktop_sync, phone_sync, _lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        orch.set_coordination_mode(CoordinationMode::Fragmentation)
            .await?;

        let desktop_persona = persona(
            "20000000-0000-4000-8000-000000000001",
            "Desk",
            1_700_000_000_000,
            1_700_600_000_000,
        );
        let phone_persona = persona(
            "20000000-0000-4000-8000-000000000002",
            "Fone",
            1_700_000_000_000,
            1_700_600_000_000,
        );
        let _ = desktop_sync; // keep the desktop identity alive for self_key
        let phone_key = encode_public_key(phone_sync.public_key());
        orch.with_store({
            let a = desktop_persona.clone();
            let b = phone_persona.clone();
            move |s| {
                s.save_persona(&a)?;
                s.save_persona(&b)
            }
        })
        .await?;

        orch.assign_fragmented_persona("", &desktop_persona.id)
            .await?;
        orch.assign_fragmented_persona(&phone_key, &phone_persona.id)
            .await?;

        let assigns = orch.assignments().await?;
        assert_eq!(assigns.len(), 2);
        // Distinct personas per device.
        let ids: std::collections::HashSet<_> =
            assigns.iter().map(|a| a.persona_id.clone()).collect();
        assert_eq!(ids.len(), 2);

        // Re-assigning the SAME persona to a different device is refused.
        assert!(matches!(
            orch.assign_fragmented_persona(&phone_key, &desktop_persona.id)
                .await,
            Err(CoreError::Orchestration(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn assign_rejects_unknown_persona() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _d, _p, _lan) = paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        assert!(matches!(
            orch.assign_persona("", "no-such-persona").await,
            Err(CoreError::Orchestration(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn shared_ip_is_expected_in_coherent() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        // Local device observes a fixed IP.
        let (orch, _desktop, phone_sync, _lan) = paired(
            dd.path(),
            pd.path(),
            Arc::new(FixedPublicIp::new("203.0.113.7")),
        )
        .await?;

        let observed = orch.observe_local_public_ip().await?;
        assert_eq!(observed.as_deref(), Some("203.0.113.7"));
        // Peer reports the SAME IP over the channel.
        let phone_key = encode_public_key(phone_sync.public_key());
        orch.record_peer_public_ip(&phone_key, Some("203.0.113.7"))
            .await?;

        let a = orch.assess_shared_ip(&phone_key).await?;
        assert_eq!(a.shared, SharedIpState::Shared);
        assert_eq!(a.recommendation, IpRecommendation::ConsistentExpected);
        Ok(())
    }

    #[tokio::test]
    async fn shared_ip_is_linkage_risk_in_fragmentation() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _desktop, phone_sync, _lan) = paired(
            dd.path(),
            pd.path(),
            Arc::new(FixedPublicIp::new("203.0.113.7")),
        )
        .await?;
        orch.set_coordination_mode(CoordinationMode::Fragmentation)
            .await?;

        orch.observe_local_public_ip().await?;
        let phone_key = encode_public_key(phone_sync.public_key());
        orch.record_peer_public_ip(&phone_key, Some("203.0.113.7"))
            .await?;

        let a = orch.assess_shared_ip(&phone_key).await?;
        assert_eq!(a.shared, SharedIpState::Shared);
        assert_eq!(a.recommendation, IpRecommendation::LinkageRisk);
        assert!(a.detail.contains("LTE"));
        Ok(())
    }

    #[tokio::test]
    async fn unknown_ip_source_yields_undetermined() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _desktop, phone_sync, _lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        // Default source: local IP unknown.
        assert_eq!(orch.observe_local_public_ip().await?, None);
        let phone_key = encode_public_key(phone_sync.public_key());
        orch.record_peer_public_ip(&phone_key, Some("203.0.113.7"))
            .await?;
        let a = orch.assess_shared_ip(&phone_key).await?;
        assert_eq!(a.shared, SharedIpState::Unknown);
        assert_eq!(a.recommendation, IpRecommendation::Undetermined);
        Ok(())
    }

    #[tokio::test]
    async fn public_ip_report_travels_over_sealed_channel() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        // Desktop has a fixed IP; phone gets an orchestrator over its own store
        // so it can apply the received report.
        let lan = FakeLan::new();
        let desktop_store = open_store(dd.path())?;
        let desktop_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Desktop".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::clone(&desktop_store)),
            Some(Arc::new(lan.clone())),
            None,
        );
        let phone_store = open_store(pd.path())?;
        let phone_sync = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Phone".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::clone(&phone_store)),
            Some(Arc::new(lan.clone())),
            None,
        );
        desktop_sync
            .complete_pairing(&phone_sync.pairing_payload())
            .await?;
        phone_sync
            .complete_pairing(&desktop_sync.pairing_payload())
            .await?;

        let desktop = HouseholdOrchestrator::with_ip_source(
            desktop_store,
            desktop_sync.clone(),
            Arc::new(FixedPublicIp::new("203.0.113.7")),
        );
        let phone = HouseholdOrchestrator::with_ip_source(
            phone_store,
            phone_sync.clone(),
            Arc::new(FixedPublicIp::new("203.0.113.7")),
        );

        // Desktop shares its IP over the sealed channel.
        let sent = desktop.share_public_ip_with_peers().await?;
        assert_eq!(sent, 1);

        // The phone drains and applies the frame, then assesses linkage.
        let desktop_key = encode_public_key(desktop_sync.public_key());
        let inbox = lan.take_inbox(phone_sync.public_key())?;
        assert_eq!(inbox.len(), 1);
        let kind = phone.apply_sync_frame(&desktop_key, &inbox[0]).await?;
        assert_eq!(kind, "PublicIpReport");

        // Phone observes its own (same) IP, then assesses: under Fragmentation
        // the shared IP is a linkage risk.
        phone
            .set_coordination_mode(CoordinationMode::Fragmentation)
            .await?;
        phone.observe_local_public_ip().await?;
        let a = phone.assess_shared_ip(&desktop_key).await?;
        assert_eq!(a.shared, SharedIpState::Shared);
        assert_eq!(a.recommendation, IpRecommendation::LinkageRisk);
        Ok(())
    }

    #[tokio::test]
    async fn household_plan_is_deterministic_and_collision_free() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _desktop, _phone, _lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        orch.set_coordination_mode(CoordinationMode::Fragmentation)
            .await?;

        let intents = vec![
            DeviceIntent::new("", "p-desk", IntensityLevel::High),
            DeviceIntent::new("phone", "p-phone", IntensityLevel::High),
        ];
        let a = orch.plan_household(&intents, 3, 77).await?;
        let b = orch.plan_household(&intents, 3, 77).await?;
        assert_eq!(a, b);
        assert!(!has_cross_device_collision(&a, 3));
        for action in &a {
            assert!(is_active_window(action.at_secs));
        }
        Ok(())
    }

    #[tokio::test]
    async fn household_plan_degrades_when_peer_offline() -> Result<()> {
        let dd = tempfile::tempdir()?;
        let pd = tempfile::tempdir()?;
        let (orch, _desktop, _phone, _lan) =
            paired(dd.path(), pd.path(), Arc::new(UnknownPublicIp)).await?;
        // Only the local device is present (peer offline): plan still produced.
        let intents = vec![DeviceIntent::new("", "p-desk", IntensityLevel::Medium)];
        let plan = orch.plan_household(&intents, 2, 5).await?;
        assert!(!plan.is_empty());
        assert!(plan.iter().all(|a| a.device_key.is_empty()));
        // No peers at all: empty intents => empty plan, no stall.
        let empty = orch.plan_household(&[], 2, 5).await?;
        assert!(empty.is_empty());
        Ok(())
    }
}
