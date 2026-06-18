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

//! Typed error taxonomy for the core. Library code returns these so callers
//! (the CLI and GUI) can match on failure modes rather than parsing strings.
//! The concrete variants grow as subsystems land (store, sync, browser).

use thiserror::Error;

/// Errors surfaced across the `fauxx-core` async API.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A subsystem exists in the API but is not yet implemented. Placeholder
    /// for the C0 skeleton; removed as milestones fill the surface in.
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),

    /// Underlying I/O failure (store access, config read, and similar).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization failure (persona records, wire schema).
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Encrypted-store (SQLCipher) failure: open, migrate, query, or write.
    #[error("store error: {0}")]
    Store(#[from] rusqlite::Error),

    /// OS keystore failure: the credential store is unavailable, or the key
    /// could not be stored/loaded. Also raised when a key fails to decrypt the
    /// database (the store fails closed rather than opening unencrypted).
    #[error("keystore error: {0}")]
    Keystore(String),

    /// Key-material failure: generation, Argon2id derivation, file
    /// wrap/unwrap, a wrong passphrase, or a malformed key file.
    #[error("key error: {0}")]
    Key(String),

    /// A requested entity (e.g. a persona) does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Cross-device sync failure (C1 #7): a crypto seal/open failed
    /// authentication, a wire payload was malformed or an unsupported version,
    /// discovery/transport errored, or a sync was attempted with an unpaired
    /// peer. Sender authentication failures (forged, tampered, or unpaired)
    /// surface here so the channel fails closed.
    #[error("sync error: {0}")]
    Sync(String),

    /// Cross-device persona orchestration failure (C1 #8/#9/#10): a referenced
    /// persona is missing, a coordination operation is invalid for the current
    /// mode, or persisted orchestration state is malformed. Distinct from
    /// [`CoreError::Sync`] so callers can tell a transport/crypto failure apart
    /// from a coordination-logic failure.
    #[error("orchestration error: {0}")]
    Orchestration(String),

    /// DSAR helper failure (C3 #16 D2c): an unknown request kind, an invalid
    /// send timestamp the statutory deadline cannot be computed from, or
    /// malformed persisted request state. Distinct so callers can tell a
    /// letter/deadline problem apart from a store/browser failure.
    #[error("dsar error: {0}")]
    Dsar(String),

    /// Email-alias management failure (C3 #17 D3c): a malformed base address,
    /// an unknown alias kind/status, or a refused no-reuse-across-sites
    /// collision. Distinct so callers can tell an alias-policy problem apart
    /// from a store failure.
    #[error("alias error: {0}")]
    Alias(String),

    /// Account-anchor scanner failure (C3 #19 D5c): an unknown identity signal
    /// or malformed persisted anchor state. The scanner is read-only analysis,
    /// so this never carries an account-automation failure (there is none).
    #[error("anchor error: {0}")]
    Anchor(String),

    /// Signed persona-pack failure (C5 #27 P4): a malformed pack, an unsigned
    /// pack, an unknown/newer pack schema version, an invalid embedded signer
    /// key or signature, or a signature that fails verification (tampered or
    /// signed by a different key). Carries the typed
    /// [`PackError`](crate::personapack::PackError) so callers can match the
    /// exact rejection mode; a pack failure is NEVER a silent accept.
    #[error("persona-pack error: {0}")]
    Pack(#[from] crate::personapack::PackError),

    /// Generated-artifact failure (C6 #28 H1): a malformed artifact, an
    /// unknown/newer artifact schema version, an invalid embedded signer key or
    /// signature, or a signature that fails verification (tampered or signed by a
    /// different key). Carries the typed
    /// [`ArtifactError`](crate::generate::ArtifactError) so callers can match the
    /// exact rejection mode; an artifact failure is NEVER a silent accept, and
    /// the consumer falls back to on-device generation rather than replaying an
    /// unverified or stale artifact.
    #[error("generated-artifact error: {0}")]
    Artifact(#[from] crate::generate::ArtifactError),

    /// Persona-pack minting failure (C6 #29 H2): a malformed/empty PUMS
    /// distribution, an invalid cell (a non-positive weight, an unknown enum name,
    /// or a non-US region), a persistently incoherent draw, or a signing failure.
    /// Carries the typed [`MintError`](crate::mint::MintError) so callers can match
    /// the exact failure mode; minting NEVER emits an incoherent or partial pack.
    #[error("persona-mint error: {0}")]
    Mint(#[from] crate::mint::MintError),

    /// Per-persona network egress / DNS-strategy failure (C7 #30 N1 / #31 N2): a
    /// malformed proxy host or resolver, an unknown persisted egress/DNS kind, or
    /// a credential-store failure for the proxy auth secret. Distinct so callers
    /// can tell a network-config problem apart from a transport/store failure. A
    /// persona whose configured egress is UNREACHABLE is NOT this error: that is
    /// the explicit fail-closed PAUSE state surfaced via the exit indicator
    /// (`EgressExit`), never a silent direct-route fallback.
    #[error("network error: {0}")]
    Network(String),

    /// Real-browser decoy-profile automation failure (C2 #11/#13). Covers the
    /// hard isolation guardrails that fail closed: the configured decoy
    /// user-data dir overlaps a real browser profile, a navigation targets a
    /// blocked authenticated-account sign-in endpoint, or the browser data dir
    /// could not be resolved. Also carries CDP launch/drive/shutdown failures
    /// surfaced by the underlying engine. Distinct so callers can tell a
    /// refused-by-guardrail failure apart from a transport/store failure.
    #[error("browser error: {0}")]
    Browser(String),

    /// Goal-driven campaign-planner failure (C8 #33 U2): a malformed goal (a
    /// non-finite threshold), an unknown persisted lifecycle/comparator value,
    /// or a campaign operation invalid for the campaign's current state.
    /// Distinct so callers can tell a campaign-logic problem apart from a
    /// store/measurement failure. A campaign that simply has not reached its
    /// goal is NOT this error: that is the ordinary `Running`/gap state.
    #[error("campaign error: {0}")]
    Campaign(String),

    /// Home Assistant / MQTT bridge failure (C8 #36 U5): a malformed MQTT
    /// configuration, or an unroutable inbound command. A DOWN broker is NOT
    /// this error: the bridge degrades (publishes are warned-and-dropped, the
    /// poll task reconnects) and never crashes the always-on core, per the
    /// cinder pattern. Distinct so callers can tell a config problem apart from
    /// a transient transport hiccup that needs no caller action.
    #[error("mqtt error: {0}")]
    Mqtt(String),

    /// Debug-logging / log-export failure (the bug-report path): the OS log
    /// directory could not be resolved or created, a log file could not be read,
    /// or the scrubbed export could not be written. Distinct so a logging problem
    /// never masquerades as a store or sync failure; logging is best-effort and a
    /// failure here never aborts the work the log was recording.
    #[error("logging error: {0}")]
    Logging(String),
}

/// Result alias used throughout the core API.
pub type Result<T> = std::result::Result<T, CoreError>;
