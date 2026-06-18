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

//! Signed persona packs (C5 #27 P4): the import/export library format.
//!
//! A persona pack is a versioned, serializable bundle that carries one or more
//! [`SyntheticPersona`](crate::persona::SyntheticPersona) records (the
//! E7-aligned, Android-shaped wire model, so a pack round-trips to the phone),
//! a [`PackProvenance`] record (the source distribution label, the generation
//! seed, and the created-at time), the signer's ed25519 PUBLIC key (base64), and
//! a SIGNATURE (base64) over the CANONICAL serialization of the pack CONTENT
//! (everything except the signature itself).
//!
//! ## What this verifies, and what it does NOT
//!
//! Verification here establishes pack INTEGRITY and that the embedded public key
//! signed the embedded content: a tampered pack (signature fails), an unsigned
//! pack (missing signature), or an unknown/newer [`schema_version`] is REJECTED
//! with a typed [`PackError`], never silently accepted. It deliberately does NOT
//! decide whether the signer key is TRUSTED. Establishing that the embedded key
//! belongs to a party the user trusts (a known-keys list, or trust-on-first-use)
//! is a FUTURE concern layered on top; see `docs/persona-packs.md`. Today a pack
//! signed by any key verifies as long as that same key signed its content.
//!
//! [`schema_version`]: PackContent::schema_version
//!
//! ## Cryptography
//!
//! Signing uses ed25519-dalek over the canonical content bytes. The signing key
//! is generated from a 32-byte seed produced by the OS CSPRNG (dryoc's
//! `copy_randombytes`) and built with `SigningKey::from_bytes`, so no rand_core
//! dependency is pulled in (ed25519-dalek 2.2 wants rand_core 0.6, which would
//! clash with rand 0.10's rand_core 0.9). The seed buffer is zeroized after use.
//! Verification recomputes the canonical content bytes and calls `verify_strict`
//! against the embedded public key. Public keys and signatures travel in the
//! pack JSON as STANDARD base64 strings; the ed25519-dalek `serde` feature is
//! deliberately NOT enabled (it would emit raw byte arrays).
//!
//! ## Canonicalization
//!
//! The bytes that get signed are `serde_json` of the [`PackContent`] struct,
//! which excludes the signature field by construction. Field order is stable
//! because the struct's field order is fixed and serde emits fields in
//! declaration order, so a signer and a verifier on the same build derive
//! identical bytes. The pack version is carried so the format can evolve while
//! older readers stay tolerant where feasible.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::persona::SyntheticPersona;

/// Length of an ed25519 signing SEED / secret-key scalar, in bytes (32). The
/// public key and signature are 32 and 64 bytes respectively.
pub const PACK_SEED_LEN: usize = ed25519_dalek::SECRET_KEY_LENGTH;

/// The CURRENT persona-pack schema version this build writes. Bumped when the
/// pack format changes incompatibly. Older packs (lower versions) are imported
/// tolerantly where feasible; an unknown/NEWER version is rejected as a typed
/// [`PackError::UnsupportedSchemaVersion`] rather than silently accepted.
pub const CURRENT_PACK_SCHEMA_VERSION: u32 = 1;

/// The lowest pack schema version this build can still import. Packs at or above
/// this and at or below [`CURRENT_PACK_SCHEMA_VERSION`] are accepted; the
/// importer stays tolerant of older versions in this window.
pub const MIN_SUPPORTED_PACK_SCHEMA_VERSION: u32 = 1;

/// Typed validation/verification failures for persona packs. Returned so callers
/// (and the later library GUI) can match the failure mode rather than parsing a
/// string. A failure is NEVER a silent accept.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackError {
    /// The pack bytes were not valid pack JSON, or a field was malformed.
    #[error("malformed persona pack: {0}")]
    Malformed(String),

    /// The pack declared a schema version this build does not support (unknown,
    /// or newer than [`CURRENT_PACK_SCHEMA_VERSION`]). Surfaced, never accepted.
    #[error("unsupported persona-pack schema version {found} (this build supports {min}..={max})")]
    UnsupportedSchemaVersion {
        /// The version the pack declared.
        found: u32,
        /// The lowest supported version.
        min: u32,
        /// The highest supported version.
        max: u32,
    },

    /// The pack carried no signature (an UNSIGNED pack). Flagged, never accepted.
    #[error("persona pack is unsigned (no signature present)")]
    Unsigned,

    /// The embedded signer public key was not valid base64, or not a valid
    /// ed25519 public key.
    #[error("invalid signer public key: {0}")]
    InvalidPublicKey(String),

    /// The embedded signature was not valid base64, or not a 64-byte ed25519
    /// signature.
    #[error("invalid signature encoding: {0}")]
    InvalidSignature(String),

    /// The signature did not verify against the embedded public key over the
    /// canonical content bytes: the pack was TAMPERED with, or it was signed by
    /// a different key than the one embedded. Rejected.
    #[error("persona-pack signature verification failed (tampered or wrong key)")]
    BadSignature,

    /// The canonical content bytes could not be produced (a serialization
    /// failure while signing or verifying).
    #[error("persona-pack canonicalization failed: {0}")]
    Canonicalization(String),
}

/// US-only PUMS-style provenance for a pack, aligned with the E7 PUMS model so a
/// pack is portable to the phone. A `DemographicCell`-style record: the source
/// distribution it was sampled from, the generation seed (so a draw is
/// reproducible), and when it was created.
///
/// Serialized in camelCase to match the Android JSON convention; additive
/// optional fields are omitted when unset so the phone's lenient reader never
/// sees keys it does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackProvenance {
    /// The source distribution label this pack was sampled from (e.g. a PUMS
    /// vintage like `"US_PUMS_2022"`). Free-form so a future distribution slots
    /// in without a format bump.
    pub source_distribution: String,
    /// The generation seed the personas were drawn with, so the draw is
    /// reproducible. A string so large/opaque seeds round-trip losslessly.
    pub generation_seed: String,
    /// Creation time, epoch milliseconds. JSON key `createdAt`.
    pub created_at: i64,
    /// Optional country code for the distribution (US-only today). Additive;
    /// omitted from JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Optional free-form note describing the pack. Additive; omitted when
    /// `None` so the phone never sees the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl PackProvenance {
    /// A US-distribution provenance with the given source label, seed, and
    /// creation time. `country` defaults to `"US"` (US-only today); `note` is
    /// unset.
    pub fn us(
        source_distribution: impl Into<String>,
        generation_seed: impl Into<String>,
        created_at: i64,
    ) -> Self {
        Self {
            source_distribution: source_distribution.into(),
            generation_seed: generation_seed.into(),
            created_at,
            country: Some("US".to_string()),
            note: None,
        }
    }
}

/// The SIGNABLE content of a persona pack: everything the signature covers. By
/// construction this struct has NO signature field, so `serde_json` of it is
/// exactly the canonical bytes that get signed and verified. Field order is
/// fixed (serde emits in declaration order), which keeps the canonical bytes
/// stable across a signer and a verifier on the same build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackContent {
    /// The pack format version. A reader rejects an unknown/newer version with a
    /// typed [`PackError::UnsupportedSchemaVersion`] rather than guessing.
    pub schema_version: u32,
    /// Where this pack came from (source distribution, seed, created-at).
    pub provenance: PackProvenance,
    /// The personas this pack carries, in the exact Android-shaped wire model,
    /// so a pack round-trips to the phone's `SyntheticPersona`.
    pub personas: Vec<SyntheticPersona>,
}

impl PackContent {
    /// Build pack content at [`CURRENT_PACK_SCHEMA_VERSION`] from a provenance
    /// record and the personas to carry.
    pub fn new(provenance: PackProvenance, personas: Vec<SyntheticPersona>) -> Self {
        Self {
            schema_version: CURRENT_PACK_SCHEMA_VERSION,
            provenance,
            personas,
        }
    }

    /// The canonical bytes that the signature covers: `serde_json` of this
    /// content struct (which has no signature field). Stable for a given content
    /// because serde emits fields in declaration order.
    pub fn canonical_bytes(&self) -> std::result::Result<Vec<u8>, PackError> {
        serde_json::to_vec(self).map_err(|e| PackError::Canonicalization(e.to_string()))
    }
}

/// A signed persona pack: the [`PackContent`] plus the signer's ed25519 public
/// key (base64) and the signature (base64) over the content's canonical bytes.
///
/// This is the on-the-wire / on-disk format. Serializes to camelCase JSON. The
/// `signature` is `Option` so an UNSIGNED pack is representable and detectable
/// (it is flagged, never silently accepted) rather than being unconstructable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaPack {
    /// The signed content (schema version, provenance, personas).
    pub content: PackContent,
    /// The signer's ed25519 public key, STANDARD base64. The signature is
    /// verified against this key over [`PackContent::canonical_bytes`].
    pub signer_public_key: String,
    /// The ed25519 signature over the canonical content bytes, STANDARD base64.
    /// `None` marks an UNSIGNED pack, which import flags as
    /// [`PackError::Unsigned`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl PersonaPack {
    /// Serialize this pack to its JSON bytes (the import/export byte form).
    pub fn to_bytes(&self) -> std::result::Result<Vec<u8>, PackError> {
        serde_json::to_vec(self).map_err(|e| PackError::Canonicalization(e.to_string()))
    }

    /// Parse a pack from its JSON bytes. A non-JSON / structurally invalid byte
    /// string is a typed [`PackError::Malformed`]; this does NOT verify the
    /// signature (use [`verify_pack`] for that).
    pub fn from_bytes(bytes: &[u8]) -> std::result::Result<Self, PackError> {
        serde_json::from_slice(bytes).map_err(|e| PackError::Malformed(e.to_string()))
    }
}

/// A device's persona-pack signing identity: an ed25519 keypair built from a
/// 32-byte seed. The seed is held in a zeroizing wrapper and scrubbed on drop;
/// only the public key is exposed in any serializable form.
pub struct PackSigningKey {
    signing: SigningKey,
}

impl std::fmt::Debug for PackSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the secret scalar; show only the public key fingerprint.
        f.debug_struct("PackSigningKey")
            .field("public_key_base64", &self.public_key_base64())
            .finish_non_exhaustive()
    }
}

impl PackSigningKey {
    /// Generate a fresh signing key from the OS CSPRNG. A 32-byte seed is drawn
    /// via dryoc's `copy_randombytes` and the key is built with
    /// `SigningKey::from_bytes`, so NO rand_core dependency is pulled in. The
    /// transient seed buffer is zeroized after the key is built.
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; PACK_SEED_LEN]);
        dryoc::rng::copy_randombytes(seed.as_mut_slice());
        let key = Self::from_seed(&seed);
        // `seed` is zeroized on drop (Zeroizing).
        key
    }

    /// Build a signing key from a fixed 32-byte seed. Used by tests (a known
    /// seed yields a deterministic key) and to reconstruct the device key loaded
    /// from the OS keystore.
    pub fn from_seed(seed: &[u8; PACK_SEED_LEN]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Build a signing key from a seed slice, failing closed on the wrong
    /// length rather than padding or truncating.
    pub fn from_seed_slice(seed: &[u8]) -> std::result::Result<Self, PackError> {
        let seed: [u8; PACK_SEED_LEN] = seed.try_into().map_err(|_| {
            PackError::InvalidPublicKey(format!(
                "pack signing seed is {} bytes, expected {PACK_SEED_LEN}",
                seed.len()
            ))
        })?;
        Ok(Self::from_seed(&seed))
    }

    /// The 32-byte seed (secret scalar), for persistence into the OS keystore
    /// only. Returned in a zeroizing wrapper so the copy is scrubbed. Crate-
    /// private so no public API or log can reach the secret.
    pub(crate) fn seed_bytes(&self) -> Zeroizing<[u8; PACK_SEED_LEN]> {
        Zeroizing::new(self.signing.to_bytes())
    }

    /// The signer's ed25519 public key as a STANDARD base64 string (the form
    /// embedded in a pack and used to verify it).
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.signing.verifying_key().to_bytes())
    }

    /// Borrow the raw ed25519 [`SigningKey`] this device key wraps. Crate-private
    /// so the secret scalar is never reachable from the public API; used by the
    /// C6 generate module to sign artifacts with the SAME device key the persona
    /// packs use (the low-level [`sign_pack`] / [`generate::sign_artifact`]
    /// primitives take a `&SigningKey`).
    ///
    /// [`generate::sign_artifact`]: crate::generate::sign_artifact
    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing
    }
}

/// Sign `content` with `key` and return the fully-formed signed [`PersonaPack`]
/// (content + embedded public key base64 + signature base64). Hermetic and
/// low-level: it touches no store and no keystore, so a test can sign with a
/// fixed [`PackSigningKey`] and verify the result deterministically.
pub fn sign_pack(
    content: PackContent,
    key: &SigningKey,
) -> std::result::Result<PersonaPack, PackError> {
    let bytes = content.canonical_bytes()?;
    let signature = key.sign(&bytes);
    Ok(PersonaPack {
        content,
        signer_public_key: BASE64.encode(key.verifying_key().to_bytes()),
        signature: Some(BASE64.encode(signature.to_bytes())),
    })
}

/// Sign `content` with a [`PackSigningKey`] (the higher-level wrapper around the
/// raw [`SigningKey`]). Convenience over [`sign_pack`].
pub fn sign_pack_with(
    content: PackContent,
    key: &PackSigningKey,
) -> std::result::Result<PersonaPack, PackError> {
    sign_pack(content, &key.signing)
}

/// Verify a persona pack from its JSON `bytes` and, on success, return the
/// parsed [`PersonaPack`] whose integrity and signer-key binding are confirmed.
///
/// Fails closed with a typed [`PackError`] for every rejection mode, NEVER a
/// silent accept:
/// - non-JSON / structurally invalid bytes: [`PackError::Malformed`];
/// - an unknown/newer schema version: [`PackError::UnsupportedSchemaVersion`];
/// - a missing signature (unsigned pack): [`PackError::Unsigned`];
/// - a malformed embedded public key / signature:
///   [`PackError::InvalidPublicKey`] / [`PackError::InvalidSignature`];
/// - a signature that does not verify against the embedded key over the
///   recomputed canonical content bytes (tampered, or signed by a different
///   key): [`PackError::BadSignature`].
///
/// Verification uses `verify_strict` so non-canonical signature/public-key
/// encodings are rejected. This confirms pack INTEGRITY and that the embedded
/// key signed it; whether that key is TRUSTED is a separate, future concern.
pub fn verify_pack(bytes: &[u8]) -> std::result::Result<PersonaPack, PackError> {
    let pack = PersonaPack::from_bytes(bytes)?;
    verify_parsed_pack(&pack)?;
    Ok(pack)
}

/// Verify an already-parsed [`PersonaPack`]. Factored out of [`verify_pack`] so
/// callers that already hold a parsed pack (e.g. after constructing one in a
/// test) can verify without re-serializing.
pub fn verify_parsed_pack(pack: &PersonaPack) -> std::result::Result<(), PackError> {
    // Reject an unknown/newer schema version BEFORE any crypto work, so the
    // failure mode is unambiguous and never depends on a downstream parse.
    let v = pack.content.schema_version;
    if !(MIN_SUPPORTED_PACK_SCHEMA_VERSION..=CURRENT_PACK_SCHEMA_VERSION).contains(&v) {
        return Err(PackError::UnsupportedSchemaVersion {
            found: v,
            min: MIN_SUPPORTED_PACK_SCHEMA_VERSION,
            max: CURRENT_PACK_SCHEMA_VERSION,
        });
    }

    // An unsigned pack is flagged, never accepted.
    let signature_b64 = pack.signature.as_deref().ok_or(PackError::Unsigned)?;

    // Decode the embedded public key.
    let pk_bytes = BASE64
        .decode(pack.signer_public_key.as_bytes())
        .map_err(|e| PackError::InvalidPublicKey(format!("not valid base64: {e}")))?;
    let pk_array: [u8; ed25519_dalek::PUBLIC_KEY_LENGTH] =
        pk_bytes.as_slice().try_into().map_err(|_| {
            PackError::InvalidPublicKey(format!(
                "public key is {} bytes, expected {}",
                pk_bytes.len(),
                ed25519_dalek::PUBLIC_KEY_LENGTH
            ))
        })?;
    let verifying_key = VerifyingKey::from_bytes(&pk_array)
        .map_err(|e| PackError::InvalidPublicKey(e.to_string()))?;

    // Decode the signature.
    let sig_bytes = BASE64
        .decode(signature_b64.as_bytes())
        .map_err(|e| PackError::InvalidSignature(format!("not valid base64: {e}")))?;
    let sig_array: [u8; SIGNATURE_LENGTH] = sig_bytes.as_slice().try_into().map_err(|_| {
        PackError::InvalidSignature(format!(
            "signature is {} bytes, expected {SIGNATURE_LENGTH}",
            sig_bytes.len()
        ))
    })?;
    let signature = Signature::from_bytes(&sig_array);

    // Recompute the canonical content bytes and verify strictly.
    let canonical = pack.content.canonical_bytes()?;
    verifying_key
        .verify_strict(&canonical, &signature)
        .map_err(|_| PackError::BadSignature)?;
    Ok(())
}

/// A ledger row recording a signed pack that was IMPORTED into this device's
/// library (C5 #27 P4). The personas a pack carried land in the persona store
/// proper; this is the library index of WHAT was installed: its provenance, the
/// signer key, the persona ids it brought in, and when. The full record is
/// persisted as JSON in the `installed_packs` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackRecord {
    /// A UUID minted on import; the library's stable handle for this pack.
    pub id: String,
    /// The pack's provenance (source distribution, seed, created-at).
    pub provenance: PackProvenance,
    /// The signer's ed25519 public key, STANDARD base64 (as embedded in the
    /// pack and used to verify it on import).
    pub signer_public_key: String,
    /// The pack format version the pack declared.
    pub schema_version: u32,
    /// The persona ids this pack brought into the library, in pack order.
    pub persona_ids: Vec<String>,
    /// When the pack was imported, epoch milliseconds.
    pub imported_at: i64,
}

impl PackRecord {
    /// Build a ledger record from a verified pack, the import time, and a freshly
    /// minted id.
    pub fn from_pack(id: impl Into<String>, pack: &PersonaPack, imported_at: i64) -> Self {
        Self {
            id: id.into(),
            provenance: pack.content.provenance.clone(),
            signer_public_key: pack.signer_public_key.clone(),
            schema_version: pack.content.schema_version,
            persona_ids: pack.content.personas.iter().map(|p| p.id.clone()).collect(),
            imported_at,
        }
    }

    /// How many personas the pack carried.
    pub fn persona_count(&self) -> usize {
        self.persona_ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    /// A fixed 32-byte seed so the signing key is deterministic in tests (never
    /// the OS keystore).
    const FIXED_SEED: [u8; PACK_SEED_LEN] = [7u8; PACK_SEED_LEN];

    fn fixed_key() -> PackSigningKey {
        PackSigningKey::from_seed(&FIXED_SEED)
    }

    fn sample_persona(id: &str) -> SyntheticPersona {
        SyntheticPersona::new(
            id.to_string(),
            "Pack Persona".to_string(),
            AgeRange::AGE_35_44.as_name().to_string(),
            Profession::FINANCE_PROF.as_name().to_string(),
            Region::US_MIDWEST.as_name().to_string(),
            vec![
                CategoryPool::FINANCE.as_name().to_string(),
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::TRAVEL.as_name().to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    fn sample_content() -> PackContent {
        PackContent::new(
            PackProvenance::us("US_PUMS_2022", "seed-1234", 1_700_000_000_000),
            vec![
                sample_persona("11111111-1111-4111-8111-111111111111"),
                sample_persona("22222222-2222-4222-8222-222222222222"),
            ],
        )
    }

    #[test]
    fn sign_then_verify_round_trips() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let pack = sign_pack(sample_content(), key_signing(&key))?;
        let bytes = pack.to_bytes()?;
        let verified = verify_pack(&bytes)?;
        assert_eq!(verified.content, pack.content);
        assert_eq!(verified.signer_public_key, key.public_key_base64());
        Ok(())
    }

    /// Borrow the raw `SigningKey` for the low-level [`sign_pack`] from a test
    /// wrapper without exposing it on the public API.
    fn key_signing(key: &PackSigningKey) -> &SigningKey {
        &key.signing
    }

    #[test]
    fn tampered_content_byte_fails_verification() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let pack = sign_pack(sample_content(), key_signing(&key))?;
        let mut bytes = pack.to_bytes()?;

        // Flip a byte inside the signed content (the persona name region) and
        // confirm verification rejects it. We mutate a content byte by parsing,
        // editing a persona, re-serializing the PACK but keeping the OLD
        // signature, which models a tampered-after-signing pack.
        let mut tampered = PersonaPack::from_bytes(&bytes)?;
        tampered.content.personas[0].name = "Tampered".to_string();
        bytes = tampered.to_bytes()?;

        match verify_pack(&bytes) {
            Err(PackError::BadSignature) => Ok(()),
            other => panic!("expected BadSignature, got {other:?}"),
        }
    }

    #[test]
    fn raw_byte_flip_in_signed_region_is_rejected() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let pack = sign_pack(sample_content(), key_signing(&key))?;
        let mut bytes = pack.to_bytes()?;
        // Flip a byte in the content region (early in the JSON, well before the
        // trailing signature field). It must either fail to parse or fail to
        // verify; it must NEVER verify as valid.
        bytes[20] ^= 0x01;
        let result = verify_pack(&bytes);
        assert!(
            matches!(
                result,
                Err(PackError::BadSignature) | Err(PackError::Malformed(_))
            ),
            "a flipped signed byte must be rejected, got {result:?}"
        );
        Ok(())
    }

    #[test]
    fn unsigned_pack_is_flagged() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let mut pack = sign_pack(sample_content(), key_signing(&key))?;
        pack.signature = None;
        let bytes = pack.to_bytes()?;
        match verify_pack(&bytes) {
            Err(PackError::Unsigned) => Ok(()),
            other => panic!("expected Unsigned, got {other:?}"),
        }
    }

    #[test]
    fn pack_signed_by_different_key_fails_against_tampered_embedded_key(
    ) -> std::result::Result<(), PackError> {
        // Sign with key A, then swap the embedded public key to key B's. The
        // signature was made by A over the content, so verifying against B's key
        // must fail.
        let key_a = PackSigningKey::from_seed(&[1u8; PACK_SEED_LEN]);
        let key_b = PackSigningKey::from_seed(&[2u8; PACK_SEED_LEN]);
        let mut pack = sign_pack(sample_content(), key_signing(&key_a))?;
        pack.signer_public_key = key_b.public_key_base64();
        let bytes = pack.to_bytes()?;
        match verify_pack(&bytes) {
            Err(PackError::BadSignature) => Ok(()),
            other => panic!("expected BadSignature, got {other:?}"),
        }
    }

    #[test]
    fn unknown_newer_schema_version_is_surfaced() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let mut content = sample_content();
        content.schema_version = CURRENT_PACK_SCHEMA_VERSION + 99;
        // Sign the future-version content so the signature itself is valid; the
        // version check must still REJECT it before/regardless of crypto.
        let pack = sign_pack(content, key_signing(&key))?;
        let bytes = pack.to_bytes()?;
        match verify_pack(&bytes) {
            Err(PackError::UnsupportedSchemaVersion { found, .. }) => {
                assert_eq!(found, CURRENT_PACK_SCHEMA_VERSION + 99);
                Ok(())
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn persona_json_inside_pack_keeps_android_camelcase_keys() -> std::result::Result<(), PackError>
    {
        let key = fixed_key();
        let pack = sign_pack(sample_content(), key_signing(&key))?;
        let bytes = pack.to_bytes()?;
        let text = String::from_utf8(bytes).map_err(|e| PackError::Malformed(e.to_string()))?;
        // The exact Android camelCase persona keys must appear verbatim.
        for key in [
            "\"ageRange\"",
            "\"profession\"",
            "\"region\"",
            "\"interests\"",
            "\"createdAt\"",
            "\"activeUntil\"",
        ] {
            assert!(
                text.contains(key),
                "expected persona key {key} in pack JSON"
            );
        }
        // Public key + signature are base64 strings (NOT byte arrays), proving
        // the ed25519-dalek serde feature is not in play.
        assert!(text.contains("\"signerPublicKey\""));
        assert!(text.contains("\"signature\""));
        assert!(
            !text.contains("[0,") && !text.contains(",0]"),
            "keys must be base64 strings, not byte arrays"
        );
        Ok(())
    }

    #[test]
    fn persona_round_trips_to_synthetic_persona_through_pack() -> std::result::Result<(), PackError>
    {
        let key = fixed_key();
        let original = sample_content();
        let pack = sign_pack(original.clone(), key_signing(&key))?;
        let bytes = pack.to_bytes()?;
        let verified = verify_pack(&bytes)?;
        // The personas survive the pack round-trip byte-faithfully.
        assert_eq!(verified.content.personas, original.personas);
        Ok(())
    }

    #[test]
    fn malformed_bytes_are_typed_not_silent() {
        let result = verify_pack(b"this is not a persona pack");
        assert!(matches!(result, Err(PackError::Malformed(_))));
    }

    #[test]
    fn from_seed_is_deterministic_and_generate_differs() {
        // The same seed yields the same public key.
        let a = PackSigningKey::from_seed(&FIXED_SEED);
        let b = PackSigningKey::from_seed(&FIXED_SEED);
        assert_eq!(a.public_key_base64(), b.public_key_base64());
        // A fresh random key almost surely differs (1 in 2^256 collision).
        let fresh = PackSigningKey::generate();
        assert_ne!(fresh.public_key_base64(), a.public_key_base64());
    }

    #[test]
    fn debug_redacts_secret_seed() {
        let key = fixed_key();
        let rendered = format!("{key:?}");
        // The public key fingerprint is shown; the raw seed bytes are not.
        assert!(rendered.contains("public_key_base64"));
        assert!(!rendered.contains(&format!("{:?}", FIXED_SEED)));
    }

    #[test]
    fn seed_round_trips_through_slice() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let seed = key.seed_bytes();
        let restored = PackSigningKey::from_seed_slice(seed.as_slice())?;
        assert_eq!(restored.public_key_base64(), key.public_key_base64());
        // Wrong length fails closed.
        assert!(PackSigningKey::from_seed_slice(&[0u8; 10]).is_err());
        Ok(())
    }

    #[test]
    fn pack_record_summarizes_a_verified_pack() -> std::result::Result<(), PackError> {
        let key = fixed_key();
        let pack = sign_pack(sample_content(), key_signing(&key))?;
        let record = PackRecord::from_pack("pack-1", &pack, 1_700_500_000_000);
        assert_eq!(record.persona_count(), 2);
        assert_eq!(record.signer_public_key, key.public_key_base64());
        assert_eq!(record.schema_version, CURRENT_PACK_SCHEMA_VERSION);
        assert_eq!(
            record.persona_ids,
            vec![
                "11111111-1111-4111-8111-111111111111".to_string(),
                "22222222-2222-4222-8222-222222222222".to_string(),
            ]
        );
        // The record serializes with camelCase keys.
        let json =
            serde_json::to_string(&record).map_err(|e| PackError::Malformed(e.to_string()))?;
        assert!(json.contains("\"signerPublicKey\""));
        assert!(json.contains("\"personaIds\""));
        assert!(json.contains("\"importedAt\""));
        Ok(())
    }
}
