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

//! Generate-on-desktop, execute-on-phone (C6 #28, H1).
//!
//! The desktop runs the heavy generation work and pushes SIGNED artifacts to the
//! paired phone over the existing O1 sealed channel; the phone verifies the
//! signature and freshness, then either REPLAYS the artifact or FALLS BACK to its
//! own on-device generation. This module owns three things:
//!
//! 1. [`allocator`]: the adversarial-allocation surrogate (the on-phone E4
//!    `AdversarialAllocator.allocate`), producing the signed WEIGHT MAP.
//! 2. [`plan`]: category-targeted, timed QUERY PLAN generation (the desktop-side
//!    analogue of the phone's E6 on-device generation), biased by the weight map.
//! 3. The signed-artifact envelope ([`SignedArtifact`]) carrying either, with the
//!    ed25519 sign/verify primitives (the same `PackSigningKey` pattern as
//!    persona packs) and a freshness/expiry the consumer checks.
//!
//! Both artifacts ride the sealed channel as the
//! [`crate::sync::wire::SyncBody::SignedArtifact`]
//! wire kind; see [`crate::sync::wire`] for the wire format and [`crate::sync`]
//! for the security model.
//!
//! ## Honesty
//!
//! The query plan emits category-targeted INTENTS, not full query strings: the
//! phone expands each intent on replay using its own query banks. Porting the
//! Android `GrammarQueryGenerator` banks + grammar here was investigated against
//! the real source (`../fauxx`) and DELIBERATELY NOT done: (1) the desktop's own
//! decoy browser visits category SITES and never issues search-engine queries, so
//! it has no internal need for query strings; (2) the phone already ships the full
//! generator + ~4 MB of per-locale banks and expands intents itself, so emitting
//! strings desktop-side would only duplicate them over the wire; and (3) it would
//! force a second copy of the safety-critical, native-speaker-reviewed
//! `QueryBlocklist` corpus, which could drift out of sync with the phone's and
//! emit a harmful query the phone would refuse. The category intent is the right
//! desktop-generate / phone-execute boundary. [`plan::QueryIntent::query`] remains
//! a wire field for any future desktop-side generator (e.g. a studio preview).

pub mod allocator;
pub mod plan;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};

use crate::personapack::PackSigningKey;

pub use allocator::{
    allocate, is_sensitive, weight_map_kl, WeightMap, WeightNormalizer, EPS, FACTORS, KL_BUDGET,
    MIN_WEIGHT, PASSES, SENSITIVE_ATTRIBUTES,
};
pub use plan::{generate_query_plan, QueryIntent, QueryPlan};

/// The CURRENT generated-artifact schema version this build writes. Bumped when
/// the artifact envelope changes incompatibly. An unknown/NEWER version is
/// REJECTED ([`ArtifactError::UnsupportedSchemaVersion`]), never silently
/// accepted.
pub const CURRENT_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// The lowest artifact schema version this build can still verify. Artifacts at
/// or above this and at or below [`CURRENT_ARTIFACT_SCHEMA_VERSION`] are accepted.
pub const MIN_SUPPORTED_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// Default freshness window for a generated artifact, in milliseconds: 24 hours.
/// The phone treats an artifact older than this (or past its explicit
/// `expires_at`) as STALE and falls back to its own on-device generation. The
/// desktop regenerates and re-pushes well within the window.
pub const DEFAULT_FRESHNESS_MS: i64 = 24 * 60 * 60 * 1_000;

/// Typed validation/verification failures for generated artifacts. Returned so
/// callers can match the failure mode rather than parsing a string. A failure is
/// NEVER a silent accept.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactError {
    /// The artifact bytes were not valid artifact JSON, or a field was malformed.
    #[error("malformed generated artifact: {0}")]
    Malformed(String),

    /// The artifact declared a schema version this build does not support.
    #[error("unsupported artifact schema version {found} (this build supports {min}..={max})")]
    UnsupportedSchemaVersion {
        /// The version the artifact declared.
        found: u32,
        /// The lowest supported version.
        min: u32,
        /// The highest supported version.
        max: u32,
    },

    /// The embedded signer public key was not valid base64, or not a valid
    /// ed25519 public key.
    #[error("invalid signer public key: {0}")]
    InvalidPublicKey(String),

    /// The embedded signature was not valid base64, or not a 64-byte ed25519
    /// signature.
    #[error("invalid signature encoding: {0}")]
    InvalidSignature(String),

    /// The signature did not verify against the embedded public key over the
    /// canonical content bytes: the artifact was TAMPERED with, or it was signed
    /// by a different key than the one embedded. Rejected.
    #[error("generated-artifact signature verification failed (tampered or wrong key)")]
    BadSignature,

    /// The canonical content bytes could not be produced (a serialization
    /// failure while signing or verifying).
    #[error("generated-artifact canonicalization failed: {0}")]
    Canonicalization(String),
}

/// The payload a [`SignedArtifact`] carries: either a generated weight map or a
/// query plan. Adjacently tagged so the wire form is
/// `{"artifactKind":"WeightMap","payload":{...}}`. `#[non_exhaustive]` so later
/// versions can add payload kinds; parsing stays CLOSED (an unknown
/// `artifactKind` fails to parse and the artifact is rejected, fail closed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "artifactKind", content = "payload")]
#[non_exhaustive]
pub enum ArtifactPayload {
    /// The signed adversarial-allocation WEIGHT MAP (a distribution over every
    /// [`crate::persona::CategoryPool`]). The phone biases its category selection
    /// with this.
    WeightMap(WeightMap),
    /// The signed, category-targeted, timed QUERY PLAN the phone replays.
    QueryPlan(QueryPlan),
}

impl ArtifactPayload {
    /// The stable `artifactKind` discriminator for this payload.
    pub fn kind_name(&self) -> &'static str {
        match self {
            ArtifactPayload::WeightMap(_) => "WeightMap",
            ArtifactPayload::QueryPlan(_) => "QueryPlan",
        }
    }
}

/// The SIGNABLE content of a generated artifact: everything the signature covers.
/// By construction this struct has NO signature field, so `serde_json` of it is
/// exactly the canonical bytes that get signed and verified. Field order is fixed
/// (serde emits in declaration order), keeping the canonical bytes stable across
/// a signer and a verifier on the same build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactContent {
    /// The artifact format version. A reader rejects an unknown/newer version
    /// with a typed [`ArtifactError::UnsupportedSchemaVersion`].
    pub schema_version: u32,
    /// When the artifact was generated, epoch milliseconds. The freshness check
    /// is measured from here.
    pub generated_at: i64,
    /// When the artifact EXPIRES, epoch milliseconds. The phone treats an
    /// artifact whose `expires_at` has passed (or that is older than the
    /// consumer's freshness window) as STALE and falls back to on-device
    /// generation.
    pub expires_at: i64,
    /// The persona id this artifact was generated for (so the phone applies it to
    /// the right persona).
    pub persona_id: String,
    /// The carried payload (weight map or query plan).
    pub payload: ArtifactPayload,
}

impl ArtifactContent {
    /// Build artifact content at [`CURRENT_ARTIFACT_SCHEMA_VERSION`] with an
    /// explicit freshness window: it expires `freshness_ms` after `generated_at`.
    pub fn new(
        persona_id: impl Into<String>,
        payload: ArtifactPayload,
        generated_at: i64,
        freshness_ms: i64,
    ) -> Self {
        Self {
            schema_version: CURRENT_ARTIFACT_SCHEMA_VERSION,
            generated_at,
            expires_at: generated_at.saturating_add(freshness_ms.max(0)),
            persona_id: persona_id.into(),
            payload,
        }
    }

    /// The canonical bytes that the signature covers: `serde_json` of this content
    /// struct (which has no signature field), STABILIZED through one
    /// serialize -> parse -> serialize round trip.
    ///
    /// The round trip matters because the artifact carries `f64` weights (the
    /// adversarial-allocation weight map), and `serde_json`'s float PARSER is not
    /// always the exact inverse of its (ryu) SERIALIZER: parsing a serialized
    /// `f64` can land one ULP away. A signer that signed the raw struct bytes and
    /// a verifier that re-serialized the PARSED artifact would then derive
    /// different bytes and the signature would spuriously fail. Serializing the
    /// already-parsed value reaches the parser's fixed point, so the signer
    /// (which canonicalizes here) and the verifier (which parses the artifact,
    /// then canonicalizes the parsed content here) converge on identical bytes.
    pub fn canonical_bytes(&self) -> std::result::Result<Vec<u8>, ArtifactError> {
        let raw =
            serde_json::to_vec(self).map_err(|e| ArtifactError::Canonicalization(e.to_string()))?;
        let parsed: ArtifactContent = serde_json::from_slice(&raw)
            .map_err(|e| ArtifactError::Canonicalization(e.to_string()))?;
        serde_json::to_vec(&parsed).map_err(|e| ArtifactError::Canonicalization(e.to_string()))
    }

    /// Whether this artifact is still FRESH at wall-clock `now` (epoch ms):
    /// `now` is at or after `generated_at` and strictly before `expires_at`. A
    /// consumer uses this to decide whether to replay the desktop artifact or
    /// fall back to its own on-device generation.
    pub fn is_fresh(&self, now: i64) -> bool {
        now >= self.generated_at && now < self.expires_at
    }
}

/// A SIGNED generated artifact: the [`ArtifactContent`] plus the signer's ed25519
/// public key (base64) and the signature (base64) over the content's canonical
/// bytes. This is the on-the-wire / on-disk form; it rides the sealed channel as
/// the [`crate::sync::wire::SyncBody::SignedArtifact`] kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedArtifact {
    /// The signed content (version, timestamps, persona id, payload).
    pub content: ArtifactContent,
    /// The signer's ed25519 public key, STANDARD base64. The signature is
    /// verified against this key over [`ArtifactContent::canonical_bytes`].
    pub signer_public_key: String,
    /// The ed25519 signature over the canonical content bytes, STANDARD base64.
    pub signature: String,
}

impl SignedArtifact {
    /// Serialize this artifact to its JSON bytes.
    pub fn to_bytes(&self) -> std::result::Result<Vec<u8>, ArtifactError> {
        serde_json::to_vec(self).map_err(|e| ArtifactError::Canonicalization(e.to_string()))
    }

    /// Parse an artifact from its JSON bytes. Does NOT verify the signature (use
    /// [`verify_artifact`]); a structurally invalid byte string is
    /// [`ArtifactError::Malformed`].
    pub fn from_bytes(bytes: &[u8]) -> std::result::Result<Self, ArtifactError> {
        serde_json::from_slice(bytes).map_err(|e| ArtifactError::Malformed(e.to_string()))
    }

    /// The artifact-kind discriminator of the carried payload.
    pub fn payload_kind(&self) -> &'static str {
        self.content.payload.kind_name()
    }
}

/// Sign `content` with the raw `key` and return the fully-formed
/// [`SignedArtifact`]. Hermetic and low-level: it touches no store and no
/// keystore, so a test can sign with a fixed [`SigningKey`] and verify the result
/// deterministically. Mirrors the persona-pack `sign_pack` primitive.
pub fn sign_artifact(
    content: ArtifactContent,
    key: &SigningKey,
) -> std::result::Result<SignedArtifact, ArtifactError> {
    let bytes = content.canonical_bytes()?;
    let signature = key.sign(&bytes);
    Ok(SignedArtifact {
        content,
        signer_public_key: BASE64.encode(key.verifying_key().to_bytes()),
        signature: BASE64.encode(signature.to_bytes()),
    })
}

/// Sign `content` with a [`PackSigningKey`] (the device's artifact-signing
/// identity, reusing the persona-pack signing key pattern: a key whose seed lives
/// in the OS keystore). Convenience over [`sign_artifact`].
pub fn sign_artifact_with(
    content: ArtifactContent,
    key: &PackSigningKey,
) -> std::result::Result<SignedArtifact, ArtifactError> {
    sign_artifact(content, key.signing_key())
}

/// Verify a generated artifact from its JSON `bytes` and, on success, return the
/// parsed [`SignedArtifact`] whose integrity and signer-key binding are confirmed.
///
/// Fails closed with a typed [`ArtifactError`] for every rejection mode, NEVER a
/// silent accept: malformed bytes, an unknown/newer schema version, a malformed
/// embedded key/signature, or a signature that does not verify (tampered, or
/// signed by a different key). Uses `verify_strict` so non-canonical encodings
/// are rejected. This confirms INTEGRITY and that the embedded key signed it;
/// whether that key is TRUSTED is a separate concern (the sealed channel only
/// accepts artifacts from a paired peer).
pub fn verify_artifact(bytes: &[u8]) -> std::result::Result<SignedArtifact, ArtifactError> {
    let artifact = SignedArtifact::from_bytes(bytes)?;
    verify_parsed_artifact(&artifact)?;
    Ok(artifact)
}

/// Verify an already-parsed [`SignedArtifact`]. Factored out of [`verify_artifact`]
/// so callers holding a parsed artifact (e.g. one opened from the sealed channel)
/// can verify without re-serializing.
pub fn verify_parsed_artifact(artifact: &SignedArtifact) -> std::result::Result<(), ArtifactError> {
    // Reject an unknown/newer schema version BEFORE any crypto work.
    let v = artifact.content.schema_version;
    if !(MIN_SUPPORTED_ARTIFACT_SCHEMA_VERSION..=CURRENT_ARTIFACT_SCHEMA_VERSION).contains(&v) {
        return Err(ArtifactError::UnsupportedSchemaVersion {
            found: v,
            min: MIN_SUPPORTED_ARTIFACT_SCHEMA_VERSION,
            max: CURRENT_ARTIFACT_SCHEMA_VERSION,
        });
    }

    // Decode the embedded public key.
    let pk_bytes = BASE64
        .decode(artifact.signer_public_key.as_bytes())
        .map_err(|e| ArtifactError::InvalidPublicKey(format!("not valid base64: {e}")))?;
    let pk_array: [u8; ed25519_dalek::PUBLIC_KEY_LENGTH] =
        pk_bytes.as_slice().try_into().map_err(|_| {
            ArtifactError::InvalidPublicKey(format!(
                "public key is {} bytes, expected {}",
                pk_bytes.len(),
                ed25519_dalek::PUBLIC_KEY_LENGTH
            ))
        })?;
    let verifying_key = VerifyingKey::from_bytes(&pk_array)
        .map_err(|e| ArtifactError::InvalidPublicKey(e.to_string()))?;

    // Decode the signature.
    let sig_bytes = BASE64
        .decode(artifact.signature.as_bytes())
        .map_err(|e| ArtifactError::InvalidSignature(format!("not valid base64: {e}")))?;
    let sig_array: [u8; SIGNATURE_LENGTH] = sig_bytes.as_slice().try_into().map_err(|_| {
        ArtifactError::InvalidSignature(format!(
            "signature is {} bytes, expected {SIGNATURE_LENGTH}",
            sig_bytes.len()
        ))
    })?;
    let signature = Signature::from_bytes(&sig_array);

    // Recompute the canonical content bytes and verify strictly.
    let canonical = artifact.content.canonical_bytes()?;
    verifying_key
        .verify_strict(&canonical, &signature)
        .map_err(|_| ArtifactError::BadSignature)?;
    Ok(())
}

/// Decide whether to REPLAY a desktop artifact or FALL BACK to on-device
/// generation, modeling the phone's consumer logic.
///
/// `bytes` is the raw signed-artifact JSON (as opened from the sealed channel),
/// or `None` when no desktop artifact is present. The artifact is replayed only
/// when it VERIFIES (signature + integrity) AND is FRESH at `now`; otherwise the
/// caller falls back. Returns the verified, fresh [`SignedArtifact`] to replay,
/// or [`ArtifactDecision::Fallback`] with the reason.
pub fn select_artifact_or_fallback(bytes: Option<&[u8]>, now: i64) -> ArtifactDecision {
    let bytes = match bytes {
        Some(b) => b,
        None => return ArtifactDecision::Fallback(FallbackReason::Absent),
    };
    // Parse only here; the verify + freshness decision is shared with the
    // frame-based consumer (`Core::receive_artifact_frame`) via [`decide_artifact`]
    // so the two fail-closed paths cannot drift. A parse failure is an invalid
    // artifact (fail closed).
    match SignedArtifact::from_bytes(bytes) {
        Ok(artifact) => decide_artifact(artifact, now),
        Err(_) => ArtifactDecision::Fallback(FallbackReason::Invalid),
    }
}

/// The shared replay-vs-fallback decision for a PARSED [`SignedArtifact`]: replay
/// it only if it VERIFIES (signature + integrity + supported schema version) AND
/// is FRESH at `now`; otherwise fall back (fail closed). Both
/// [`select_artifact_or_fallback`] (bytes from a store/file) and
/// [`Core::receive_artifact_frame`](crate::Core::receive_artifact_frame) (an
/// artifact opened from the sealed channel) route through this, so the two
/// consumer paths apply IDENTICAL accept/reject rules and cannot diverge.
pub fn decide_artifact(artifact: SignedArtifact, now: i64) -> ArtifactDecision {
    match verify_parsed_artifact(&artifact) {
        Ok(()) if artifact.content.is_fresh(now) => ArtifactDecision::Replay(Box::new(artifact)),
        Ok(()) => ArtifactDecision::Fallback(FallbackReason::Stale),
        Err(_) => ArtifactDecision::Fallback(FallbackReason::Invalid),
    }
}

/// The outcome of [`select_artifact_or_fallback`]: replay the verified, fresh
/// desktop artifact, or fall back to on-device generation.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ArtifactDecision {
    /// A fresh, valid desktop artifact is present; replay it. Boxed to keep the
    /// enum small (the artifact carries a full payload).
    Replay(Box<SignedArtifact>),
    /// No usable desktop artifact; fall back to on-device generation.
    Fallback(FallbackReason),
}

/// Why the consumer fell back to on-device generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FallbackReason {
    /// No desktop artifact was present at all.
    Absent,
    /// The artifact failed verification (tampered, wrong key, malformed, or an
    /// unsupported version) and was rejected (fail closed).
    Invalid,
    /// The artifact verified but is past its expiry / outside the freshness
    /// window.
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::allocator::{weight_map_from, KL_BUDGET};
    use crate::orchestration::scheduler::IntensityLevel;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region, SyntheticPersona};
    use crate::personapack::{PackSigningKey, PACK_SEED_LEN};

    const FIXED_SEED: [u8; PACK_SEED_LEN] = [11u8; PACK_SEED_LEN];

    fn fixed_key() -> PackSigningKey {
        PackSigningKey::from_seed(&FIXED_SEED)
    }

    fn persona() -> SyntheticPersona {
        SyntheticPersona::new(
            "gen-test-0000-4000-8000-000000000001".to_string(),
            "Gen Test".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            vec![
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
                CategoryPool::FINANCE.as_name().to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    fn interests() -> Vec<CategoryPool> {
        vec![
            CategoryPool::TECHNOLOGY,
            CategoryPool::SCIENCE,
            CategoryPool::FINANCE,
        ]
    }

    fn weight_map() -> WeightMap {
        let blend = weight_map_from(|c| {
            if interests().contains(&c) {
                1.79
            } else {
                0.345
            }
        });
        allocate(&blend, &interests(), KL_BUDGET)
    }

    fn weight_map_content(generated_at: i64) -> ArtifactContent {
        ArtifactContent::new(
            "gen-test-0000-4000-8000-000000000001",
            ArtifactPayload::WeightMap(weight_map()),
            generated_at,
            DEFAULT_FRESHNESS_MS,
        )
    }

    fn query_plan_content(generated_at: i64) -> ArtifactContent {
        let plan = generate_query_plan(&persona(), &weight_map(), IntensityLevel::Medium, 7);
        ArtifactContent::new(
            "gen-test-0000-4000-8000-000000000001",
            ArtifactPayload::QueryPlan(plan),
            generated_at,
            DEFAULT_FRESHNESS_MS,
        )
    }

    #[test]
    fn weight_map_artifact_signs_and_verifies() -> std::result::Result<(), ArtifactError> {
        let artifact = sign_artifact_with(weight_map_content(1_700_000_000_000), &fixed_key())?;
        let bytes = artifact.to_bytes()?;
        let verified = verify_artifact(&bytes)?;
        assert_eq!(verified.payload_kind(), "WeightMap");
        assert_eq!(verified.signer_public_key, fixed_key().public_key_base64());
        Ok(())
    }

    #[test]
    fn query_plan_artifact_signs_and_verifies() -> std::result::Result<(), ArtifactError> {
        let artifact = sign_artifact_with(query_plan_content(1_700_000_000_000), &fixed_key())?;
        let bytes = artifact.to_bytes()?;
        let verified = verify_artifact(&bytes)?;
        assert_eq!(verified.payload_kind(), "QueryPlan");
        Ok(())
    }

    #[test]
    fn tampered_artifact_fails_verification() -> std::result::Result<(), ArtifactError> {
        let artifact = sign_artifact_with(weight_map_content(1_700_000_000_000), &fixed_key())?;
        let mut tampered = artifact.clone();
        // Tamper the signed content: shift the persona id, keep the OLD signature.
        tampered.content.persona_id = "tampered-id".to_string();
        let bytes = tampered.to_bytes()?;
        match verify_artifact(&bytes) {
            Err(ArtifactError::BadSignature) => Ok(()),
            other => panic!("expected BadSignature, got {other:?}"),
        }
    }

    #[test]
    fn raw_byte_flip_in_signed_region_is_rejected() -> std::result::Result<(), ArtifactError> {
        let artifact = sign_artifact_with(weight_map_content(1_700_000_000_000), &fixed_key())?;
        let mut bytes = artifact.to_bytes()?;
        // Flip a byte early in the content region (well before the trailing
        // signature). It must fail to parse or fail to verify, never verify OK.
        bytes[15] ^= 0x01;
        let result = verify_artifact(&bytes);
        assert!(
            matches!(
                result,
                Err(ArtifactError::BadSignature) | Err(ArtifactError::Malformed(_))
            ),
            "a flipped signed byte must be rejected, got {result:?}"
        );
        Ok(())
    }

    #[test]
    fn artifact_signed_by_different_key_fails() -> std::result::Result<(), ArtifactError> {
        let key_a = PackSigningKey::from_seed(&[1u8; PACK_SEED_LEN]);
        let key_b = PackSigningKey::from_seed(&[2u8; PACK_SEED_LEN]);
        let mut artifact = sign_artifact_with(weight_map_content(1_700_000_000_000), &key_a)?;
        // Swap the embedded public key to B's; the signature was made by A.
        artifact.signer_public_key = key_b.public_key_base64();
        let bytes = artifact.to_bytes()?;
        match verify_artifact(&bytes) {
            Err(ArtifactError::BadSignature) => Ok(()),
            other => panic!("expected BadSignature, got {other:?}"),
        }
    }

    #[test]
    fn unknown_newer_schema_version_is_surfaced() -> std::result::Result<(), ArtifactError> {
        let mut content = weight_map_content(1_700_000_000_000);
        content.schema_version = CURRENT_ARTIFACT_SCHEMA_VERSION + 99;
        // Sign the future-version content so the signature is valid; the version
        // check must still reject it.
        let artifact = sign_artifact_with(content, &fixed_key())?;
        let bytes = artifact.to_bytes()?;
        match verify_artifact(&bytes) {
            Err(ArtifactError::UnsupportedSchemaVersion { found, .. }) => {
                assert_eq!(found, CURRENT_ARTIFACT_SCHEMA_VERSION + 99);
                Ok(())
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn malformed_bytes_are_typed_not_silent() {
        let result = verify_artifact(b"this is not an artifact");
        assert!(matches!(result, Err(ArtifactError::Malformed(_))));
    }

    #[test]
    fn freshness_check_selects_replay_vs_fallback() -> std::result::Result<(), ArtifactError> {
        let generated_at = 1_700_000_000_000;
        let artifact = sign_artifact_with(weight_map_content(generated_at), &fixed_key())?;
        let bytes = artifact.to_bytes()?;

        // Fresh: just after generation, well within the 24h window -> replay.
        match select_artifact_or_fallback(Some(&bytes), generated_at + 1_000) {
            ArtifactDecision::Replay(a) => assert_eq!(a.payload_kind(), "WeightMap"),
            other => panic!("expected Replay, got {other:?}"),
        }

        // Stale: past expiry -> fallback.
        let expired = generated_at + DEFAULT_FRESHNESS_MS + 1;
        assert_eq!(
            select_artifact_or_fallback(Some(&bytes), expired),
            ArtifactDecision::Fallback(FallbackReason::Stale)
        );

        // Absent: no artifact -> fallback.
        assert_eq!(
            select_artifact_or_fallback(None, generated_at + 1_000),
            ArtifactDecision::Fallback(FallbackReason::Absent)
        );

        // Invalid: tampered bytes -> fallback (fail closed), even within window.
        let mut bad = artifact.clone();
        bad.content.persona_id = "tampered".to_string();
        let bad_bytes = bad.to_bytes()?;
        assert_eq!(
            select_artifact_or_fallback(Some(&bad_bytes), generated_at + 1_000),
            ArtifactDecision::Fallback(FallbackReason::Invalid)
        );
        Ok(())
    }

    #[test]
    fn is_fresh_boundaries() -> std::result::Result<(), ArtifactError> {
        let content = weight_map_content(1_000);
        assert!(!content.is_fresh(999)); // before generation
        assert!(content.is_fresh(1_000)); // at generation
        assert!(content.is_fresh(1_000 + DEFAULT_FRESHNESS_MS - 1)); // last fresh ms
        assert!(!content.is_fresh(1_000 + DEFAULT_FRESHNESS_MS)); // expiry is exclusive
        Ok(())
    }

    #[test]
    fn artifact_json_uses_camelcase_and_base64_strings() -> std::result::Result<(), ArtifactError> {
        let artifact = sign_artifact_with(weight_map_content(1_700_000_000_000), &fixed_key())?;
        let bytes = artifact.to_bytes()?;
        let text = String::from_utf8(bytes).map_err(|e| ArtifactError::Malformed(e.to_string()))?;
        assert!(text.contains("\"schemaVersion\""));
        assert!(text.contains("\"generatedAt\""));
        assert!(text.contains("\"expiresAt\""));
        assert!(text.contains("\"personaId\""));
        assert!(text.contains("\"signerPublicKey\""));
        assert!(text.contains("\"artifactKind\""));
        assert!(text.contains("\"WeightMap\""));
        // Keys/signatures are base64 strings, not byte arrays.
        assert!(!text.contains("[0,") && !text.contains(",0]"));
        Ok(())
    }
}
