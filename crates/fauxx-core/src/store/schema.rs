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

//! Database schema and forward-only migrations.
//!
//! The schema covers the three domains the milestone names: `personas`,
//! `efficacy_history`, and `secrets`. The current version is tracked in the
//! built-in SQLite `user_version` pragma. Migrations are an append-only list;
//! opening a database runs every migration whose index is `>= user_version`,
//! then stamps the new version, all inside one transaction so an interrupted
//! upgrade rolls back cleanly.

use rusqlite::Connection;

use crate::error::Result;

/// The schema version this build expects. Equals the number of migrations.
pub const SCHEMA_VERSION: i64 = 15;

/// Ordered, forward-only migrations. Migration `i` upgrades the database from
/// version `i` to version `i + 1`. Never edit a shipped migration; append a new
/// one instead.
const MIGRATIONS: &[&str] = &[
    // v0 -> v1: initial schema for the three C0 domains.
    "
    CREATE TABLE personas (
        id            TEXT PRIMARY KEY NOT NULL,
        -- Canonical persisted form: the exact Android-compatible JSON, stored
        -- verbatim so cross-device round-trips are byte-faithful.
        json          TEXT NOT NULL,
        created_at    INTEGER NOT NULL,
        active_until  INTEGER NOT NULL
    );
    CREATE INDEX idx_personas_active_until ON personas (active_until);

    CREATE TABLE efficacy_history (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        persona_id  TEXT NOT NULL,
        recorded_at INTEGER NOT NULL,
        -- Opaque measurement payload (JSON); the schema is defined in a later
        -- milestone. Stored as TEXT so it round-trips losslessly for now.
        metric      TEXT NOT NULL,
        score       REAL NOT NULL
    );
    CREATE INDEX idx_efficacy_persona ON efficacy_history (persona_id, recorded_at);

    CREATE TABLE secrets (
        name        TEXT PRIMARY KEY NOT NULL,
        -- Already-encrypted-at-rest by SQLCipher; this is the secret value.
        value       BLOB NOT NULL,
        updated_at  INTEGER NOT NULL
    );
    ",
    // v1 -> v2: cross-device pairing (C1 #7). Paired-peer records (public keys
    // and connection hints) are access-control state, so they live behind
    // SQLCipher with the persona cache. The device's own secret pairing key
    // never lands here; it stays in the OS keystore (see store/keystore.rs).
    "
    CREATE TABLE paired_peers (
        -- The peer's X25519 public key, base64url, is the natural key.
        public_key   TEXT PRIMARY KEY NOT NULL,
        fingerprint  TEXT NOT NULL,
        -- The exact PairedPeer JSON, stored verbatim for lossless round-trips.
        json         TEXT NOT NULL,
        paired_at    INTEGER NOT NULL
    );
    CREATE INDEX idx_paired_peers_fingerprint ON paired_peers (fingerprint);
    ",
    // v2 -> v3: cross-device persona orchestration (C1 #8/#9/#10). All three
    // tables are household-coordination state, so they live behind SQLCipher
    // beside the persona cache.
    //
    // `orchestration_kv` is a tiny singleton key/value table for scalar mode
    // state (the active CoordinationMode). `device_assignments` records, per
    // device public key, which persona that device is pinned to (the Coherent
    // shared persona, or the per-device Fragmentation persona). `device_ip`
    // records each device's last observed public IP for WAN-IP linkage
    // detection (O3); a NULL ip means "unknown / not yet observed".
    "
    CREATE TABLE orchestration_kv (
        key    TEXT PRIMARY KEY NOT NULL,
        value  TEXT NOT NULL
    );

    CREATE TABLE device_assignments (
        -- The device's X25519 public key, base64url. The empty string is the
        -- reserved key for THIS device's own assignment.
        device_key  TEXT PRIMARY KEY NOT NULL,
        persona_id  TEXT NOT NULL,
        updated_at  INTEGER NOT NULL
    );

    CREATE TABLE device_ip (
        device_key  TEXT PRIMARY KEY NOT NULL,
        -- The observed public IP string, or NULL when unknown.
        ip          TEXT,
        observed_at INTEGER NOT NULL
    );
    ",
    // v3 -> v4: Privacy Sandbox Topics read-back (C2 #12 R2), the closed loop.
    // Each row is one Topics measurement taken from a decoy profile's own
    // `document.browsingTopics()` read after seeding category history. The
    // parsed AssignedTopic list is stored verbatim as JSON (`topics_json`) so it
    // round-trips losslessly to dashboards (C4) and campaigns (C8); the scalar
    // columns make the common queries (per persona, newest first; was the read
    // empty) index-friendly without re-parsing the JSON.
    //
    // `available` records whether the Topics API was callable at read time (the
    // flags were on and the context secure); `topic_count` is 0 for the common
    // epoch-boundary read (history seeded, weekly epoch not yet rolled), which is
    // a valid measurement, not a failure. `decoy_id` ties the read to the decoy
    // profile it came from. There is no foreign key to `personas`: a persona may
    // have rotated out by the time a dashboard reads its history, and the record
    // must survive that.
    "
    CREATE TABLE topics_measurements (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        persona_id   TEXT NOT NULL,
        decoy_id     TEXT NOT NULL,
        recorded_at  INTEGER NOT NULL,
        -- Whether document.browsingTopics() was callable (1) or not (0).
        available    INTEGER NOT NULL,
        -- Number of assigned topics in this read (0 at the epoch boundary).
        topic_count  INTEGER NOT NULL,
        -- The exact parsed Vec<AssignedTopic> JSON, stored verbatim.
        topics_json  TEXT NOT NULL
    );
    CREATE INDEX idx_topics_persona ON topics_measurements (persona_id, recorded_at);
    ",
    // v4 -> v5: the deterministic-channel defense (C3). Two tables.
    //
    // `broker_submissions` (D1c #15) is the lifecycle record of a data-broker
    // opt-out request: one row per submission, keyed by a UUID. The full
    // BrokerSubmission JSON is stored verbatim for lossless round-trips; scalar
    // columns (broker_id, persona_id, status, deadline) make the common queries
    // (per persona, by status, due deadlines) index-friendly without re-parsing
    // the JSON. There is no foreign key to `personas`: a persona may rotate out
    // while an opt-out is still being tracked, and the record must survive that.
    //
    // `gpc_site_status` (D4c #18) records, per site origin, the most recently
    // observed GPC-honoring status (parsed from the site's /.well-known/gpc.json
    // over the decoy browser). The full GpcSupport JSON is stored verbatim;
    // `honored` and `checked_at` are pulled out for fast filtering. The origin is
    // the natural key, so a re-check upserts the latest observation.
    "
    CREATE TABLE broker_submissions (
        id                 TEXT PRIMARY KEY NOT NULL,
        broker_id          TEXT NOT NULL,
        persona_id         TEXT NOT NULL,
        submitted_at       INTEGER NOT NULL,
        -- Lifecycle: drafted | submitted | confirmed | removed | relisted.
        status             TEXT NOT NULL,
        deadline           INTEGER NOT NULL,
        -- The exact BrokerSubmission JSON, stored verbatim.
        json               TEXT NOT NULL
    );
    CREATE INDEX idx_broker_submissions_persona ON broker_submissions (persona_id);
    CREATE INDEX idx_broker_submissions_status ON broker_submissions (status, deadline);

    CREATE TABLE gpc_site_status (
        -- The site origin (e.g. https://example.com) is the natural key.
        origin       TEXT PRIMARY KEY NOT NULL,
        -- 1 when the site advertised it honors GPC, else 0.
        honored      INTEGER NOT NULL,
        checked_at   INTEGER NOT NULL,
        -- The exact GpcSupport JSON, stored verbatim.
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_gpc_site_honored ON gpc_site_status (honored);
    ",
    // v5 -> v6: DSAR requests (C3 #16 D2c), the lawful identity-layer letters.
    //
    // `dsar_requests` is the lifecycle record of one statutory privacy letter
    // (GDPR access/erasure, CCPA access/deletion) to one controller for one
    // persona/subject: one row per request, keyed by a UUID. The full
    // DsarRequest JSON is stored verbatim for lossless round-trips; scalar
    // columns (kind, persona_id, status, deadline) make the common queries (per
    // persona, by status, overdue deadlines) index-friendly without re-parsing
    // the JSON. `deadline` is NULL while a request is still drafted (the
    // statutory clock starts only when the letter is marked sent). There is no
    // foreign key to `personas`: a persona may rotate out while a request is
    // still tracked, and the record must survive that. The letter text itself
    // is rendered on demand and never persisted, and the subject's real
    // identity details are passed at export time, not stored here.
    "
    CREATE TABLE dsar_requests (
        id           TEXT PRIMARY KEY NOT NULL,
        -- Statutory kind: gdpr-access | gdpr-deletion | ccpa-access | ccpa-deletion.
        kind         TEXT NOT NULL,
        persona_id   TEXT NOT NULL,
        -- Lifecycle: drafted | sent | acknowledged | fulfilled.
        status       TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        -- Statutory deadline (epoch millis), or NULL while still drafted.
        deadline     INTEGER,
        -- The exact DsarRequest JSON, stored verbatim.
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_dsar_persona ON dsar_requests (persona_id);
    CREATE INDEX idx_dsar_status ON dsar_requests (status, deadline);
    ",
    // v6 -> v7: email aliases (C3 #17 D3c).
    //
    // `email_aliases` maps one address to the (persona, site) pair it fronts:
    // one row per alias, keyed by a UUID. The full EmailAlias JSON is stored
    // verbatim; scalar columns (persona_id, site, address, status) make the
    // inventory queries and the no-reuse-across-sites check index-friendly. The
    // address index is deliberately NON-unique: the no-reuse-across-sites rule
    // is an APPLICATION-layer policy that the user can explicitly override
    // (sharing one address across sites), and rotation keeps a revoked row with
    // the old address beside the fresh one, so a hard DB uniqueness constraint
    // would wrongly forbid both. PROVIDER credentials never land here: a future
    // HTTP masking provider's API token lives in the OS keystore, not the DB;
    // only the alias->persona->site mapping is persisted.
    "
    CREATE TABLE email_aliases (
        id           TEXT PRIMARY KEY NOT NULL,
        persona_id   TEXT NOT NULL,
        site         TEXT NOT NULL,
        address      TEXT NOT NULL,
        -- plus-address | masked.
        kind         TEXT NOT NULL,
        -- active | revoked.
        status       TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        -- The exact EmailAlias JSON, stored verbatim.
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_email_aliases_persona ON email_aliases (persona_id);
    CREATE INDEX idx_email_aliases_site ON email_aliases (persona_id, site);
    CREATE INDEX idx_email_aliases_address ON email_aliases (persona_id, address);
    ",
    // v7 -> v8: account anchors (C3 #19 D5c), the user-curated inventory.
    //
    // `account_anchors` is the user-curated inventory of real accounts and the
    // identity signals each anchors: one row per account, keyed by a UUID. The
    // full AccountAnchor JSON (including the signal set and the optional shared-
    // contact key for cross-account linkage detection) is stored verbatim. This
    // is a READ-ONLY analysis inventory: the scanner scores and recommends over
    // it but never scrapes or automates against any real account. There is no
    // foreign key and no credential column; the raw recovery contact is never
    // stored, only an opaque shared-contact key the user supplies for linkage.
    "
    CREATE TABLE account_anchors (
        id                 TEXT PRIMARY KEY NOT NULL,
        label              TEXT NOT NULL,
        site               TEXT NOT NULL,
        created_at         INTEGER NOT NULL,
        -- The exact AccountAnchor JSON, stored verbatim.
        json               TEXT NOT NULL
    );
    CREATE INDEX idx_account_anchors_site ON account_anchors (site);
    ",
    // v8 -> v9: shadow profiles for the control-profile A/B (C4 #21 A2).
    //
    // `shadow_profiles` is the set of experimental arms the A/B comparison runs
    // over: one row per profile, keyed by a UUID. Each profile is a TREATED
    // (noised) or untreated CONTROL arm bound to its OWN persona, so their drift
    // metrics are tracked separately and compared. The full ShadowProfile JSON
    // is stored verbatim for lossless round-trips; the scalar `arm` and
    // `persona_id` columns make the per-arm cohort queries index-friendly. There
    // is no foreign key to `personas`: a profile's persona may rotate while the
    // experiment is still tracked, and the definition must survive that.
    "
    CREATE TABLE shadow_profiles (
        id           TEXT PRIMARY KEY NOT NULL,
        label        TEXT NOT NULL,
        -- The experimental arm: treated | control.
        arm          TEXT NOT NULL,
        persona_id   TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        -- The exact ShadowProfile JSON, stored verbatim.
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_shadow_profiles_arm ON shadow_profiles (arm);
    CREATE INDEX idx_shadow_profiles_persona ON shadow_profiles (persona_id);
    ",
    // v9 -> v10: per-broker identity scan snapshots for the broker diff view
    // (C4 #22 A3).
    //
    // `broker_scan_snapshots` records, per (broker, persona) at a point in time,
    // the SET of identity fields/records the broker exposes about that persona.
    // One row per scan, keyed by a UUID. The C3 D1c re-listing seam
    // (`ListingCheck`) only returns a bool; this richer snapshot is what the A3
    // diff view diffs across time to classify fields as added/removed/unchanged
    // and to flag re-listing (a removed field reappearing). The live scanning
    // that POPULATES snapshots from a broker site is DEFERRED (like the C3 live
    // `ListingCheck`); A3 computes diffs from STORED snapshots.
    //
    // The full BrokerScanSnapshot JSON (including the exposed-field set) is
    // stored verbatim for lossless round-trips; the scalar `broker_id`,
    // `persona_id`, and `scanned_at` columns make the per-(broker, persona)
    // time-ordered query index-friendly without re-parsing the JSON. There is no
    // foreign key to `personas`: a persona may rotate out while its broker
    // exposure is still being tracked, and the record must survive that.
    "
    CREATE TABLE broker_scan_snapshots (
        id           TEXT PRIMARY KEY NOT NULL,
        broker_id    TEXT NOT NULL,
        persona_id   TEXT NOT NULL,
        scanned_at   INTEGER NOT NULL,
        -- The exact BrokerScanSnapshot JSON, stored verbatim.
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_broker_scan_snapshots_lookup
        ON broker_scan_snapshots (broker_id, persona_id, scanned_at);
    ",
    // v10 -> v11: desktop-LOCAL Persona Studio editor metadata (C5 #24 P1).
    //
    // `persona_settings` is the desktop-only editor state for each persona: which
    // fields the user has LOCKED (so they survive regeneration and rotation) and
    // the persona's rotation schedule (the frozen 8-to-10-day cadence, or disabled
    // to PIN the persona). One row per persona, keyed by the persona id.
    //
    // This is DELIBERATELY separate from the synced `personas` JSON: locking and
    // rotation tuning are a desktop authoring concern and must NOT pollute the
    // cross-device wire model the phone reads, so they never ride in the persona
    // JSON and the Android round-trip stays byte-faithful. The full
    // PersonaSettings JSON is stored verbatim for lossless round-trips. There is
    // no foreign key to `personas`: settings may briefly outlive a deleted/rotated
    // persona without corrupting the table.
    "
    CREATE TABLE persona_settings (
        persona_id   TEXT PRIMARY KEY NOT NULL,
        -- The exact PersonaSettings JSON (locked fields + rotation), verbatim.
        json         TEXT NOT NULL
    );
    ",
    // v11 -> v12: the installed persona-pack ledger (C5 #27 P4).
    //
    // `installed_packs` records each signed persona pack that was imported into
    // this device's library: one row per pack, keyed by a UUID minted on import.
    // It is the library index the (later) GUI library view lists and removes
    // over; the personas a pack carried land in the `personas` table proper, so
    // a pack row is metadata (provenance, signer, persona count, the full
    // PackRecord JSON) rather than a copy of the persona payloads.
    //
    // `signer_public_key` is the base64 ed25519 public key the pack was signed
    // with; `source_label` is the pack's provenance distribution label;
    // `schema_version` is the pack format version it declared; `persona_count`
    // is how many personas it carried; `imported_at` is when it was installed.
    // The full PackRecord JSON is stored verbatim for lossless round-trips. There
    // is no foreign key to `personas`: an imported persona may be rotated or
    // deleted while the pack ledger row survives as an audit record of what was
    // installed.
    "
    CREATE TABLE installed_packs (
        id                 TEXT PRIMARY KEY NOT NULL,
        source_label       TEXT NOT NULL,
        signer_public_key  TEXT NOT NULL,
        schema_version     INTEGER NOT NULL,
        persona_count      INTEGER NOT NULL,
        imported_at        INTEGER NOT NULL,
        -- The exact PackRecord JSON (provenance + signer + persona ids), verbatim.
        json               TEXT NOT NULL
    );
    CREATE INDEX idx_installed_packs_imported ON installed_packs (imported_at);
    CREATE INDEX idx_installed_packs_signer ON installed_packs (signer_public_key);
    ",
    // v12 -> v13: per-persona network egress (C7 #30 N1).
    //
    // `persona_egress` binds, per persona, how that persona's decoy browser
    // reaches the internet: Direct (OS route), an HTTP/SOCKS proxy, Tor, or a
    // VPN's local SOCKS/HTTP front. One row per persona, keyed by the persona id.
    // The full Egress JSON is stored verbatim for lossless round-trips.
    //
    // CREDENTIALS ARE NOT STORED HERE. The persisted Egress JSON carries only a
    // non-secret keystore account label (ProxyAuth) when a proxy needs auth; the
    // secret username/password live in the OS keystore (see store/keystore.rs
    // proxy-credential helpers), never in this row and never in a log. There is
    // no foreign key to `personas`: an egress binding may briefly outlive a
    // rotated/deleted persona without corrupting the table, and the application
    // layer cleans it up.
    "
    CREATE TABLE persona_egress (
        persona_id   TEXT PRIMARY KEY NOT NULL,
        -- The exact Egress JSON (kind + endpoint + non-secret auth marker).
        json         TEXT NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    ",
    // v13 -> v14: per-persona DNS strategy (C7 #31 N2). N2 layers on N1.
    //
    // `persona_dns` binds, per persona, how that persona's decoy browser resolves
    // DNS: SystemDefault (OS resolver), DoH (an https:// resolver template), or
    // DoT (a resolver endpoint). One row per persona, keyed by the persona id.
    // The full DnsStrategy JSON is stored verbatim for lossless round-trips. The
    // chosen resolver is applied to the SAME isolated decoy profile as the egress
    // so lookups and traffic share one observer where the egress supports it.
    //
    // DNS choices are persisted (never logged as sensitive); the observer
    // trade-off (the chosen DoH/DoT resolver sees the persona's lookups) is made
    // explicit in the types (DnsStrategy::observer_note), not hidden in the DB.
    // No foreign key to `personas`, for the same rotation-survival reason as
    // `persona_egress`.
    "
    CREATE TABLE persona_dns (
        persona_id   TEXT PRIMARY KEY NOT NULL,
        -- The exact DnsStrategy JSON (mode + optional resolver template).
        json         TEXT NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    ",
    // v14 -> v15: goal-driven campaigns (C8 #33 U2), the closed loop.
    //
    // `campaigns` is the lifecycle record of one goal-driven campaign: a goal
    // (target metric, comparator, threshold) over a target segment/category for
    // a persona, plus its lifecycle (planned | running | achieved | paused) and
    // its closed-loop PROGRESS (the last observed metric, the last gap, and how
    // long the goal has held continuously, for the Achieved dwell). One row per
    // campaign, keyed by a UUID. The full Campaign JSON is stored verbatim for
    // lossless round-trips; the scalar columns (persona_id, status, updated_at)
    // make the per-persona and by-status queries index-friendly without
    // re-parsing the JSON.
    //
    // Persisting the campaign AND its progress is the acceptance criterion: a
    // campaign survives restart with its dwell clock intact, so a box that
    // reboots mid-campaign resumes rather than restarting the goal. There is no
    // foreign key to `personas`: a persona may rotate out while a campaign is
    // still tracked, and the record must survive that.
    "
    CREATE TABLE campaigns (
        id           TEXT PRIMARY KEY NOT NULL,
        persona_id   TEXT NOT NULL,
        -- Lifecycle: planned | running | achieved | paused.
        status       TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL,
        -- The exact Campaign JSON (goal + target segment + lifecycle + progress).
        json         TEXT NOT NULL
    );
    CREATE INDEX idx_campaigns_persona ON campaigns (persona_id);
    CREATE INDEX idx_campaigns_status ON campaigns (status, updated_at);
    ",
];

/// Read the current schema version from `PRAGMA user_version`.
fn current_version(conn: &Connection) -> Result<i64> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(v)
}

/// Apply every pending migration in a single transaction and stamp the new
/// version. A no-op when the database is already current.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let from = current_version(conn)?;
    if from < 0 {
        // Defensive: a negative user_version is corruption, not an upgrade path.
        return Err(crate::error::CoreError::Key(
            "database reports a negative schema version".to_string(),
        ));
    }
    if from as usize >= MIGRATIONS.len() {
        return Ok(());
    }

    let tx = conn.transaction()?;
    for stmt in &MIGRATIONS[from as usize..] {
        tx.execute_batch(stmt)?;
    }
    // `PRAGMA user_version` does not accept bound parameters.
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_count_matches_schema_version() {
        // The version is defined as the number of migrations; keep them in lock
        // step so a forgotten SCHEMA_VERSION bump is caught.
        assert_eq!(MIGRATIONS.len() as i64, SCHEMA_VERSION);
    }

    #[test]
    fn forward_migration_preserves_existing_data() -> Result<()> {
        // Build an OLD database at the earliest real version (v1, just the
        // initial schema), insert a sentinel row, then migrate forward to the
        // current version and assert both that the version advances AND that the
        // pre-existing row survives. This exercises the full incremental chain
        // (v1 -> current), not just a freshly-opened database, so a future edit
        // that breaks an intermediate migration or drops data is caught.
        let mut conn = Connection::open_in_memory()?;
        const OLD_VERSION: usize = 1;
        {
            let tx = conn.transaction()?;
            for stmt in &MIGRATIONS[..OLD_VERSION] {
                tx.execute_batch(stmt)?;
            }
            tx.execute_batch(&format!("PRAGMA user_version = {OLD_VERSION};"))?;
            tx.commit()?;
        }
        // A sentinel row in `personas`, which exists since v1.
        conn.execute(
            "INSERT INTO personas (id, json, created_at, active_until) \
             VALUES ('sentinel', '{}', 1, 2)",
            [],
        )?;

        migrate(&mut conn)?;

        assert_eq!(current_version(&conn)?, SCHEMA_VERSION);
        let survived: i64 = conn.query_row(
            "SELECT COUNT(*) FROM personas WHERE id = 'sentinel'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            survived, 1,
            "pre-existing row must survive forward migration"
        );
        // A table introduced by the latest migration must now exist.
        let latest_table: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'campaigns'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            latest_table, 1,
            "newest migration's table must exist after upgrade"
        );
        Ok(())
    }

    #[test]
    fn migrate_is_idempotent_on_current_db() -> Result<()> {
        let mut conn = Connection::open_in_memory()?;
        migrate(&mut conn)?;
        let after_first = current_version(&conn)?;
        // A second migrate is a no-op (already current).
        migrate(&mut conn)?;
        assert_eq!(current_version(&conn)?, after_first);
        assert_eq!(after_first, SCHEMA_VERSION);
        Ok(())
    }
}
