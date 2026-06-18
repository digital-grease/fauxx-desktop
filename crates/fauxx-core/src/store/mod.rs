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

//! The encrypted local store.
//!
//! [`EncryptedStore`] wraps a SQLCipher database whose key is sourced through
//! [`KeySource`] (OS keystore or an Argon2id-wrapped key file). It is the only
//! way the core touches persisted state; there is no public database handle.
//!
//! The store **fails closed**: if the key cannot be loaded/derived, or the key
//! does not decrypt the database, opening returns a [`CoreError`] and the
//! database is never accessed unencrypted.
//!
//! Persona records are persisted as the exact Android-compatible JSON (see
//! [`crate::persona`]), so a desktop write reads back identically on the phone.

mod keystore;
mod schema;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

pub use keystore::{
    delete_proxy_credentials, load_device_keypair, load_pack_signing_seed, load_proxy_credentials,
    store_device_keypair, store_pack_signing_seed, store_proxy_credentials, DbKey, KeySource,
    DEVICE_KEYPAIR_LEN, KEY_LEN, PACK_SEED_LEN,
};
pub use schema::SCHEMA_VERSION;

// Re-exported below: `GpcSiteStatus` and `TopicsMeasurement` are defined in this
// module; `EfficacyRecord` likewise. Listed here for discoverability alongside
// the keystore re-exports.

use crate::aliases::{AliasStatus, EmailAlias};
use crate::anchors::AccountAnchor;
use crate::brokers::{BrokerScanSnapshot, BrokerSubmission};
use crate::browser::{AssignedTopic, GpcSupport};
use crate::campaigns::Campaign;
use crate::dsar::DsarRequest;
use crate::error::{CoreError, Result};
use crate::measurement::ShadowProfile;
use crate::network::{DnsStrategy, Egress};
use crate::persona::SyntheticPersona;
use crate::personapack::PackRecord;
use crate::studio::PersonaSettings;

/// Default application qualifier/org/app used to locate the data directory.
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";
/// Database file name under the data directory.
const DB_FILE: &str = "fauxx.db";

/// A row in `efficacy_history`. The metric shape is finalized in a later
/// milestone; for C0 it is an opaque label plus score with a timestamp.
#[derive(Debug, Clone, PartialEq)]
pub struct EfficacyRecord {
    /// Persona this measurement belongs to.
    pub persona_id: String,
    /// Epoch milliseconds when the measurement was taken.
    pub recorded_at: i64,
    /// Opaque metric identifier/payload (JSON or label).
    pub metric: String,
    /// Numeric score for the metric.
    pub score: f64,
}

/// One Privacy Sandbox Topics read-back measurement (C2 #12 R2): the decoy
/// profile's own `document.browsingTopics()` result, taken after seeding
/// category-targeted history, persisted as part of the closed loop.
///
/// A measurement with an empty [`topics`](Self::topics) list is a VALID record,
/// not a failure: topics are computed per weekly epoch, so a read right after
/// seeding commonly returns nothing until the epoch rolls. The
/// [`available`](Self::available) flag records whether the API was callable at
/// all (flags on, secure context). Persisted via [`EncryptedStore::insert_topics_measurement`]
/// and read back with [`EncryptedStore::topics_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicsMeasurement {
    /// Persona this read-back belongs to.
    pub persona_id: String,
    /// The decoy profile id the read came from.
    pub decoy_id: String,
    /// Epoch milliseconds when the read-back was taken.
    pub recorded_at: i64,
    /// Whether `document.browsingTopics()` was callable (flags on, secure
    /// context). `false` means the API was unavailable in that context.
    pub available: bool,
    /// The parsed assigned topics. Commonly EMPTY inside the epoch observation
    /// window; an empty list is well-formed and intentionally persisted.
    pub topics: Vec<AssignedTopic>,
}

/// One per-site GPC-honoring observation (D4c #18): the most recently parsed
/// `/.well-known/gpc.json` result for a site origin, with the time it was
/// checked. Persisted via [`EncryptedStore::upsert_gpc_status`] and read back
/// with [`EncryptedStore::gpc_status_for`] / [`EncryptedStore::list_gpc_status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpcSiteStatus {
    /// The site origin checked (e.g. `https://example.com`).
    pub origin: String,
    /// Epoch milliseconds when the check was performed.
    pub checked_at: i64,
    /// The parsed support observation (`honored` plus optional advertised meta).
    pub support: GpcSupport,
}

/// One row in the `installed_packs` library ledger (C5 #27 P4): a signed
/// persona pack that was imported into this device. Wraps the
/// [`PackRecord`](crate::personapack::PackRecord) (provenance, signer key,
/// persona ids) the pack carried; the personas themselves live in the
/// `personas` table. Persisted via [`EncryptedStore::upsert_installed_pack`] and
/// read back with [`EncryptedStore::list_installed_packs`] /
/// [`EncryptedStore::get_installed_pack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPack {
    /// The library record describing the imported pack.
    pub record: PackRecord,
}

impl InstalledPack {
    /// Wrap a [`PackRecord`] as an installed-pack ledger row.
    pub fn new(record: PackRecord) -> Self {
        Self { record }
    }
}

/// The encrypted store. Holds the open SQLCipher connection privately; all
/// access is through this type's methods.
pub struct EncryptedStore {
    conn: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for EncryptedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the connection (could leak key material in some builds).
        f.debug_struct("EncryptedStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl EncryptedStore {
    /// Resolve the default database path under the OS data directory.
    ///
    /// Returns an error if no home/data directory can be determined.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
            .ok_or_else(|| {
                CoreError::Keystore("could not determine OS data directory".to_string())
            })?;
        Ok(dirs.data_dir().join(DB_FILE))
    }

    /// Open (or create) the encrypted database at [`default_path`] using the
    /// given key source.
    ///
    /// [`default_path`]: EncryptedStore::default_path
    pub fn open(source: &KeySource) -> Result<Self> {
        let path = Self::default_path()?;
        Self::open_at(&path, source)
    }

    /// Open (or create) the encrypted database at `path` using `source`.
    ///
    /// Fails closed: the key is applied via `PRAGMA key` and then verified by
    /// touching the database; a missing key or wrong key returns an error and
    /// the database is never used unencrypted.
    pub fn open_at(path: &Path, source: &KeySource) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Fail closed: obtain the key *before* opening; any failure aborts.
        let key = source.load_or_create()?;

        let conn = Connection::open(path)?;
        Self::apply_key(&conn, &key)?;
        // Verify the key actually decrypts the database. On a wrong key this
        // is the first statement that touches encrypted pages and errors out.
        Self::verify_key(&conn)?;

        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };
        schema::migrate(&mut store.conn)?;
        Ok(store)
    }

    /// Apply the SQLCipher key with `PRAGMA key`. Uses the raw-hex form
    /// (`x'..'`) so SQLCipher skips its own KDF over our already-random key.
    fn apply_key(conn: &Connection, key: &DbKey) -> Result<()> {
        let hex = key.to_hex();
        // `PRAGMA key` does not accept bound parameters; the value is our own
        // hex-encoded random key, never user input, so there is no injection
        // surface. We still constrain it to hex above.
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex.as_str()))?;
        // Reasonable, explicit cipher settings (defaults for SQLCipher 4).
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    }

    /// Touch the database so a wrong key surfaces immediately as an error
    /// (SQLCipher reports the failure on first access of an encrypted page).
    fn verify_key(conn: &Connection) -> Result<()> {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|_| ())
        .map_err(|e| match e {
            // Map the opaque "not a database" / "file is encrypted" error to a
            // clear keystore failure so callers can tell key problems apart.
            rusqlite::Error::SqliteFailure(_, _) => {
                CoreError::Keystore("key does not decrypt the database".to_string())
            }
            other => CoreError::Store(other),
        })
    }

    /// The on-disk path of this store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // --- Persona CRUD -------------------------------------------------------

    /// List all stored personas, newest-created first.
    pub fn list_personas(&self) -> Result<Vec<SyntheticPersona>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM personas ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            out.push(serde_json::from_str(&json)?);
        }
        Ok(out)
    }

    /// Fetch a single persona by id, or `None` if absent.
    pub fn get_persona(&self, id: &str) -> Result<Option<SyntheticPersona>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM personas WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Insert or replace a persona, keyed on its id. Persisted as the exact
    /// Android-compatible JSON.
    pub fn save_persona(&self, persona: &SyntheticPersona) -> Result<()> {
        let json = serde_json::to_string(persona)?;
        self.conn.execute(
            "INSERT INTO personas (id, json, created_at, active_until)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 json = excluded.json,
                 created_at = excluded.created_at,
                 active_until = excluded.active_until",
            rusqlite::params![persona.id, json, persona.created_at, persona.active_until],
        )?;
        Ok(())
    }

    /// Delete a persona by id. Returns `true` if a row was removed.
    pub fn delete_persona(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM personas WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- Persona Studio editor settings (C5 #24 P1) -------------------------

    /// Fetch the desktop-local [`PersonaSettings`] for a persona, or `None` if
    /// none have been saved yet. These are editor-only metadata (locked fields +
    /// rotation), kept OUT of the synced persona JSON.
    pub fn get_persona_settings(&self, persona_id: &str) -> Result<Option<PersonaSettings>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM persona_settings WHERE persona_id = ?1",
                [persona_id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Insert or replace the desktop-local [`PersonaSettings`] for a persona,
    /// keyed on its persona id. The full JSON is stored verbatim.
    pub fn save_persona_settings(&self, settings: &PersonaSettings) -> Result<()> {
        let json = serde_json::to_string(settings)?;
        self.conn.execute(
            "INSERT INTO persona_settings (persona_id, json) VALUES (?1, ?2)
             ON CONFLICT(persona_id) DO UPDATE SET json = excluded.json",
            rusqlite::params![settings.persona_id, json],
        )?;
        Ok(())
    }

    /// Delete the desktop-local settings for a persona. Returns `true` if a row
    /// was removed.
    pub fn delete_persona_settings(&self, persona_id: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM persona_settings WHERE persona_id = ?1",
            [persona_id],
        )?;
        Ok(affected > 0)
    }

    // --- efficacy_history (minimal accessors for later milestones) ----------

    /// Append an efficacy measurement row.
    pub fn insert_efficacy(&self, record: &EfficacyRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO efficacy_history (persona_id, recorded_at, metric, score)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                record.persona_id,
                record.recorded_at,
                record.metric,
                record.score
            ],
        )?;
        Ok(())
    }

    /// Read efficacy rows for a persona, oldest first.
    pub fn efficacy_for(&self, persona_id: &str) -> Result<Vec<EfficacyRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT persona_id, recorded_at, metric, score
             FROM efficacy_history WHERE persona_id = ?1 ORDER BY recorded_at ASC",
        )?;
        let rows = stmt.query_map([persona_id], |row| {
            Ok(EfficacyRecord {
                persona_id: row.get(0)?,
                recorded_at: row.get(1)?,
                metric: row.get(2)?,
                score: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // --- topics_measurements (Privacy Sandbox Topics read-back, C2 #12 R2) ---

    /// Append a Topics read-back measurement. The parsed
    /// [`AssignedTopic`](crate::browser::AssignedTopic) list is stored verbatim
    /// as JSON; an empty list (the common epoch-boundary case) is a valid record.
    pub fn insert_topics_measurement(&self, record: &TopicsMeasurement) -> Result<()> {
        let topics_json = serde_json::to_string(&record.topics)?;
        self.conn.execute(
            "INSERT INTO topics_measurements
                 (persona_id, decoy_id, recorded_at, available, topic_count, topics_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                record.persona_id,
                record.decoy_id,
                record.recorded_at,
                record.available as i64,
                record.topics.len() as i64,
                topics_json,
            ],
        )?;
        Ok(())
    }

    /// Read Topics measurements for a persona, oldest first.
    pub fn topics_for(&self, persona_id: &str) -> Result<Vec<TopicsMeasurement>> {
        let mut stmt = self.conn.prepare(
            "SELECT persona_id, decoy_id, recorded_at, available, topics_json
             FROM topics_measurements WHERE persona_id = ?1 ORDER BY recorded_at ASC",
        )?;
        let rows = stmt.query_map([persona_id], |row| {
            let topics_json: String = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)? != 0,
                topics_json,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (persona_id, decoy_id, recorded_at, available, topics_json) = row?;
            let topics: Vec<AssignedTopic> = serde_json::from_str(&topics_json)?;
            out.push(TopicsMeasurement {
                persona_id,
                decoy_id,
                recorded_at,
                available,
                topics,
            });
        }
        Ok(out)
    }

    /// The most recent Topics measurement for a persona, or `None` if none has
    /// been recorded yet.
    pub fn latest_topics_for(&self, persona_id: &str) -> Result<Option<TopicsMeasurement>> {
        let row = self
            .conn
            .query_row(
                "SELECT persona_id, decoy_id, recorded_at, available, topics_json
                 FROM topics_measurements WHERE persona_id = ?1
                 ORDER BY recorded_at DESC LIMIT 1",
                [persona_id],
                |row| {
                    let topics_json: String = row.get(4)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)? != 0,
                        topics_json,
                    ))
                },
            )
            .optional()?;
        match row {
            Some((persona_id, decoy_id, recorded_at, available, topics_json)) => {
                let topics: Vec<AssignedTopic> = serde_json::from_str(&topics_json)?;
                Ok(Some(TopicsMeasurement {
                    persona_id,
                    decoy_id,
                    recorded_at,
                    available,
                    topics,
                }))
            }
            None => Ok(None),
        }
    }

    // --- paired peers (cross-device sync, C1 #7) ----------------------------

    /// Insert or replace a paired-peer record, keyed on its public key. The
    /// record is persisted as the exact [`PairedPeer`] JSON.
    ///
    /// [`PairedPeer`]: crate::sync::PairedPeer
    pub fn save_paired_peer(&self, peer: &crate::sync::PairedPeer) -> Result<()> {
        let json = serde_json::to_string(peer)?;
        self.conn.execute(
            "INSERT INTO paired_peers (public_key, fingerprint, json, paired_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(public_key) DO UPDATE SET
                 fingerprint = excluded.fingerprint,
                 json = excluded.json,
                 paired_at = excluded.paired_at",
            rusqlite::params![peer.public_key, peer.fingerprint, json, peer.paired_at],
        )?;
        Ok(())
    }

    /// List all paired peers, most-recently paired first.
    pub fn list_paired_peers(&self) -> Result<Vec<crate::sync::PairedPeer>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM paired_peers ORDER BY paired_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Fetch a paired peer by its base64url public key, or `None` if absent.
    pub fn get_paired_peer(&self, public_key: &str) -> Result<Option<crate::sync::PairedPeer>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM paired_peers WHERE public_key = ?1",
                [public_key],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Delete a paired peer by its base64url public key. Returns `true` if a
    /// row was removed (revoking that peer's ability to sync).
    pub fn delete_paired_peer(&self, public_key: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM paired_peers WHERE public_key = ?1",
            [public_key],
        )?;
        Ok(affected > 0)
    }

    // --- orchestration: mode key/value (C1 #8) ------------------------------

    /// Read a scalar orchestration value by key, or `None` if unset.
    pub fn get_orchestration_value(&self, key: &str) -> Result<Option<String>> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM orchestration_kv WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Insert or replace a scalar orchestration value.
    pub fn put_orchestration_value(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO orchestration_kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    // --- orchestration: per-device persona assignment (C1 #8) ---------------

    /// Read the persona id assigned to `device_key`, or `None` if unassigned.
    /// The empty-string `device_key` is the reserved slot for this device.
    pub fn get_device_assignment(&self, device_key: &str) -> Result<Option<String>> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT persona_id FROM device_assignments WHERE device_key = ?1",
                [device_key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Insert or replace the persona assignment for `device_key`.
    pub fn put_device_assignment(
        &self,
        device_key: &str,
        persona_id: &str,
        updated_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO device_assignments (device_key, persona_id, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(device_key) DO UPDATE SET
                 persona_id = excluded.persona_id,
                 updated_at = excluded.updated_at",
            rusqlite::params![device_key, persona_id, updated_at],
        )?;
        Ok(())
    }

    /// List every `(device_key, persona_id)` assignment, device-key ascending
    /// for deterministic ordering.
    pub fn list_device_assignments(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT device_key, persona_id FROM device_assignments ORDER BY device_key ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Remove a device's persona assignment. Returns `true` if a row was removed.
    pub fn delete_device_assignment(&self, device_key: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM device_assignments WHERE device_key = ?1",
            [device_key],
        )?;
        Ok(affected > 0)
    }

    // --- orchestration: observed public IP per device (C1 #9) ---------------

    /// Read the last observed public IP for `device_key`. Returns `None` if the
    /// device has no row at all, and `Some(None)` if the row exists but the IP
    /// is recorded as unknown.
    pub fn get_device_ip(&self, device_key: &str) -> Result<Option<Option<String>>> {
        let row: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT ip FROM device_ip WHERE device_key = ?1",
                [device_key],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(row)
    }

    /// Insert or replace the observed public IP for `device_key`. A `None` ip
    /// records the device as present but with an unknown public IP.
    pub fn put_device_ip(
        &self,
        device_key: &str,
        ip: Option<&str>,
        observed_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO device_ip (device_key, ip, observed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(device_key) DO UPDATE SET
                 ip = excluded.ip,
                 observed_at = excluded.observed_at",
            rusqlite::params![device_key, ip, observed_at],
        )?;
        Ok(())
    }

    /// List every `(device_key, ip)` observation, device-key ascending. A `None`
    /// ip means the device was observed but its public IP is unknown.
    pub fn list_device_ips(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT device_key, ip FROM device_ip ORDER BY device_key ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // --- secrets (minimal accessors for later milestones) -------------------

    /// Insert or replace a named secret value.
    pub fn put_secret(&self, name: &str, value: &[u8]) -> Result<()> {
        let now = now_millis();
        self.conn.execute(
            "INSERT INTO secrets (name, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![name, value, now],
        )?;
        Ok(())
    }

    /// Read a named secret value, or `None` if absent.
    pub fn get_secret(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let value: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT value FROM secrets WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value)
    }

    // --- broker opt-out submissions (C3 #15 D1c) ----------------------------

    /// Insert or replace a broker opt-out submission, keyed on its id. The full
    /// [`BrokerSubmission`] JSON is stored verbatim; the scalar columns mirror
    /// it for index-friendly queries.
    pub fn upsert_broker_submission(&self, sub: &BrokerSubmission) -> Result<()> {
        let json = serde_json::to_string(sub)?;
        self.conn.execute(
            "INSERT INTO broker_submissions
                 (id, broker_id, persona_id, submitted_at, status, deadline, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 broker_id = excluded.broker_id,
                 persona_id = excluded.persona_id,
                 submitted_at = excluded.submitted_at,
                 status = excluded.status,
                 deadline = excluded.deadline,
                 json = excluded.json",
            rusqlite::params![
                sub.id,
                sub.broker_id,
                sub.persona_id,
                sub.submitted_at,
                sub.status.as_str(),
                sub.deadline,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a broker submission by id, or `None` if absent.
    pub fn get_broker_submission(&self, id: &str) -> Result<Option<BrokerSubmission>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM broker_submissions WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List broker submissions. When `persona_id` is `Some`, scoped to that
    /// persona; otherwise all. Newest submission first.
    pub fn list_broker_submissions(
        &self,
        persona_id: Option<&str>,
    ) -> Result<Vec<BrokerSubmission>> {
        let mut out = Vec::new();
        match persona_id {
            Some(pid) => {
                let mut stmt = self.conn.prepare(
                    "SELECT json FROM broker_submissions
                     WHERE persona_id = ?1 ORDER BY submitted_at DESC",
                )?;
                let rows = stmt.query_map([pid], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT json FROM broker_submissions ORDER BY submitted_at DESC")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
        }
        Ok(out)
    }

    /// Delete a broker submission by id. Returns `true` if a row was removed.
    pub fn delete_broker_submission(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM broker_submissions WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- broker scan snapshots (C4 #22 A3) ----------------------------------

    /// Insert or replace a broker scan snapshot, keyed on its id. The full
    /// [`BrokerScanSnapshot`] JSON (including the exposed-field set) is stored
    /// verbatim; the scalar `broker_id`/`persona_id`/`scanned_at` columns mirror
    /// it for the time-ordered per-`(broker, persona)` query.
    pub fn upsert_broker_scan_snapshot(&self, snapshot: &BrokerScanSnapshot) -> Result<()> {
        let json = serde_json::to_string(snapshot)?;
        self.conn.execute(
            "INSERT INTO broker_scan_snapshots
                 (id, broker_id, persona_id, scanned_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 broker_id = excluded.broker_id,
                 persona_id = excluded.persona_id,
                 scanned_at = excluded.scanned_at,
                 json = excluded.json",
            rusqlite::params![
                snapshot.id,
                snapshot.broker_id,
                snapshot.persona_id,
                snapshot.scanned_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a broker scan snapshot by id, or `None` if absent.
    pub fn get_broker_scan_snapshot(&self, id: &str) -> Result<Option<BrokerScanSnapshot>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM broker_scan_snapshots WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List broker scan snapshots for one `(broker, persona)`, OLDEST first
    /// (the order the diff timeline consumes). Empty when none recorded.
    pub fn list_broker_scan_snapshots(
        &self,
        broker_id: &str,
        persona_id: &str,
    ) -> Result<Vec<BrokerScanSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM broker_scan_snapshots
             WHERE broker_id = ?1 AND persona_id = ?2
             ORDER BY scanned_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([broker_id, persona_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// List broker scan snapshots scoped to one persona across ALL brokers
    /// (e.g. to enumerate which brokers have scans). Oldest first per the same
    /// ordering. Empty when none recorded.
    pub fn list_broker_scan_snapshots_for_persona(
        &self,
        persona_id: &str,
    ) -> Result<Vec<BrokerScanSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM broker_scan_snapshots
             WHERE persona_id = ?1
             ORDER BY broker_id ASC, scanned_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([persona_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Delete a broker scan snapshot by id. Returns `true` if a row was removed.
    pub fn delete_broker_scan_snapshot(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM broker_scan_snapshots WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- per-site GPC honoring status (C3 #18 D4c) --------------------------

    /// Insert or replace the GPC-honoring observation for a site origin. The
    /// full [`GpcSupport`] JSON is stored verbatim; `honored`/`checked_at` are
    /// mirrored for fast filtering. A re-check upserts the latest observation.
    pub fn upsert_gpc_status(&self, status: &GpcSiteStatus) -> Result<()> {
        let json = serde_json::to_string(&status.support)?;
        self.conn.execute(
            "INSERT INTO gpc_site_status (origin, honored, checked_at, json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(origin) DO UPDATE SET
                 honored = excluded.honored,
                 checked_at = excluded.checked_at,
                 json = excluded.json",
            rusqlite::params![
                status.origin,
                status.support.honored as i64,
                status.checked_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Read the GPC-honoring observation for a site origin, or `None` if the
    /// site has never been checked.
    pub fn gpc_status_for(&self, origin: &str) -> Result<Option<GpcSiteStatus>> {
        let row: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT checked_at, json FROM gpc_site_status WHERE origin = ?1",
                [origin],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        match row {
            Some((checked_at, json)) => {
                let support: GpcSupport = serde_json::from_str(&json)?;
                Ok(Some(GpcSiteStatus {
                    origin: origin.to_string(),
                    checked_at,
                    support,
                }))
            }
            None => Ok(None),
        }
    }

    /// List every per-site GPC observation, origin ascending for deterministic
    /// ordering.
    pub fn list_gpc_status(&self) -> Result<Vec<GpcSiteStatus>> {
        let mut stmt = self
            .conn
            .prepare("SELECT origin, checked_at, json FROM gpc_site_status ORDER BY origin ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (origin, checked_at, json) = row?;
            let support: GpcSupport = serde_json::from_str(&json)?;
            out.push(GpcSiteStatus {
                origin,
                checked_at,
                support,
            });
        }
        Ok(out)
    }

    // --- DSAR requests (C3 #16 D2c) -----------------------------------------

    /// Insert or replace a DSAR request, keyed on its id. The full
    /// [`DsarRequest`] JSON is stored verbatim; scalar columns mirror it for
    /// index-friendly queries (per persona, by status, due deadlines).
    pub fn upsert_dsar_request(&self, req: &DsarRequest) -> Result<()> {
        let json = serde_json::to_string(req)?;
        self.conn.execute(
            "INSERT INTO dsar_requests
                 (id, kind, persona_id, status, created_at, deadline, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 kind = excluded.kind,
                 persona_id = excluded.persona_id,
                 status = excluded.status,
                 created_at = excluded.created_at,
                 deadline = excluded.deadline,
                 json = excluded.json",
            rusqlite::params![
                req.id,
                req.kind.as_str(),
                req.persona_id,
                req.status.as_str(),
                req.created_at,
                req.deadline,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a DSAR request by id, or `None` if absent.
    pub fn get_dsar_request(&self, id: &str) -> Result<Option<DsarRequest>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM dsar_requests WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List DSAR requests. When `persona_id` is `Some`, scoped to that persona;
    /// otherwise all. Newest-created first.
    pub fn list_dsar_requests(&self, persona_id: Option<&str>) -> Result<Vec<DsarRequest>> {
        let mut out = Vec::new();
        match persona_id {
            Some(pid) => {
                let mut stmt = self.conn.prepare(
                    "SELECT json FROM dsar_requests
                     WHERE persona_id = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([pid], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT json FROM dsar_requests ORDER BY created_at DESC")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
        }
        Ok(out)
    }

    /// Delete a DSAR request by id. Returns `true` if a row was removed.
    pub fn delete_dsar_request(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM dsar_requests WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- email aliases (C3 #17 D3c) -----------------------------------------

    /// Insert or replace an email alias, keyed on its id. The full
    /// [`EmailAlias`] JSON is stored verbatim; scalar columns mirror it for the
    /// inventory queries and the no-reuse-across-sites check. PROVIDER secrets
    /// are NOT stored here; only the alias->persona->site mapping is.
    pub fn upsert_email_alias(&self, alias: &EmailAlias) -> Result<()> {
        let json = serde_json::to_string(alias)?;
        self.conn.execute(
            "INSERT INTO email_aliases
                 (id, persona_id, site, address, kind, status, created_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 persona_id = excluded.persona_id,
                 site = excluded.site,
                 address = excluded.address,
                 kind = excluded.kind,
                 status = excluded.status,
                 created_at = excluded.created_at,
                 json = excluded.json",
            rusqlite::params![
                alias.id,
                alias.persona_id,
                alias.site,
                alias.address,
                alias.kind.as_str(),
                alias.status.as_str(),
                alias.created_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch an email alias by id, or `None` if absent.
    pub fn get_email_alias(&self, id: &str) -> Result<Option<EmailAlias>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM email_aliases WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List email aliases. When `persona_id` is `Some`, scoped to that persona;
    /// otherwise all. Newest-created first.
    pub fn list_email_aliases(&self, persona_id: Option<&str>) -> Result<Vec<EmailAlias>> {
        let mut out = Vec::new();
        match persona_id {
            Some(pid) => {
                let mut stmt = self.conn.prepare(
                    "SELECT json FROM email_aliases
                     WHERE persona_id = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([pid], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT json FROM email_aliases ORDER BY created_at DESC")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
        }
        Ok(out)
    }

    /// The ACTIVE email aliases for `(persona_id, site)`, newest first. Used to
    /// enforce the no-reuse-across-sites rule and to find the alias to rotate.
    pub fn active_aliases_for_site(&self, persona_id: &str, site: &str) -> Result<Vec<EmailAlias>> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM email_aliases
             WHERE persona_id = ?1 AND site = ?2 AND status = ?3
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![persona_id, site, AliasStatus::Active.as_str()],
            |row| row.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// The ACTIVE alias records that already use `address` for this persona,
    /// regardless of site. Used to flag/enforce reusing one address across two
    /// sites. Newest first.
    pub fn active_aliases_with_address(
        &self,
        persona_id: &str,
        address: &str,
    ) -> Result<Vec<EmailAlias>> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM email_aliases
             WHERE persona_id = ?1 AND address = ?2 AND status = ?3
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![persona_id, address, AliasStatus::Active.as_str()],
            |row| row.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Delete an email alias by id. Returns `true` if a row was removed. (Revoke
    /// is normally an upsert to `revoked` status, preserving the audit trail;
    /// this hard-deletes.)
    pub fn delete_email_alias(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM email_aliases WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- account anchors (C3 #19 D5c) ---------------------------------------

    /// Insert or replace an account anchor, keyed on its id. The full
    /// [`AccountAnchor`] JSON is stored verbatim. This is a user-curated,
    /// read-only analysis inventory; no credential is ever stored.
    pub fn upsert_account_anchor(&self, anchor: &AccountAnchor) -> Result<()> {
        let json = serde_json::to_string(anchor)?;
        self.conn.execute(
            "INSERT INTO account_anchors (id, label, site, created_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 label = excluded.label,
                 site = excluded.site,
                 created_at = excluded.created_at,
                 json = excluded.json",
            rusqlite::params![
                anchor.id,
                anchor.label,
                anchor.site,
                anchor.created_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch an account anchor by id, or `None` if absent.
    pub fn get_account_anchor(&self, id: &str) -> Result<Option<AccountAnchor>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM account_anchors WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List every account anchor in the inventory, label ascending for
    /// deterministic ordering (scoring/recommendation re-orders by score).
    pub fn list_account_anchors(&self) -> Result<Vec<AccountAnchor>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM account_anchors ORDER BY label ASC, id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Delete an account anchor by id. Returns `true` if a row was removed.
    pub fn delete_account_anchor(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM account_anchors WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- shadow profiles (C4 #21 A2) ----------------------------------------

    /// Insert or replace a shadow-profile definition, keyed on its id. The full
    /// [`ShadowProfile`] JSON is stored verbatim; the scalar `arm`/`persona_id`
    /// columns mirror it for the per-arm cohort queries.
    pub fn upsert_shadow_profile(&self, profile: &ShadowProfile) -> Result<()> {
        let json = serde_json::to_string(profile)?;
        self.conn.execute(
            "INSERT INTO shadow_profiles (id, label, arm, persona_id, created_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 label = excluded.label,
                 arm = excluded.arm,
                 persona_id = excluded.persona_id,
                 created_at = excluded.created_at,
                 json = excluded.json",
            rusqlite::params![
                profile.id,
                profile.label,
                profile.arm.as_str(),
                profile.persona_id,
                profile.created_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a shadow-profile definition by id, or `None` if absent.
    pub fn get_shadow_profile(&self, id: &str) -> Result<Option<ShadowProfile>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM shadow_profiles WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List every shadow-profile definition, newest-defined first.
    pub fn list_shadow_profiles(&self) -> Result<Vec<ShadowProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM shadow_profiles ORDER BY created_at DESC, id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Delete a shadow-profile definition by id. Returns `true` if a row was
    /// removed.
    pub fn delete_shadow_profile(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM shadow_profiles WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- installed persona packs (C5 #27 P4) --------------------------------

    /// Insert or replace an installed-pack ledger row, keyed on its id. The full
    /// [`PackRecord`] JSON (provenance, signer key, persona ids) is stored
    /// verbatim; the scalar columns mirror it for the library list/filter
    /// queries. The personas themselves live in the `personas` table.
    pub fn upsert_installed_pack(&self, pack: &InstalledPack) -> Result<()> {
        let record = &pack.record;
        let json = serde_json::to_string(record)?;
        self.conn.execute(
            "INSERT INTO installed_packs
                 (id, source_label, signer_public_key, schema_version,
                  persona_count, imported_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 source_label = excluded.source_label,
                 signer_public_key = excluded.signer_public_key,
                 schema_version = excluded.schema_version,
                 persona_count = excluded.persona_count,
                 imported_at = excluded.imported_at,
                 json = excluded.json",
            rusqlite::params![
                record.id,
                record.provenance.source_distribution,
                record.signer_public_key,
                record.schema_version as i64,
                record.persona_count() as i64,
                record.imported_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch an installed-pack ledger row by id, or `None` if absent.
    pub fn get_installed_pack(&self, id: &str) -> Result<Option<InstalledPack>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM installed_packs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(InstalledPack::new(serde_json::from_str(&j)?))),
            None => Ok(None),
        }
    }

    /// List every installed-pack ledger row, most-recently imported first.
    pub fn list_installed_packs(&self) -> Result<Vec<InstalledPack>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM installed_packs ORDER BY imported_at DESC, id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(InstalledPack::new(serde_json::from_str(&row?)?));
        }
        Ok(out)
    }

    /// Delete an installed-pack ledger row by id. Returns `true` if a row was
    /// removed. This removes only the library ledger entry; whether the personas
    /// it brought in are also removed is the caller's policy decision.
    pub fn delete_installed_pack(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM installed_packs WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // --- per-persona network egress (C7 #30 N1) -----------------------------

    /// Insert or replace the [`Egress`] binding for a persona, keyed on its
    /// persona id. The full Egress JSON is stored verbatim. CREDENTIALS are NOT
    /// stored here: the JSON carries only a non-secret keystore account label;
    /// the secret username/password live in the OS keystore.
    pub fn put_persona_egress(&self, persona_id: &str, egress: &Egress) -> Result<()> {
        let json = serde_json::to_string(egress)?;
        self.conn.execute(
            "INSERT INTO persona_egress (persona_id, json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(persona_id) DO UPDATE SET
                 json = excluded.json,
                 updated_at = excluded.updated_at",
            rusqlite::params![persona_id, json, now_millis()],
        )?;
        Ok(())
    }

    /// Read the [`Egress`] binding for a persona, or `None` if none is set (the
    /// caller treats that as [`Egress::Direct`]).
    pub fn get_persona_egress(&self, persona_id: &str) -> Result<Option<Egress>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM persona_egress WHERE persona_id = ?1",
                [persona_id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Delete a persona's egress binding. Returns `true` if a row was removed.
    pub fn delete_persona_egress(&self, persona_id: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM persona_egress WHERE persona_id = ?1",
            [persona_id],
        )?;
        Ok(affected > 0)
    }

    // --- per-persona DNS strategy (C7 #31 N2) -------------------------------

    /// Insert or replace the [`DnsStrategy`] for a persona, keyed on its persona
    /// id. The full DnsStrategy JSON is stored verbatim. DNS choices are
    /// persisted, never logged as sensitive.
    pub fn put_persona_dns(&self, persona_id: &str, dns: &DnsStrategy) -> Result<()> {
        let json = serde_json::to_string(dns)?;
        self.conn.execute(
            "INSERT INTO persona_dns (persona_id, json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(persona_id) DO UPDATE SET
                 json = excluded.json,
                 updated_at = excluded.updated_at",
            rusqlite::params![persona_id, json, now_millis()],
        )?;
        Ok(())
    }

    /// Read the [`DnsStrategy`] for a persona, or `None` if none is set (the
    /// caller treats that as [`DnsStrategy::SystemDefault`]).
    pub fn get_persona_dns(&self, persona_id: &str) -> Result<Option<DnsStrategy>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM persona_dns WHERE persona_id = ?1",
                [persona_id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Delete a persona's DNS-strategy binding. Returns `true` if a row was
    /// removed.
    pub fn delete_persona_dns(&self, persona_id: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "DELETE FROM persona_dns WHERE persona_id = ?1",
            [persona_id],
        )?;
        Ok(affected > 0)
    }

    // --- goal-driven campaigns (C8 #33 U2) ----------------------------------

    /// Insert or replace a campaign, keyed on its id. The full [`Campaign`] JSON
    /// (goal + target segment + lifecycle + closed-loop progress) is stored
    /// verbatim; the scalar `persona_id`/`status`/`updated_at` columns mirror it
    /// for the per-persona and by-status queries. Persisting the progress is what
    /// lets a campaign survive restart with its dwell clock intact.
    pub fn upsert_campaign(&self, campaign: &Campaign) -> Result<()> {
        let json = serde_json::to_string(campaign)?;
        self.conn.execute(
            "INSERT INTO campaigns
                 (id, persona_id, status, created_at, updated_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 persona_id = excluded.persona_id,
                 status = excluded.status,
                 created_at = excluded.created_at,
                 updated_at = excluded.updated_at,
                 json = excluded.json",
            rusqlite::params![
                campaign.id,
                campaign.persona_id,
                campaign.status.as_str(),
                campaign.created_at,
                campaign.updated_at,
                json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a campaign by id, or `None` if absent.
    pub fn get_campaign(&self, id: &str) -> Result<Option<Campaign>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM campaigns WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// List campaigns. Scoped to `persona_id` when `Some`; else all. Most
    /// recently updated first.
    pub fn list_campaigns(&self, persona_id: Option<&str>) -> Result<Vec<Campaign>> {
        let mut out = Vec::new();
        match persona_id {
            Some(pid) => {
                let mut stmt = self.conn.prepare(
                    "SELECT json FROM campaigns
                     WHERE persona_id = ?1 ORDER BY updated_at DESC",
                )?;
                let rows = stmt.query_map([pid], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT json FROM campaigns ORDER BY updated_at DESC")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    out.push(serde_json::from_str(&row?)?);
                }
            }
        }
        Ok(out)
    }

    /// Delete a campaign by id. Returns `true` if a row was removed.
    pub fn delete_campaign(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM campaigns WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }
}

/// Current wall-clock time in epoch milliseconds (0 if the clock predates the
/// epoch, which cannot happen on a sane host).
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
    use tempfile::tempdir;

    fn passphrase_source(dir: &Path) -> KeySource {
        KeySource::EncryptedFile {
            path: dir.join("key.bin"),
            passphrase: "test-passphrase".to_string(),
        }
    }

    fn sample_persona() -> SyntheticPersona {
        SyntheticPersona::new(
            "33333333-3333-4333-8333-333333333333".to_string(),
            "Round Trip".to_string(),
            AgeRange::AGE_25_34.as_name().to_string(),
            Profession::ENGINEER.as_name().to_string(),
            Region::US_WEST.as_name().to_string(),
            vec![
                CategoryPool::TECHNOLOGY.as_name().to_string(),
                CategoryPool::GAMING.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
                CategoryPool::MUSIC.as_name().to_string(),
            ],
            1_700_000_000_123,
            1_700_600_000_456,
        )
    }

    #[test]
    fn open_write_read_back_equal() -> Result<()> {
        let dir = tempdir()?;
        let db = dir.path().join("fauxx.db");
        let src = passphrase_source(dir.path());

        let store = EncryptedStore::open_at(&db, &src)?;
        let persona = sample_persona();
        store.save_persona(&persona)?;

        let fetched = store
            .get_persona(&persona.id)?
            .ok_or_else(|| CoreError::Key("persona missing after save".into()))?;
        assert_eq!(fetched, persona);

        let all = store.list_personas()?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], persona);
        Ok(())
    }

    #[test]
    fn reopen_with_same_key_reads_data() -> Result<()> {
        let dir = tempdir()?;
        let db = dir.path().join("fauxx.db");
        let src = passphrase_source(dir.path());
        let persona = sample_persona();

        {
            let store = EncryptedStore::open_at(&db, &src)?;
            store.save_persona(&persona)?;
        } // drop closes the connection

        let reopened = EncryptedStore::open_at(&db, &src)?;
        let fetched = reopened
            .get_persona(&persona.id)?
            .ok_or_else(|| CoreError::Key("persona missing after reopen".into()))?;
        assert_eq!(fetched, persona);
        Ok(())
    }

    #[test]
    fn wrong_passphrase_fails_closed_on_reopen() -> Result<()> {
        let dir = tempdir()?;
        let db = dir.path().join("fauxx.db");
        let key_path = dir.path().join("key.bin");

        // Create + populate with the right passphrase.
        let right = KeySource::EncryptedFile {
            path: key_path.clone(),
            passphrase: "right".to_string(),
        };
        {
            let store = EncryptedStore::open_at(&db, &right)?;
            store.save_persona(&sample_persona())?;
        }

        // A different key file + passphrase yields a different key, which must
        // not decrypt the existing database.
        let wrong = KeySource::EncryptedFile {
            path: dir.path().join("other-key.bin"),
            passphrase: "wrong".to_string(),
        };
        let result = EncryptedStore::open_at(&db, &wrong);
        assert!(
            matches!(result, Err(CoreError::Keystore(_))),
            "expected fail-closed Keystore error, got {result:?}"
        );
        Ok(())
    }

    #[test]
    fn persona_round_trips_all_fields() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;
        let mut persona = sample_persona();
        persona.note = Some("desktop-only note".to_string());
        persona.home_location = Some("Seattle, WA".to_string());
        persona.schedule = Some("night_owl".to_string());
        persona.browsing_style = Some("skimmer".to_string());
        store.save_persona(&persona)?;
        let back = store
            .get_persona(&persona.id)?
            .ok_or_else(|| CoreError::Key("missing".into()))?;
        assert_eq!(back.id, persona.id);
        assert_eq!(back.name, persona.name);
        assert_eq!(back.age_range, persona.age_range);
        assert_eq!(back.profession, persona.profession);
        assert_eq!(back.region, persona.region);
        assert_eq!(back.interests, persona.interests);
        assert_eq!(back.created_at, persona.created_at);
        assert_eq!(back.active_until, persona.active_until);
        assert_eq!(back.schema_version, persona.schema_version);
        assert_eq!(back.note, persona.note);
        assert_eq!(back.home_location, persona.home_location);
        assert_eq!(back.schedule, persona.schedule);
        assert_eq!(back.browsing_style, persona.browsing_style);
        assert_eq!(back, persona);
        Ok(())
    }

    #[test]
    fn update_and_delete_persona() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;
        let mut persona = sample_persona();
        store.save_persona(&persona)?;

        persona.name = "Renamed".to_string();
        store.save_persona(&persona)?; // upsert
        let back = store
            .get_persona(&persona.id)?
            .ok_or_else(|| CoreError::Key("missing".into()))?;
        assert_eq!(back.name, "Renamed");
        assert_eq!(store.list_personas()?.len(), 1);

        assert!(store.delete_persona(&persona.id)?);
        assert!(!store.delete_persona(&persona.id)?); // already gone
        assert!(store.get_persona(&persona.id)?.is_none());
        Ok(())
    }

    #[test]
    fn persona_settings_persist_and_round_trip() -> Result<()> {
        use crate::studio::{PersonaField, PersonaSettings, RotationSchedule};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        // Absent until saved.
        assert!(store.get_persona_settings("p1")?.is_none());

        let mut settings = PersonaSettings::default_for("p1");
        settings.lock(PersonaField::AgeRange);
        settings.lock(PersonaField::Region);
        settings.set_rotation(RotationSchedule::Disabled);
        store.save_persona_settings(&settings)?;

        let back = store
            .get_persona_settings("p1")?
            .ok_or_else(|| CoreError::Key("settings missing".into()))?;
        assert_eq!(back, settings);
        assert!(back.is_locked(PersonaField::AgeRange));
        assert!(!back.rotation.is_enabled());

        // Upsert replaces.
        settings.unlock(PersonaField::AgeRange);
        settings.set_rotation(RotationSchedule::frozen_cadence());
        store.save_persona_settings(&settings)?;
        let updated = store
            .get_persona_settings("p1")?
            .ok_or_else(|| CoreError::Key("settings missing".into()))?;
        assert!(!updated.is_locked(PersonaField::AgeRange));
        assert!(updated.rotation.is_enabled());

        // Delete.
        assert!(store.delete_persona_settings("p1")?);
        assert!(!store.delete_persona_settings("p1")?);
        assert!(store.get_persona_settings("p1")?.is_none());
        Ok(())
    }

    #[test]
    fn efficacy_and_secrets_accessors_work() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        store.insert_efficacy(&EfficacyRecord {
            persona_id: "p1".to_string(),
            recorded_at: 1000,
            metric: "ctr".to_string(),
            score: 0.42,
        })?;
        store.insert_efficacy(&EfficacyRecord {
            persona_id: "p1".to_string(),
            recorded_at: 2000,
            metric: "ctr".to_string(),
            score: 0.50,
        })?;
        let rows = store.efficacy_for("p1")?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].recorded_at, 1000);
        assert_eq!(rows[1].score, 0.50);
        assert!(store.efficacy_for("absent")?.is_empty());

        assert!(store.get_secret("token")?.is_none());
        store.put_secret("token", b"s3cr3t")?;
        assert_eq!(store.get_secret("token")?.as_deref(), Some(&b"s3cr3t"[..]));
        store.put_secret("token", b"rotated")?; // upsert
        assert_eq!(store.get_secret("token")?.as_deref(), Some(&b"rotated"[..]));
        Ok(())
    }

    #[test]
    fn topics_measurements_persist_and_read_back() -> Result<()> {
        use crate::browser::AssignedTopic;
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        // No measurements yet.
        assert!(store.topics_for("p1")?.is_empty());
        assert!(store.latest_topics_for("p1")?.is_none());

        // A non-empty read (topics were assigned).
        let with_topics = TopicsMeasurement {
            persona_id: "p1".to_string(),
            decoy_id: "decoy-p1".to_string(),
            recorded_at: 1000,
            available: true,
            topics: vec![
                AssignedTopic {
                    topic_id: 57,
                    taxonomy_version: Some("1".to_string()),
                    model_version: Some("2206021246".to_string()),
                    version: Some("chrome.1".to_string()),
                    name: Some("/Arts & Entertainment".to_string()),
                },
                AssignedTopic {
                    topic_id: 104,
                    taxonomy_version: Some("1".to_string()),
                    model_version: None,
                    version: None,
                    name: None,
                },
            ],
        };
        store.insert_topics_measurement(&with_topics)?;

        // The expected epoch-boundary read: available but EMPTY topics.
        let empty_epoch = TopicsMeasurement {
            persona_id: "p1".to_string(),
            decoy_id: "decoy-p1".to_string(),
            recorded_at: 2000,
            available: true,
            topics: Vec::new(),
        };
        store.insert_topics_measurement(&empty_epoch)?;

        let all = store.topics_for("p1")?;
        assert_eq!(all.len(), 2);
        // Oldest first; the full AssignedTopic list round-trips byte-faithfully.
        assert_eq!(all[0], with_topics);
        assert_eq!(all[0].topics.len(), 2);
        assert_eq!(all[0].topics[0].topic_id, 57);
        // The empty epoch-boundary read persisted as a valid record.
        assert_eq!(all[1], empty_epoch);
        assert!(all[1].topics.is_empty());
        assert!(all[1].available);

        // Latest is the most recently recorded.
        let latest = store
            .latest_topics_for("p1")?
            .ok_or_else(|| CoreError::Key("latest missing".into()))?;
        assert_eq!(latest, empty_epoch);

        // A different persona is isolated.
        assert!(store.topics_for("other")?.is_empty());
        Ok(())
    }

    #[test]
    fn migration_brings_schema_to_current_version() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn orchestration_kv_and_assignments_round_trip() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.get_orchestration_value("mode")?.is_none());
        store.put_orchestration_value("mode", "CoherentHousehold")?;
        assert_eq!(
            store.get_orchestration_value("mode")?.as_deref(),
            Some("CoherentHousehold")
        );
        store.put_orchestration_value("mode", "Fragmentation")?; // upsert
        assert_eq!(
            store.get_orchestration_value("mode")?.as_deref(),
            Some("Fragmentation")
        );

        assert!(store.get_device_assignment("")?.is_none());
        store.put_device_assignment("", "persona-self", 10)?;
        store.put_device_assignment("peer-key", "persona-peer", 11)?;
        assert_eq!(
            store.get_device_assignment("")?.as_deref(),
            Some("persona-self")
        );
        let all = store.list_device_assignments()?;
        assert_eq!(all.len(), 2);
        // Empty string sorts first.
        assert_eq!(all[0], (String::new(), "persona-self".to_string()));
        assert!(store.delete_device_assignment("peer-key")?);
        assert!(!store.delete_device_assignment("peer-key")?);
        assert_eq!(store.list_device_assignments()?.len(), 1);
        Ok(())
    }

    #[test]
    fn broker_submissions_round_trip_and_scope_by_persona() -> Result<()> {
        use crate::brokers::registry::broker;
        use crate::brokers::{BrokerSubmission, SubmissionStatus};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        // Nothing recorded yet.
        assert!(store.list_broker_submissions(None)?.is_empty());
        assert!(store.get_broker_submission("missing")?.is_none());

        let spokeo = broker("spokeo")?;
        let mut s1 = BrokerSubmission::draft("s1".to_string(), "spokeo", "p1", spokeo, 1_000);
        let s2 = BrokerSubmission::draft("s2".to_string(), "spokeo", "p2", spokeo, 2_000);
        store.upsert_broker_submission(&s1)?;
        store.upsert_broker_submission(&s2)?;

        // Round-trips byte-faithfully.
        let back = store
            .get_broker_submission("s1")?
            .ok_or_else(|| CoreError::Key("s1 missing".into()))?;
        assert_eq!(back, s1);

        // Scoped to a persona.
        let p1 = store.list_broker_submissions(Some("p1"))?;
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].id, "s1");
        assert_eq!(store.list_broker_submissions(None)?.len(), 2);

        // Upsert updates status in both JSON and the scalar column.
        s1.status = SubmissionStatus::Removed;
        s1.confirmation_token = Some("tok-123".to_string());
        store.upsert_broker_submission(&s1)?;
        let updated = store
            .get_broker_submission("s1")?
            .ok_or_else(|| CoreError::Key("s1 missing".into()))?;
        assert_eq!(updated.status, SubmissionStatus::Removed);
        assert_eq!(updated.confirmation_token.as_deref(), Some("tok-123"));

        // Delete.
        assert!(store.delete_broker_submission("s1")?);
        assert!(!store.delete_broker_submission("s1")?);
        assert_eq!(store.list_broker_submissions(None)?.len(), 1);
        Ok(())
    }

    #[test]
    fn broker_scan_snapshots_round_trip_and_order_oldest_first() -> Result<()> {
        use crate::brokers::BrokerScanSnapshot;
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        // Nothing recorded yet.
        assert!(store.list_broker_scan_snapshots("spokeo", "p1")?.is_empty());
        assert!(store.get_broker_scan_snapshot("missing")?.is_none());

        // Two snapshots for (spokeo, p1), recorded out of time order.
        let later = BrokerScanSnapshot::new(
            "snap-2",
            "spokeo",
            "p1",
            2_000,
            ["name".to_string(), "email".to_string()],
        );
        let earlier = BrokerScanSnapshot::new(
            "snap-1",
            "spokeo",
            "p1",
            1_000,
            ["name".to_string(), "phone".to_string()],
        );
        // A snapshot for a different persona must stay isolated.
        let other = BrokerScanSnapshot::new("snap-x", "spokeo", "p2", 1_500, ["name".to_string()]);
        store.upsert_broker_scan_snapshot(&later)?;
        store.upsert_broker_scan_snapshot(&earlier)?;
        store.upsert_broker_scan_snapshot(&other)?;

        // Round-trips byte-faithfully.
        let back = store
            .get_broker_scan_snapshot("snap-1")?
            .ok_or_else(|| CoreError::Key("snap-1 missing".into()))?;
        assert_eq!(back, earlier);

        // Listed oldest first and scoped to (broker, persona).
        let scoped = store.list_broker_scan_snapshots("spokeo", "p1")?;
        assert_eq!(scoped.len(), 2);
        assert_eq!(scoped[0].id, "snap-1");
        assert_eq!(scoped[1].id, "snap-2");

        // The other persona is isolated.
        let p2 = store.list_broker_scan_snapshots("spokeo", "p2")?;
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].id, "snap-x");

        // Per-persona-across-brokers listing.
        assert_eq!(store.list_broker_scan_snapshots_for_persona("p1")?.len(), 2);

        // Delete.
        assert!(store.delete_broker_scan_snapshot("snap-1")?);
        assert!(!store.delete_broker_scan_snapshot("snap-1")?);
        assert_eq!(store.list_broker_scan_snapshots("spokeo", "p1")?.len(), 1);
        Ok(())
    }

    #[test]
    fn gpc_site_status_round_trips_and_upserts() -> Result<()> {
        use crate::browser::GpcSupport;
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.gpc_status_for("https://example.com")?.is_none());
        assert!(store.list_gpc_status()?.is_empty());

        let honored = GpcSiteStatus {
            origin: "https://example.com".to_string(),
            checked_at: 1_000,
            support: GpcSupport {
                honored: true,
                last_update: Some("2022-06-01".to_string()),
                version: None,
            },
        };
        store.upsert_gpc_status(&honored)?;
        let back = store
            .gpc_status_for("https://example.com")?
            .ok_or_else(|| CoreError::Key("origin missing".into()))?;
        assert_eq!(back, honored);
        assert!(back.support.honored);

        // Re-check upserts the latest observation for the same origin.
        let not_honored = GpcSiteStatus {
            origin: "https://example.com".to_string(),
            checked_at: 2_000,
            support: GpcSupport::not_advertised(),
        };
        store.upsert_gpc_status(&not_honored)?;
        let back = store
            .gpc_status_for("https://example.com")?
            .ok_or_else(|| CoreError::Key("origin missing".into()))?;
        assert!(!back.support.honored);
        assert_eq!(back.checked_at, 2_000);
        // Still one row (upsert, not insert).
        assert_eq!(store.list_gpc_status()?.len(), 1);
        Ok(())
    }

    #[test]
    fn dsar_requests_round_trip_and_scope_by_persona() -> Result<()> {
        use crate::dsar::{Controller, DsarRequest, RequestKind, RequestStatus};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.list_dsar_requests(None)?.is_empty());
        assert!(store.get_dsar_request("missing")?.is_none());

        let mut r1 = DsarRequest::draft(
            "r1".to_string(),
            RequestKind::GdprAccess,
            "p1",
            Controller::arbitrary("Example Corp", "privacy@example.test"),
            1_000,
        );
        let r2 = DsarRequest::draft(
            "r2".to_string(),
            RequestKind::CcpaDeletion,
            "p2",
            Controller::resolve_broker("spokeo")?,
            2_000,
        );
        store.upsert_dsar_request(&r1)?;
        store.upsert_dsar_request(&r2)?;

        // Round-trips byte-faithfully.
        let back = store
            .get_dsar_request("r1")?
            .ok_or_else(|| CoreError::Key("r1 missing".into()))?;
        assert_eq!(back, r1);

        // Scoped to a persona.
        let p1 = store.list_dsar_requests(Some("p1"))?;
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].id, "r1");
        assert_eq!(store.list_dsar_requests(None)?.len(), 2);

        // Upsert updates the status + deadline in both JSON and scalar columns.
        r1.mark_sent(5_000)?;
        assert_eq!(r1.status, RequestStatus::Sent);
        store.upsert_dsar_request(&r1)?;
        let updated = store
            .get_dsar_request("r1")?
            .ok_or_else(|| CoreError::Key("r1 missing".into()))?;
        assert_eq!(updated.status, RequestStatus::Sent);
        assert!(updated.deadline.is_some());
        // The scalar deadline column mirrors the JSON.
        let scalar_deadline: Option<i64> = store.conn.query_row(
            "SELECT deadline FROM dsar_requests WHERE id = 'r1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(scalar_deadline, updated.deadline);

        assert!(store.delete_dsar_request("r1")?);
        assert!(!store.delete_dsar_request("r1")?);
        assert_eq!(store.list_dsar_requests(None)?.len(), 1);
        Ok(())
    }

    #[test]
    fn email_aliases_round_trip_and_query_by_site_and_address() -> Result<()> {
        use crate::aliases::{AliasKind, AliasStatus, EmailAlias};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.list_email_aliases(None)?.is_empty());

        let a1 = EmailAlias::new(
            "a1".to_string(),
            "p1",
            "spokeo.com",
            "alice+spokeo.com@example.com",
            AliasKind::PlusAddress,
            10,
            Some("plus".to_string()),
        );
        let a2 = EmailAlias::new(
            "a2".to_string(),
            "p1",
            "whitepages.com",
            "alice+whitepages.com@example.com",
            AliasKind::PlusAddress,
            20,
            Some("plus".to_string()),
        );
        store.upsert_email_alias(&a1)?;
        store.upsert_email_alias(&a2)?;

        let back = store
            .get_email_alias("a1")?
            .ok_or_else(|| CoreError::Key("a1 missing".into()))?;
        assert_eq!(back, a1);

        // Scoped + all.
        assert_eq!(store.list_email_aliases(Some("p1"))?.len(), 2);
        assert_eq!(store.list_email_aliases(None)?.len(), 2);

        // Active aliases for a site.
        let for_site = store.active_aliases_for_site("p1", "spokeo.com")?;
        assert_eq!(for_site.len(), 1);
        assert_eq!(for_site[0].id, "a1");

        // Active aliases with an address.
        let with_addr = store.active_aliases_with_address("p1", "alice+spokeo.com@example.com")?;
        assert_eq!(with_addr.len(), 1);

        // Revoke (upsert to revoked) drops it out of the active-site query.
        let mut revoked = a1.clone();
        revoked.status = AliasStatus::Revoked;
        store.upsert_email_alias(&revoked)?;
        assert!(store
            .active_aliases_for_site("p1", "spokeo.com")?
            .is_empty());
        // But it is still in the full inventory (audit trail).
        assert_eq!(store.list_email_aliases(None)?.len(), 2);

        assert!(store.delete_email_alias("a2")?);
        assert!(!store.delete_email_alias("a2")?);
        Ok(())
    }

    #[test]
    fn account_anchors_round_trip() -> Result<()> {
        use crate::anchors::{AccountAnchor, IdentitySignal};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.list_account_anchors()?.is_empty());

        let anchor = AccountAnchor::new(
            "an1".to_string(),
            "Personal Email",
            "google.com",
            [
                IdentitySignal::VerifiedEmail,
                IdentitySignal::RecoveryContact,
                IdentitySignal::LegalName,
            ],
            Some("recovery-key-1".to_string()),
            100,
        );
        store.upsert_account_anchor(&anchor)?;

        let back = store
            .get_account_anchor("an1")?
            .ok_or_else(|| CoreError::Key("an1 missing".into()))?;
        assert_eq!(back, anchor);
        // The signal set round-trips, sorted/deduped.
        assert!(back.signals.contains(&IdentitySignal::LegalName));
        assert_eq!(back.shared_contact_key.as_deref(), Some("recovery-key-1"));

        // Update (upsert) by id.
        let mut edited = anchor.clone();
        edited.label = "Renamed".to_string();
        store.upsert_account_anchor(&edited)?;
        assert_eq!(store.list_account_anchors()?.len(), 1);
        assert_eq!(store.list_account_anchors()?[0].label, "Renamed");

        assert!(store.delete_account_anchor("an1")?);
        assert!(!store.delete_account_anchor("an1")?);
        Ok(())
    }

    #[test]
    fn alias_provider_secrets_never_land_in_the_database() -> Result<()> {
        // D3c invariant: PROVIDER credentials go to the OS keystore, NOT the
        // DB. The alias record persists only the address + mapping; a provider
        // API token (here a stand-in secret) is stored via put_secret, which is
        // a BLOB column that SQLCipher encrypts at rest. We assert the alias row
        // carries no provider secret, and that scanning the on-disk DB file
        // never reveals the plaintext token (because the whole DB is encrypted).
        use crate::aliases::{AliasKind, EmailAlias};
        let dir = tempdir()?;
        let db = dir.path().join("fauxx.db");
        let secret_token = b"super-secret-provider-api-token-DO-NOT-LEAK";
        {
            let store = EncryptedStore::open_at(&db, &passphrase_source(dir.path()))?;
            // The provider credential is a keystore/secrets concern, not an
            // alias-row column. (In production it goes to the OS keystore; the
            // `secrets` table is the encrypted-at-rest fallback. Either way it
            // is never an email_aliases column.)
            store.put_secret("alias-provider-token", secret_token)?;

            let alias = EmailAlias::new(
                "a1".to_string(),
                "p1",
                "spokeo.com",
                "alice+spokeo.com@example.com",
                AliasKind::PlusAddress,
                10,
                Some("plus".to_string()),
            );
            store.upsert_email_alias(&alias)?;

            // The persisted alias JSON carries the address + provider LABEL, but
            // no secret token.
            let json: String = store.conn.query_row(
                "SELECT json FROM email_aliases WHERE id = 'a1'",
                [],
                |row| row.get(0),
            )?;
            assert!(json.contains("alice+spokeo.com@example.com"));
            assert!(json.contains("\"plus\"")); // provider label only
            assert!(
                !json.contains("super-secret"),
                "alias row must not carry a provider secret"
            );
        } // close the connection so all pages are flushed to disk

        // The raw DB file on disk is fully encrypted: the secret token never
        // appears in plaintext anywhere in it.
        let raw = std::fs::read(&db)?;
        assert!(
            !contains_subslice(&raw, secret_token),
            "the plaintext provider secret must never appear in the on-disk DB"
        );
        // The alias address likewise is encrypted at rest (sanity check that the
        // DB really is encrypted, not that the address is itself a secret).
        assert!(
            !contains_subslice(&raw, b"alice+spokeo.com@example.com"),
            "the DB must be encrypted at rest"
        );
        Ok(())
    }

    /// Whether `haystack` contains the contiguous byte sequence `needle`.
    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || haystack.len() < needle.len() {
            return false;
        }
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn migration_brings_c3_defense_tables_to_v8() -> Result<()> {
        // Forward migration creates the three new C3 tables; opening an existing
        // (older-shaped) DB still works because migrations only run forward.
        let dir = tempdir()?;
        let db = dir.path().join("fauxx.db");
        let src = passphrase_source(dir.path());
        let store = EncryptedStore::open_at(&db, &src)?;

        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        // The C3 defense tables landed at v8; later milestones append further
        // migrations (C4 shadow profiles at v9), so the schema is at least v8.
        const _: () = assert!(SCHEMA_VERSION >= 8);

        // The three new tables exist.
        for table in ["dsar_requests", "email_aliases", "account_anchors"] {
            let count: i64 = store.conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1, "expected table {table} to exist");
        }

        // Re-opening is a no-op migration (already current) and still reads.
        drop(store);
        let reopened = EncryptedStore::open_at(&db, &src)?;
        let version: i64 = reopened
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn installed_packs_round_trip_and_order_newest_first() -> Result<()> {
        use crate::personapack::{PackProvenance, PackRecord};
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        assert!(store.list_installed_packs()?.is_empty());
        assert!(store.get_installed_pack("missing")?.is_none());

        let mk = |id: &str, imported_at: i64, ids: Vec<String>| {
            InstalledPack::new(PackRecord {
                id: id.to_string(),
                provenance: PackProvenance::us("US_PUMS_2022", "seed", 1),
                signer_public_key: "AAAA".to_string(),
                schema_version: 1,
                persona_ids: ids,
                imported_at,
            })
        };
        let older = mk("pack-1", 1_000, vec!["p1".to_string()]);
        let newer = mk("pack-2", 2_000, vec!["p2".to_string(), "p3".to_string()]);
        store.upsert_installed_pack(&older)?;
        store.upsert_installed_pack(&newer)?;

        // Round-trips byte-faithfully.
        let back = store
            .get_installed_pack("pack-1")?
            .ok_or_else(|| CoreError::Key("pack-1 missing".into()))?;
        assert_eq!(back, older);

        // Listed newest-imported first.
        let all = store.list_installed_packs()?;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].record.id, "pack-2");
        assert_eq!(all[0].record.persona_count(), 2);
        assert_eq!(all[1].record.id, "pack-1");

        // The scalar persona_count column mirrors the record.
        let scalar_count: i64 = store.conn.query_row(
            "SELECT persona_count FROM installed_packs WHERE id = 'pack-2'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(scalar_count, 2);

        // Delete.
        assert!(store.delete_installed_pack("pack-1")?);
        assert!(!store.delete_installed_pack("pack-1")?);
        assert_eq!(store.list_installed_packs()?.len(), 1);
        Ok(())
    }

    #[test]
    fn device_ip_distinguishes_absent_from_unknown() -> Result<()> {
        let dir = tempdir()?;
        let store =
            EncryptedStore::open_at(&dir.path().join("fauxx.db"), &passphrase_source(dir.path()))?;

        // No row at all.
        assert_eq!(store.get_device_ip("peer")?, None);
        // Row exists but IP unknown.
        store.put_device_ip("peer", None, 5)?;
        assert_eq!(store.get_device_ip("peer")?, Some(None));
        // Row with a known IP.
        store.put_device_ip("peer", Some("203.0.113.7"), 6)?;
        assert_eq!(
            store.get_device_ip("peer")?,
            Some(Some("203.0.113.7".to_string()))
        );
        let all = store.list_device_ips()?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1.as_deref(), Some("203.0.113.7"));
        Ok(())
    }
}
