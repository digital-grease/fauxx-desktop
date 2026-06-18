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

//! WAN-IP (shared public IP) awareness (C1 #9, O3).
//!
//! Two devices that egress from the same public IP look, to any observer that
//! sees the source address (an ad network, a tracker), like one network. In
//! [`CoherentHousehold`](super::CoordinationMode::CoherentHousehold) that is
//! expected and reinforcing: one household, one identity, one IP. In
//! [`Fragmentation`](super::CoordinationMode::Fragmentation) it is a LINKAGE
//! RISK: two personas meant to look unrelated share a network-layer
//! correlator, so the recommendation is to diverge egress (route the phone via
//! LTE while the desktop keeps the home IP), coordinating with per-persona
//! egress (C7) where available.
//!
//! ## The public-IP source seam
//!
//! A device behind NAT cannot know its own public IP without SOME external
//! observation. Rather than bake in a STUN client or a third-party echo
//! (against the no-backend posture), the source is abstracted behind the
//! [`PublicIpSource`] trait, mirroring the O1 [`Transport`]/[`Discovery`] seam.
//! The default concrete implementation, [`UnknownPublicIp`], reports
//! [`None`](Option::None) (unknown). Real detection (a STUN probe, or the C7
//! egress layer which already terminates the connection and thus observes the
//! public source) plugs in here as another [`PublicIpSource`]; tests inject a
//! fixed IP with [`FixedPublicIp`]. The core itself makes no network call.
//!
//! Each device shares its OBSERVED public IP with paired peers over the O1
//! sealed channel (a new [`SyncBody`](crate::sync::SyncBody) kind), so the
//! comparison is peer-to-peer and never relies on a shared third party.
//!
//! [`Transport`]: crate::sync::SealedTransport

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::CoordinationMode;
use crate::error::Result;

/// A source of this device's currently observed public (WAN) IP.
///
/// Object-safe and async, like the O1 transport seam, so a real STUN/egress
/// backend or a test double can be injected without the core taking a network
/// dependency. `None` means "not observed / unknown"; the core never guesses.
#[async_trait]
pub trait PublicIpSource: Send + Sync {
    /// Best-effort observation of this device's current public IP, as a string
    /// (e.g. `"203.0.113.7"`). `None` when unknown. MUST NOT block on a network
    /// call from inside the core's default build; real detection is a separate,
    /// injectable backend.
    async fn observe_public_ip(&self) -> Result<Option<String>>;
}

/// The default public-IP source: always [`None`] (unknown).
///
/// This is what ships by default so the core makes no network call. Real
/// detection (STUN or the C7 egress layer) replaces it by injecting another
/// [`PublicIpSource`].
#[derive(Debug, Clone, Copy, Default)]
pub struct UnknownPublicIp;

#[async_trait]
impl PublicIpSource for UnknownPublicIp {
    async fn observe_public_ip(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

/// A public-IP source that always reports a fixed IP. Used in tests (and by
/// callers that already know their egress IP, e.g. a static homelab) to inject
/// an observation without any network call.
#[derive(Debug, Clone)]
pub struct FixedPublicIp {
    ip: Option<String>,
}

impl FixedPublicIp {
    /// A source that reports the given IP.
    pub fn new(ip: impl Into<String>) -> Self {
        Self {
            ip: Some(ip.into()),
        }
    }

    /// A source that reports "unknown" (equivalent to [`UnknownPublicIp`], but
    /// constructible alongside other [`FixedPublicIp`] sources in tests).
    pub fn unknown() -> Self {
        Self { ip: None }
    }
}

#[async_trait]
impl PublicIpSource for FixedPublicIp {
    async fn observe_public_ip(&self) -> Result<Option<String>> {
        Ok(self.ip.clone())
    }
}

/// Whether two devices share a public IP, accounting for unknowns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SharedIpState {
    /// Both IPs are known and equal: the devices share a public IP.
    Shared,
    /// Both IPs are known and differ: the devices egress separately.
    Distinct,
    /// At least one IP is unknown, so sharing cannot be determined.
    Unknown,
}

/// How a [`SharedIpState`] reads under the active [`CoordinationMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum IpRecommendation {
    /// Coherent mode, shared IP: expected and reinforcing. Keep signals
    /// consistent across devices.
    ConsistentExpected,
    /// Fragmentation mode, shared IP: a network-layer linkage RISK. Diverge
    /// egress (e.g. route the phone via LTE while the desktop keeps the home
    /// IP), coordinating with per-persona egress (C7) where available.
    LinkageRisk,
    /// Distinct IPs: appropriate for fragmentation; harmless under coherent.
    Independent,
    /// Sharing is unknown (no public-IP detection wired). No action; wire a
    /// [`PublicIpSource`] to assess linkage.
    Undetermined,
}

/// The full WAN-IP awareness verdict surfaced over the core API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WanIpAssessment {
    /// This device's observed public IP, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_ip: Option<String>,
    /// The peer's reported public IP, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ip: Option<String>,
    /// Whether the two devices share a public IP.
    pub shared: SharedIpState,
    /// The mode-aware recommendation derived from `shared`.
    pub recommendation: IpRecommendation,
    /// A short, human-readable explanation a client can show verbatim.
    pub detail: String,
}

/// Compare two observed public IPs into a [`SharedIpState`].
pub fn shared_ip_state(local: Option<&str>, peer: Option<&str>) -> SharedIpState {
    match (local, peer) {
        (Some(a), Some(b)) if a == b => SharedIpState::Shared,
        (Some(_), Some(_)) => SharedIpState::Distinct,
        _ => SharedIpState::Unknown,
    }
}

/// Build the full mode-aware assessment from a pair of observations.
pub fn assess(
    mode: CoordinationMode,
    local: Option<String>,
    peer: Option<String>,
) -> WanIpAssessment {
    let shared = shared_ip_state(local.as_deref(), peer.as_deref());
    let recommendation = recommend(mode, shared);
    let detail = describe(mode, shared, local.as_deref(), peer.as_deref());
    WanIpAssessment {
        local_ip: local,
        peer_ip: peer,
        shared,
        recommendation,
        detail,
    }
}

/// Map (mode, shared-state) to the recommendation.
pub fn recommend(mode: CoordinationMode, shared: SharedIpState) -> IpRecommendation {
    match (mode, shared) {
        (_, SharedIpState::Unknown) => IpRecommendation::Undetermined,
        (CoordinationMode::CoherentHousehold, SharedIpState::Shared) => {
            IpRecommendation::ConsistentExpected
        }
        (CoordinationMode::Fragmentation, SharedIpState::Shared) => IpRecommendation::LinkageRisk,
        (_, SharedIpState::Distinct) => IpRecommendation::Independent,
    }
}

/// A short, human-readable explanation of the verdict.
fn describe(
    mode: CoordinationMode,
    shared: SharedIpState,
    local: Option<&str>,
    peer: Option<&str>,
) -> String {
    match recommend(mode, shared) {
        IpRecommendation::ConsistentExpected => {
            "Coherent household sharing one public IP is expected and reinforcing; keep signals consistent across devices.".to_string()
        }
        IpRecommendation::LinkageRisk => format!(
            "Fragmentation mode but both devices egress from the same public IP ({}); this links the personas at the network layer. Diverge egress (route the phone via LTE while the desktop keeps the home IP) and use per-persona egress (C7) where available.",
            local.or(peer).unwrap_or("unknown")
        ),
        IpRecommendation::Independent => {
            "Devices egress from distinct public IPs; no network-layer linkage between their personas.".to_string()
        }
        IpRecommendation::Undetermined => {
            "Public IP not observed for at least one device; wire a PublicIpSource (STUN or the C7 egress layer) to assess linkage. The core makes no network call by default.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_source_reports_none() -> Result<()> {
        assert_eq!(UnknownPublicIp.observe_public_ip().await?, None);
        assert_eq!(FixedPublicIp::unknown().observe_public_ip().await?, None);
        assert_eq!(
            FixedPublicIp::new("198.51.100.9")
                .observe_public_ip()
                .await?,
            Some("198.51.100.9".to_string())
        );
        Ok(())
    }

    #[test]
    fn shared_state_classifies_pairs() {
        assert_eq!(
            shared_ip_state(Some("1.2.3.4"), Some("1.2.3.4")),
            SharedIpState::Shared
        );
        assert_eq!(
            shared_ip_state(Some("1.2.3.4"), Some("5.6.7.8")),
            SharedIpState::Distinct
        );
        assert_eq!(
            shared_ip_state(Some("1.2.3.4"), None),
            SharedIpState::Unknown
        );
        assert_eq!(shared_ip_state(None, None), SharedIpState::Unknown);
    }

    #[test]
    fn coherent_shared_is_expected() {
        let a = assess(
            CoordinationMode::CoherentHousehold,
            Some("203.0.113.7".to_string()),
            Some("203.0.113.7".to_string()),
        );
        assert_eq!(a.shared, SharedIpState::Shared);
        assert_eq!(a.recommendation, IpRecommendation::ConsistentExpected);
    }

    #[test]
    fn fragmentation_shared_is_linkage_risk() {
        let a = assess(
            CoordinationMode::Fragmentation,
            Some("203.0.113.7".to_string()),
            Some("203.0.113.7".to_string()),
        );
        assert_eq!(a.shared, SharedIpState::Shared);
        assert_eq!(a.recommendation, IpRecommendation::LinkageRisk);
        // The divergence guidance is present for a client to surface.
        assert!(a.detail.contains("LTE"));
    }

    #[test]
    fn distinct_is_independent_in_both_modes() {
        for mode in [
            CoordinationMode::CoherentHousehold,
            CoordinationMode::Fragmentation,
        ] {
            let a = assess(
                mode,
                Some("203.0.113.7".to_string()),
                Some("198.51.100.9".to_string()),
            );
            assert_eq!(a.shared, SharedIpState::Distinct);
            assert_eq!(a.recommendation, IpRecommendation::Independent);
        }
    }

    #[test]
    fn unknown_is_undetermined() {
        let a = assess(
            CoordinationMode::Fragmentation,
            None,
            Some("203.0.113.7".to_string()),
        );
        assert_eq!(a.shared, SharedIpState::Unknown);
        assert_eq!(a.recommendation, IpRecommendation::Undetermined);
    }

    #[test]
    fn assessment_serializes_camelcase() -> Result<()> {
        let a = assess(
            CoordinationMode::Fragmentation,
            Some("203.0.113.7".to_string()),
            Some("203.0.113.7".to_string()),
        );
        let json = serde_json::to_string(&a)?;
        assert!(json.contains("\"localIp\""));
        assert!(json.contains("\"peerIp\""));
        assert!(json.contains("\"recommendation\""));
        let back: WanIpAssessment = serde_json::from_str(&json)?;
        assert_eq!(back, a);
        Ok(())
    }
}
