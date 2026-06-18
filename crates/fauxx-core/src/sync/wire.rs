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

//! The cross-device wire schema.
//!
//! This module is the desktop-authoritative contract that the Android side
//! (issue E13) implements against. It defines, all versioned:
//!
//! - [`SYNC_PROTOCOL_VERSION`]: the single protocol version both sides carry so
//!   they can negotiate and stay forward-compatible.
//! - [`SyncMessage`]: the envelope that wraps a persona operation, serialized
//!   with `serde_json` and then sealed by the [`crypto`](super::crypto) layer.
//! - [`PairingPayload`]: the compact, base64-of-JSON blob carried in the
//!   pairing QR.
//! - [`fingerprint`]: the short, human-comparable hash of a public key shown in
//!   discovery and printed under the QR.
//!
//! See `docs/SYNC_PROTOCOL.md` for the full prose specification.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::generate::SignedArtifact;
use crate::persona::SyntheticPersona;
use crate::sync::crypto::PUBLIC_KEY_LEN;

/// The sync protocol version. Bumped when the envelope or pairing schema
/// changes incompatibly. Both peers carry it (QR, mDNS TXT, and every
/// [`SyncMessage`]) so a newer peer can detect and refuse an older one rather
/// than misparsing.
pub const SYNC_PROTOCOL_VERSION: u16 = 1;

/// The mDNS service type advertised and browsed on the LAN.
///
/// `_fauxx-sync._tcp.local.` per RFC 6763. Carried here so the desktop, the
/// CLI, and the phone all agree on one string.
pub const SERVICE_TYPE: &str = "_fauxx-sync._tcp.local.";

/// mDNS TXT record key carrying the protocol version (decimal string).
pub const TXT_KEY_VERSION: &str = "v";
/// mDNS TXT record key carrying the device public-key fingerprint.
pub const TXT_KEY_FINGERPRINT: &str = "fp";
/// mDNS TXT record key carrying the full base64url public key (so a browsing
/// peer can pair from discovery alone, the QR being the primary path).
pub const TXT_KEY_PUBKEY: &str = "pk";

/// A persona operation carried over the sealed channel.
///
/// Adjacently tagged (`#[serde(tag = "kind", content = "body")]`), so the wire
/// form is `{"kind":"PersonaUpsert","body":{...}}`. The enum is
/// `#[non_exhaustive]` so later protocol versions can add kinds, but parsing is
/// closed: a receiver that does not recognize a `kind` fails to parse and the
/// frame is rejected (fail closed). Forward compatibility is negotiated through
/// [`SyncMessage::protocol_version`], not by tolerating unknown kinds. See
/// `docs/SYNC_PROTOCOL.md` section 7.
// `Eq` is deliberately NOT derived: the C6 `SignedArtifact` body carries a
// weight map / query plan with `f64` weights, which are `PartialEq` but not
// `Eq`. `PartialEq` is all the call sites and tests need (no `SyncBody` ever
// lives in a `HashSet`/`BTreeSet` key position).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body")]
#[non_exhaustive]
// `PersonaUpsert` carries a full `SyntheticPersona`, which is the DOMINANT,
// common-case wire payload; the other variants are tiny control messages. As the
// persona grew additive desktop fields (C5 #24), the size gap tripped
// `large_enum_variant`. Boxing the common-case payload to shrink rare control
// variants would pessimize the hot path (an allocation on every persona sync)
// for no real benefit, so the gap is accepted deliberately.
#[allow(clippy::large_enum_variant)]
pub enum SyncBody {
    /// Insert or replace a persona on the receiver. The body is the exact
    /// Android-compatible [`SyntheticPersona`] JSON. The `kind` tag is the
    /// stable PascalCase string `"PersonaUpsert"`.
    PersonaUpsert(SyntheticPersona),

    /// Share the sender's currently observed public (WAN) IP so the receiver
    /// can detect whether the two devices share a public IP (C1 #9, O3). The
    /// `kind` tag is `"PublicIpReport"`. The body carries the sender's observed
    /// IP, or `null` when unknown (a device behind NAT that has not observed
    /// its public IP). The comparison is peer-to-peer, never via a third party.
    PublicIpReport(PublicIpReport),

    /// Propagate household coordination state: the active mode and the persona
    /// assignment the sender believes applies to the receiver (C1 #8, O2). The
    /// `kind` tag is `"CoordinationState"`. Coherent election propagates the
    /// elected persona via a `PersonaUpsert` and the converged assignment via
    /// this kind; Fragmentation propagates each device's distinct assignment.
    CoordinationState(CoordinationState),

    /// Deliver a SIGNED generated artifact (C6 #28, H1): the desktop-generated
    /// weight map or query plan the phone replays. The `kind` tag is
    /// `"SignedArtifact"`. The body is a self-contained
    /// [`SignedArtifact`](crate::generate::SignedArtifact) carrying its own
    /// ed25519 signature, signer public key, and freshness/expiry timestamps; the
    /// receiver VERIFIES the signature and freshness before use and FALLS BACK to
    /// on-device generation when no fresh, valid artifact is present. The sealed
    /// channel already authenticates the paired SENDER; the embedded signature
    /// additionally binds the artifact CONTENT to the desktop's artifact-signing
    /// key, so a tampered or stale artifact is rejected (fail closed).
    SignedArtifact(SignedArtifact),

    /// Distribute a SIGNED persona pack (C6 #29, H2): the desktop-minted
    /// (PUMS-microdata) personas bundled into a [`PersonaPack`] the phone imports.
    /// The `kind` tag is `"PersonaPack"`. The body carries the pack's JSON bytes as
    /// a STANDARD-base64 string (the same import/export byte form, so the wire kind
    /// is decoupled from the pack's internal serde shape and stays stable as the
    /// pack format evolves). The receiver decodes the bytes and VERIFIES the pack
    /// signature ([`verify_pack`](crate::personapack::verify_pack)) BEFORE importing
    /// the personas (verify-before-write, fail closed on a bad/unsigned/tampered
    /// pack). The sealed channel already authenticates the paired SENDER; the
    /// embedded ed25519 signature additionally binds the pack CONTENT to the
    /// signer's key.
    PersonaPack(PersonaPackBody),
}

impl SyncBody {
    /// The stable `kind` discriminator string for this body (for logs and
    /// diagnostics; matches the `kind` tag on the wire).
    pub fn kind_name(&self) -> &'static str {
        match self {
            SyncBody::PersonaUpsert(_) => "PersonaUpsert",
            SyncBody::PublicIpReport(_) => "PublicIpReport",
            SyncBody::CoordinationState(_) => "CoordinationState",
            SyncBody::SignedArtifact(_) => "SignedArtifact",
            SyncBody::PersonaPack(_) => "PersonaPack",
        }
    }
}

/// The body of a [`SyncBody::PersonaPack`]: the signed persona pack's JSON bytes,
/// carried as a STANDARD-base64 string.
///
/// The pack bytes are carried OPAQUELY (base64 of the pack's own import/export
/// byte form) rather than re-nesting the [`PersonaPack`](crate::personapack::PersonaPack)
/// struct, so the wire kind is decoupled from the pack's internal serde shape and
/// stays stable as the pack format version evolves; the receiver decodes the bytes
/// and runs the P4 verifier over them. Serialized in camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaPackBody {
    /// The signed persona pack's JSON bytes, STANDARD base64.
    pub pack_base64: String,
}

impl PersonaPackBody {
    /// Wrap raw signed-pack bytes as a base64 pack body.
    pub fn from_pack_bytes(bytes: &[u8]) -> Self {
        use base64::engine::general_purpose::STANDARD as BASE64;
        Self {
            pack_base64: BASE64.encode(bytes),
        }
    }

    /// Decode the carried pack bytes, failing closed on invalid base64.
    pub fn pack_bytes(&self) -> Result<Vec<u8>> {
        use base64::engine::general_purpose::STANDARD as BASE64;
        BASE64
            .decode(self.pack_base64.as_bytes())
            .map_err(|e| CoreError::Sync(format!("persona-pack body not valid base64: {e}")))
    }
}

/// The body of a [`SyncBody::PublicIpReport`]: a device's observed public IP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicIpReport {
    /// The sender's observed public IP, or `None` (JSON `null`) when unknown.
    #[serde(default)]
    pub public_ip: Option<String>,
    /// Epoch milliseconds when the sender observed it.
    pub observed_at: i64,
}

/// The body of a [`SyncBody::CoordinationState`]: a mode and an assignment.
///
/// `mode` is the stable [`CoordinationMode`](crate::orchestration::CoordinationMode)
/// string (`"CoherentHousehold"` / `"Fragmentation"`). `personaId` is the
/// persona id the sender asserts for the receiving device (the elected shared
/// persona under Coherent, or the receiver's distinct persona under
/// Fragmentation). A receiver applies it according to its own paired-peer
/// trust; an unknown mode string fails closed at parse of the orchestration
/// layer, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationState {
    /// The sender's active coordination mode, as its stable string.
    pub mode: String,
    /// The persona id the sender asserts for the receiving device.
    pub persona_id: String,
}

/// The versioned envelope serialized (as JSON) and then sealed.
///
/// The plaintext that gets sealed is the `serde_json` encoding of this struct;
/// nothing here travels in the clear (the whole struct is inside the
/// ciphertext). The `protocol_version` rides inside so a receiver that opens an
/// envelope can still reject a version it does not understand.
// `Eq` is not derived for the same reason as [`SyncBody`]: a `SignedArtifact`
// body carries `f64` weights, which are `PartialEq` but not `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncMessage {
    /// Protocol version of this message. See [`SYNC_PROTOCOL_VERSION`].
    pub protocol_version: u16,
    /// The operation and its payload.
    #[serde(flatten)]
    pub body: SyncBody,
}

impl SyncMessage {
    /// Wrap a persona upsert at the current protocol version.
    pub fn persona_upsert(persona: SyntheticPersona) -> Self {
        Self {
            protocol_version: SYNC_PROTOCOL_VERSION,
            body: SyncBody::PersonaUpsert(persona),
        }
    }

    /// Wrap a public-IP report at the current protocol version (C1 #9, O3).
    pub fn public_ip_report(public_ip: Option<String>, observed_at: i64) -> Self {
        Self {
            protocol_version: SYNC_PROTOCOL_VERSION,
            body: SyncBody::PublicIpReport(PublicIpReport {
                public_ip,
                observed_at,
            }),
        }
    }

    /// Wrap a coordination-state propagation at the current protocol version
    /// (C1 #8, O2).
    pub fn coordination_state(mode: String, persona_id: String) -> Self {
        Self {
            protocol_version: SYNC_PROTOCOL_VERSION,
            body: SyncBody::CoordinationState(CoordinationState { mode, persona_id }),
        }
    }

    /// Wrap a signed generated artifact at the current protocol version (C6 #28,
    /// H1). The receiver verifies the embedded signature + freshness before use.
    pub fn signed_artifact(artifact: SignedArtifact) -> Self {
        Self {
            protocol_version: SYNC_PROTOCOL_VERSION,
            body: SyncBody::SignedArtifact(artifact),
        }
    }

    /// Wrap a signed persona pack at the current protocol version (C6 #29, H2),
    /// from the pack's JSON bytes. The receiver decodes and VERIFIES the pack
    /// signature before importing (verify-before-write, fail closed).
    pub fn persona_pack(pack_bytes: &[u8]) -> Self {
        Self {
            protocol_version: SYNC_PROTOCOL_VERSION,
            body: SyncBody::PersonaPack(PersonaPackBody::from_pack_bytes(pack_bytes)),
        }
    }

    /// Serialize to the canonical JSON plaintext that the channel seals.
    pub fn to_plaintext(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Parse a [`SyncMessage`] from opened (decrypted) plaintext, rejecting a
    /// protocol version this build does not understand. A *newer* major
    /// version fails closed rather than being misinterpreted.
    pub fn from_plaintext(bytes: &[u8]) -> Result<Self> {
        let msg: SyncMessage = serde_json::from_slice(bytes)?;
        if msg.protocol_version > SYNC_PROTOCOL_VERSION {
            return Err(CoreError::Sync(format!(
                "unsupported sync protocol version {} (this build speaks {SYNC_PROTOCOL_VERSION})",
                msg.protocol_version
            )));
        }
        Ok(msg)
    }
}

/// The pairing blob encoded into the QR shown out of band.
///
/// Compact on purpose: it is base64url-of-JSON so it fits a small QR. It
/// carries this device's public key plus a connection hint (the mDNS instance
/// name, host, and port) so the scanner can both record the key and reconnect
/// over the LAN. It is *versioned* so the phone can reject an unknown layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingPayload {
    /// Pairing payload schema version. Equals [`SYNC_PROTOCOL_VERSION`] for now.
    pub v: u16,
    /// Human-readable device name (the mDNS instance name).
    pub name: String,
    /// This device's X25519 public key, base64url (no padding).
    pub pk: String,
    /// mDNS host name hint (e.g. `desktop.local.`), best-effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// TCP port hint for the sync transport.
    pub port: u16,
}

impl PairingPayload {
    /// Build a pairing payload from this device's name, public key, and
    /// connection hint.
    pub fn new(
        name: String,
        public_key: &[u8; PUBLIC_KEY_LEN],
        host: Option<String>,
        port: u16,
    ) -> Self {
        Self {
            v: SYNC_PROTOCOL_VERSION,
            name,
            pk: URL_SAFE_NO_PAD.encode(public_key),
            host,
            port,
        }
    }

    /// Encode to the compact base64url string carried in the QR. The outer
    /// base64 wraps the JSON so the QR alphabet stays small and dense.
    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_vec(self)?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    /// Decode a pairing payload from the QR string, rejecting an unknown
    /// version (fail closed) so a future layout is never misread.
    pub fn decode(encoded: &str) -> Result<Self> {
        let json = URL_SAFE_NO_PAD
            .decode(encoded.trim())
            .map_err(|e| CoreError::Sync(format!("pairing payload not valid base64url: {e}")))?;
        let payload: PairingPayload = serde_json::from_slice(&json)?;
        if payload.v > SYNC_PROTOCOL_VERSION {
            return Err(CoreError::Sync(format!(
                "unsupported pairing payload version {} (this build speaks {SYNC_PROTOCOL_VERSION})",
                payload.v
            )));
        }
        Ok(payload)
    }

    /// Decode and return the public-key bytes carried in this payload, failing
    /// closed on the wrong length.
    pub fn public_key_bytes(&self) -> Result<[u8; PUBLIC_KEY_LEN]> {
        decode_public_key(&self.pk)
    }
}

/// Decode a base64url-encoded public key into fixed-size bytes, failing closed
/// on bad encoding or wrong length.
pub fn decode_public_key(encoded: &str) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|e| CoreError::Sync(format!("public key not valid base64url: {e}")))?;
    bytes.as_slice().try_into().map_err(|_| {
        CoreError::Sync(format!(
            "public key is {} bytes, expected {PUBLIC_KEY_LEN}",
            bytes.len()
        ))
    })
}

/// Encode public-key bytes as base64url (no padding), the canonical text form
/// used across the QR, the TXT record, and persistence.
pub fn encode_public_key(public_key: &[u8; PUBLIC_KEY_LEN]) -> String {
    URL_SAFE_NO_PAD.encode(public_key)
}

/// A short, human-comparable fingerprint of a public key.
///
/// Defined as the lowercase hex of the first 8 bytes of the BLAKE2b-256 hash of
/// the key, grouped in four colon-separated pairs (e.g. `1a2b:3c4d:5e6f:7081`).
/// Used in the discovery list and printed under the QR so a user can eyeball
/// that the scanned device matches the discovered one. It is an integrity
/// convenience, not a security boundary; the sealed channel is what enforces
/// authentication.
pub fn fingerprint(public_key: &[u8]) -> String {
    use dryoc::generichash::{GenericHash, Key};
    // 32-byte BLAKE2b digest, no key. `Key` pins the (unused) key type so the
    // `None` argument type-checks; an internal hash failure degrades to an
    // empty digest rather than panicking (this is a display convenience).
    let digest: Vec<u8> =
        GenericHash::hash_with_defaults_to_vec::<_, Key>(public_key, None).unwrap_or_default();
    let short = &digest[..digest.len().min(8)];
    let mut out = String::with_capacity(short.len() * 2 + 3);
    for (i, b) in short.iter().enumerate() {
        if i > 0 && i % 2 == 0 {
            out.push(':');
        }
        let _ = std::fmt::write(&mut out, format_args!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn sample_persona() -> SyntheticPersona {
        SyntheticPersona::new(
            "55555555-5555-4555-8555-555555555555".to_string(),
            "Wire Persona".to_string(),
            AgeRange::AGE_55_64.as_name().to_string(),
            Profession::RETIRED.as_name().to_string(),
            Region::WESTERN_EUROPE.as_name().to_string(),
            vec![
                CategoryPool::TRAVEL.as_name().to_string(),
                CategoryPool::COOKING.as_name().to_string(),
                CategoryPool::HISTORY.as_name().to_string(),
            ],
            1_700_000_000_111,
            1_700_600_000_222,
        )
    }

    #[test]
    fn sync_message_round_trips_persona_all_fields() -> Result<()> {
        let mut persona = sample_persona();
        persona.note = Some("a desktop-only note".to_string());
        let msg = SyncMessage::persona_upsert(persona.clone());

        let bytes = msg.to_plaintext()?;
        let back = SyncMessage::from_plaintext(&bytes)?;
        assert_eq!(back.protocol_version, SYNC_PROTOCOL_VERSION);
        let got = match back.body {
            SyncBody::PersonaUpsert(p) => p,
            other => {
                return Err(CoreError::Sync(format!(
                    "expected PersonaUpsert, got {other:?}"
                )))
            }
        };
        // Every field, including rotation timing and additive fields.
        assert_eq!(got.id, persona.id);
        assert_eq!(got.name, persona.name);
        assert_eq!(got.age_range, persona.age_range);
        assert_eq!(got.profession, persona.profession);
        assert_eq!(got.region, persona.region);
        assert_eq!(got.interests, persona.interests);
        assert_eq!(got.created_at, persona.created_at);
        assert_eq!(got.active_until, persona.active_until);
        assert_eq!(got.schema_version, persona.schema_version);
        assert_eq!(got.note, persona.note);
        assert_eq!(got, persona);
        Ok(())
    }

    #[test]
    fn sync_message_json_carries_protocol_and_camelcase() -> Result<()> {
        let msg = SyncMessage::persona_upsert(sample_persona());
        let json =
            String::from_utf8(msg.to_plaintext()?).map_err(|e| CoreError::Sync(e.to_string()))?;
        assert!(json.contains("\"protocolVersion\""));
        assert!(json.contains("\"kind\""));
        assert!(json.contains("PersonaUpsert"));
        // The persona body keeps the Android camelCase keys.
        assert!(json.contains("\"ageRange\""));
        assert!(json.contains("\"createdAt\""));
        assert!(json.contains("\"activeUntil\""));
        Ok(())
    }

    #[test]
    fn sync_message_rejects_future_version() -> Result<()> {
        let mut msg = SyncMessage::persona_upsert(sample_persona());
        msg.protocol_version = SYNC_PROTOCOL_VERSION + 1;
        let bytes = msg.to_plaintext()?;
        assert!(matches!(
            SyncMessage::from_plaintext(&bytes),
            Err(CoreError::Sync(_))
        ));
        Ok(())
    }

    /// Build a signed C6 weight-map artifact for the wire tests, using a fixed
    /// pack-signing seed so the result is deterministic.
    fn sample_signed_artifact() -> Result<SignedArtifact> {
        use crate::generate::{
            sign_artifact_with, ArtifactContent, ArtifactPayload, WeightMap, DEFAULT_FRESHNESS_MS,
        };
        use crate::personapack::{PackSigningKey, PACK_SEED_LEN};
        let key = PackSigningKey::from_seed(&[5u8; PACK_SEED_LEN]);
        let mut map: WeightMap = WeightMap::new();
        for c in CategoryPool::all() {
            map.insert(
                c.as_name().to_string(),
                1.0 / CategoryPool::all().len() as f64,
            );
        }
        let content = ArtifactContent::new(
            "55555555-5555-4555-8555-555555555555",
            ArtifactPayload::WeightMap(map),
            1_700_000_000_000,
            DEFAULT_FRESHNESS_MS,
        );
        sign_artifact_with(content, &key).map_err(|e| CoreError::Sync(e.to_string()))
    }

    #[test]
    fn sync_message_round_trips_signed_artifact() -> Result<()> {
        let artifact = sample_signed_artifact()?;
        let msg = SyncMessage::signed_artifact(artifact.clone());
        let bytes = msg.to_plaintext()?;
        let back = SyncMessage::from_plaintext(&bytes)?;
        assert_eq!(back.protocol_version, SYNC_PROTOCOL_VERSION);
        match back.body {
            SyncBody::SignedArtifact(got) => {
                assert_eq!(got.payload_kind(), "WeightMap");
                assert_eq!(got.signer_public_key, artifact.signer_public_key);
                assert_eq!(got.content.persona_id, artifact.content.persona_id);
                // The whole artifact survives, weight map included.
                assert_eq!(got, artifact);
                // It still verifies after the wire round trip.
                crate::generate::verify_parsed_artifact(&got)
                    .map_err(|e| CoreError::Sync(e.to_string()))?;
            }
            other => {
                return Err(CoreError::Sync(format!(
                    "expected SignedArtifact, got {other:?}"
                )))
            }
        }
        Ok(())
    }

    #[test]
    fn sync_message_json_carries_signed_artifact_kind() -> Result<()> {
        let msg = SyncMessage::signed_artifact(sample_signed_artifact()?);
        let json =
            String::from_utf8(msg.to_plaintext()?).map_err(|e| CoreError::Sync(e.to_string()))?;
        assert!(json.contains("\"kind\""));
        assert!(json.contains("SignedArtifact"));
        assert!(json.contains("\"artifactKind\""));
        assert!(json.contains("\"signerPublicKey\""));
        Ok(())
    }

    #[test]
    fn sync_message_round_trips_persona_pack() -> Result<()> {
        // A signed persona pack rides the PersonaPack kind as base64 bytes and
        // survives the envelope round trip; the decoded bytes still verify (P4).
        use crate::mint::{mint_pack, mint_personas, PersonaDistribution};
        use crate::personapack::{verify_pack, PackSigningKey, PACK_SEED_LEN};

        let dist = PersonaDistribution::bundled().map_err(|e| CoreError::Sync(e.to_string()))?;
        let minted = mint_personas(&dist, 2, 4, 1_700_000_000_000)
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let key = PackSigningKey::from_seed(&[3u8; PACK_SEED_LEN]);
        let pack = mint_pack(&minted, 1_700_000_000_000, &key)
            .map_err(|e| CoreError::Sync(e.to_string()))?;
        let pack_bytes = pack
            .to_bytes()
            .map_err(|e| CoreError::Sync(e.to_string()))?;

        let msg = SyncMessage::persona_pack(&pack_bytes);
        let bytes = msg.to_plaintext()?;
        let back = SyncMessage::from_plaintext(&bytes)?;
        assert_eq!(back.protocol_version, SYNC_PROTOCOL_VERSION);
        match back.body {
            SyncBody::PersonaPack(body) => {
                let decoded = body.pack_bytes()?;
                assert_eq!(decoded, pack_bytes);
                // It still verifies after the wire round trip.
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

    #[test]
    fn sync_message_json_carries_persona_pack_kind() -> Result<()> {
        let msg = SyncMessage::persona_pack(b"signed-pack-bytes");
        let json =
            String::from_utf8(msg.to_plaintext()?).map_err(|e| CoreError::Sync(e.to_string()))?;
        assert!(json.contains("\"kind\""));
        assert!(json.contains("PersonaPack"));
        assert!(json.contains("\"packBase64\""));
        Ok(())
    }

    #[test]
    fn unknown_kind_fails_closed_at_parse() -> Result<()> {
        // A frame carrying a `kind` this build does not define MUST fail to parse
        // (the adjacently-tagged enum is closed), never be silently ignored. This
        // is the unknown-kind fail-closed posture for the new wire kind era.
        let unknown = br#"{"protocolVersion":1,"kind":"TotallyUnknownKind","body":{}}"#;
        let result = SyncMessage::from_plaintext(unknown);
        assert!(
            matches!(result, Err(CoreError::Serde(_))),
            "an unknown kind must fail closed at parse, got {result:?}"
        );
        Ok(())
    }

    #[test]
    fn pairing_payload_round_trip() -> Result<()> {
        let pk = [7u8; PUBLIC_KEY_LEN];
        let payload = PairingPayload::new(
            "Desktop-Study".to_string(),
            &pk,
            Some("desktop.local.".to_string()),
            45_999,
        );
        let encoded = payload.encode()?;
        let back = PairingPayload::decode(&encoded)?;
        assert_eq!(back, payload);
        assert_eq!(back.public_key_bytes()?, pk);
        assert_eq!(back.port, 45_999);
        assert_eq!(back.host.as_deref(), Some("desktop.local."));
        Ok(())
    }

    #[test]
    fn pairing_payload_rejects_future_version() -> Result<()> {
        let mut payload = PairingPayload::new("D".to_string(), &[1u8; PUBLIC_KEY_LEN], None, 1);
        payload.v = SYNC_PROTOCOL_VERSION + 5;
        let encoded = payload.encode()?;
        assert!(matches!(
            PairingPayload::decode(&encoded),
            Err(CoreError::Sync(_))
        ));
        Ok(())
    }

    #[test]
    fn pairing_payload_rejects_garbage() {
        assert!(PairingPayload::decode("!!!not base64!!!").is_err());
    }

    #[test]
    fn public_key_codec_round_trip() -> Result<()> {
        let pk = [42u8; PUBLIC_KEY_LEN];
        let text = encode_public_key(&pk);
        assert_eq!(decode_public_key(&text)?, pk);
        assert!(decode_public_key("short").is_err());
        Ok(())
    }

    #[test]
    fn fingerprint_is_stable_and_grouped() {
        let pk = [9u8; PUBLIC_KEY_LEN];
        let fp = fingerprint(&pk);
        assert_eq!(fp, fingerprint(&pk));
        // Four colon-separated 2-byte groups => 16 hex chars + 3 colons.
        assert_eq!(fp.len(), 19);
        assert_eq!(fp.matches(':').count(), 3);
        assert_ne!(fp, fingerprint(&[8u8; PUBLIC_KEY_LEN]));
    }
}
