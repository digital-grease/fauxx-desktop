# Signed persona packs (C5 #27 P4)

This document describes the import/export persona-pack format implemented in
`fauxx-core::personapack`, the cryptographic guarantees it provides, and the one
guarantee it deliberately leaves to a future iteration.

The GUI library view is a separate, later batch. This batch is the headless
core: the pack format, the sign/verify primitives, and the `Core` async library
operations. No GUI or CLI type appears in this code.

## What a pack is

A persona pack is a versioned, serializable bundle. Its JSON form
(`PersonaPack`) has three top-level parts:

1. `content` (`PackContent`), the SIGNABLE payload:
   - `schemaVersion`: the pack format version (a `u32`), so the format can
     evolve and a reader can refuse a version it does not understand.
   - `provenance` (`PackProvenance`): a US-only, PUMS-style `DemographicCell`
     record aligned with the E7 PUMS model so the pack is portable to the phone.
     It carries the `sourceDistribution` label (e.g. a PUMS vintage), the
     `generationSeed` (so a draw is reproducible), the `createdAt` time in epoch
     milliseconds, and an optional `country` (defaulting to `US`) and `note`.
   - `personas`: one or more `SyntheticPersona` records in the EXACT
     Android-shaped wire model. Because the personas serialize with the frozen
     Android camelCase keys, a pack round-trips to the phone's persona shape.
2. `signerPublicKey`: the signer's ed25519 PUBLIC key, STANDARD base64.
3. `signature`: the ed25519 signature over the canonical content bytes, STANDARD
   base64. It is optional in the type so an UNSIGNED pack is representable and
   detectable; an unsigned pack is flagged on import, never silently accepted.

## Canonicalization (what gets signed)

The signature covers the canonical bytes of `PackContent`, defined as
`serde_json::to_vec(&content)`. By construction `PackContent` has NO signature
field, so the canonical bytes never include the signature. Serde emits struct
fields in declaration order and the field order is fixed, so a signer and a
verifier on the same build derive identical bytes. The canonical bytes are
recomputed from `content` at verify time and compared via the signature; the
signature is never trusted to match the bytes it claims to cover.

## Cryptography

- Signing and verification use `ed25519-dalek` 2.2.
- The signing key is generated from a 32-byte seed produced by the OS CSPRNG via
  `dryoc::rng::copy_randombytes`, then built with `SigningKey::from_bytes`. This
  avoids pulling in `rand_core`: ed25519-dalek 2.2 expects `rand_core` 0.6, which
  would clash with the workspace `rand` 0.10 (`rand_core` 0.9). The transient
  seed buffer is held in a `Zeroizing` wrapper and scrubbed when dropped.
- Public keys and signatures are carried as STANDARD base64 strings in the pack
  JSON. The ed25519-dalek `serde` feature is deliberately NOT enabled, because it
  would emit raw byte arrays instead of compact strings.
- Verification uses `verify_strict`, which rejects non-canonical encodings.

## The signing key lives in the OS keystore

The device's pack-signing seed is stored through the same `KeySource` the
encrypted store uses (`store::store_pack_signing_seed` /
`store::load_pack_signing_seed`). On the OS-keystore path it lives in the
platform credential store under its own account, beside the database key and the
cross-device pairing key, and never in the SQLite plaintext. On the headless
passphrase-file fallback it is wrapped under an Argon2id-derived key with
XChaCha20-Poly1305, exactly like the database and pairing keys. The key is loaded
(or generated and persisted on first run) when `Core::open` runs.

Tests never touch the OS keystore: they sign with a fixed `PackSigningKey` built
from a known 32-byte seed, and the `Core` integration tests use a temp
`EncryptedFile` store.

## Core library operations

The `Core` async API exposes the library operations over the encrypted store:

- `export_persona_pack(persona_ids, provenance) -> Vec<u8>`: pulls the selected
  personas from the store, wraps them with the provenance, signs the canonical
  content with the device key, and returns the pack JSON bytes. An unknown id
  fails closed with `NotFound`.
- `import_persona_pack(bytes) -> Vec<SyntheticPersona>`: VERIFIES the pack, then,
  only on success, lands its personas into the encrypted persona store and
  records the pack in the `installed_packs` library ledger. A rejected pack
  writes nothing. Returns a typed `CoreError::Pack` (wrapping `PackError`) on any
  verification failure.
- `list_installed_packs()` / `get_installed_pack(id)` / `remove_installed_pack(id)`:
  the library ledger. Removing a pack drops only the ledger row; the personas it
  brought in are removed by a separate, explicit `delete_persona` decision.
- `pack_signer_public_key()`: this device's signer public key (base64), surfaced
  so a recipient can record it for the future trust list described below.

The `installed_packs` table is added by a forward-only migration
(`PRAGMA user_version` v11 -> v12) in `store/schema.rs`.

## What verification guarantees, and what it does NOT

`verify_pack(bytes)` establishes:

- INTEGRITY: the `content` was not modified after signing (a flipped byte fails).
- BINDING: the embedded `signerPublicKey` is the key that produced the embedded
  signature over the content (a signature swapped to a different embedded key
  fails).
- WELL-FORMEDNESS: a malformed pack, an unsigned pack, or an unknown/newer
  `schemaVersion` is rejected with a typed error, never a silent accept.

It deliberately does NOT decide whether the signer key is TRUSTED. Today, a pack
signed by ANY key verifies as long as that same key signed its content. Deciding
that the embedded key belongs to a party the user trusts is a FUTURE concern: a
known-keys list, or trust-on-first-use (record the signer key the first time a
pack from it is imported, and warn if a later pack claims to be from a known
party but carries a different key). The `pack_signer_public_key()` accessor and
the `signerPublicKey` recorded in each `installed_packs` row are the hooks that
future iteration will build on.

## Forward compatibility

The pack carries an explicit `schemaVersion`. The current build supports the
window `MIN_SUPPORTED_PACK_SCHEMA_VERSION..=CURRENT_PACK_SCHEMA_VERSION`. The
importer is tolerant of OLDER versions within that window where feasible; a
version outside it (unknown or newer) is surfaced as
`PackError::UnsupportedSchemaVersion`. When the format changes incompatibly,
bump `CURRENT_PACK_SCHEMA_VERSION`, and only raise
`MIN_SUPPORTED_PACK_SCHEMA_VERSION` if a truly unreadable old version must be
dropped.
