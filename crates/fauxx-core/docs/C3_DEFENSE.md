# C3 deterministic-channel defense (identity layer)

This note documents the three lawful, identity-layer defenses the desktop
companion performs that the phone deliberately cannot: DSAR letters (#16, D2c),
email-alias management (#17, D3c), and the account-anchor scanner (#19, D5c).
All three live in `fauxx-core` behind the clean async `Core` API; the GUI and
CLI are thin clients. They join the broker opt-out automation (#15, D1c) and
GPC honoring (#18, D4c) already shipped in this milestone.

These features fail closed, are 100% local (no network, no telemetry), persist
behind SQLCipher, and never automate against a real authenticated account.

## D2c: DSAR helper (`crate::dsar`)

Generates and tracks the statutory privacy letters a data subject may send.

- Four letter kinds: GDPR access (Art. 15), GDPR erasure (Art. 17), CCPA/CPRA
  right to know, and CCPA/CPRA right to delete. Each carries its legal framing
  and is rendered from a template filled with the subject's real identity
  details (`SubjectDetails`, supplied at export time, never persisted) and the
  target `Controller`.
- The controller is either a known broker (reusing the `crate::brokers`
  registry via `Controller::resolve_broker`) or an arbitrary name + contact.
- Statutory deadline, computed from the send date with the `time` crate:
  - GDPR = one CALENDAR month (same day-of-month next month, clamped to the
    last day for short months; Jan 31 -> Feb 28, or Feb 29 in a leap year;
    time-of-day preserved; year rolls over at December).
  - CCPA = a flat 45 days.
- Lifecycle: `drafted -> sent -> acknowledged -> fulfilled`. The statutory
  clock starts only at `sent`; `overdue` and `due-soon` are derived predicates,
  not stored statuses. A `fulfilled` request is never overdue.
- Letters are EXPORTED as rendered text (`DsarLetter`) for the user to send by
  hand. The core NEVER auto-sends a legal letter: no SMTP, no network.
- Persisted in the `dsar_requests` table (schema v5 -> v6).

## D3c: email-alias management (`crate::aliases`)

Mints or records a per-`(persona, site)` email address so a leaked address
reveals only the one site it fronts.

- Two address kinds today: locally generated PLUS-ADDRESSES
  (`base+tag@domain`, via `PlusAddressProvider`, no provider needed) and
  MANUALLY-created MASKED aliases the user records (e.g. an iCloud Hide-My-Email
  forward).
- Inventory: list which address fronts which site for which persona; `revoke`
  (keeps the row for the audit trail) and `rotate` (revoke the old, mint a
  fresh active alias for the same site).
- No two sites for one persona reuse the same alias unless the caller
  explicitly opts in (`allow_reuse`); the rule is enforced at the application
  layer in `Core::persist_new_alias`. The DB address index is intentionally
  non-unique so explicit reuse and revoked-then-rotated rows coexist.
- Persisted in the `email_aliases` table (schema v6 -> v7). Only the
  alias -> persona -> site mapping is stored. PROVIDER credentials go in the OS
  keystore, never the DB; a store test asserts no provider secret reaches the
  on-disk database (the whole DB is encrypted at rest).

### DEFERRED: masking-provider HTTP API

A masking-PROVIDER API integration (addy.io / SimpleLogin / Apple
Hide-My-Email, etc., that mints fresh forwarding addresses over HTTP) is
DEFERRED: the workspace deliberately carries no `reqwest`. The `AliasProvider`
trait is the seam a future provider impl slots into. It is object-safe and
async precisely so a network-backed provider drops in without touching the
`Core` API or the schema. When that provider lands:

- it constructs itself by loading its API token from the OS keystore (the
  existing `KeySource` path), NOT from the DB;
- it implements `AliasProvider::mint` with an HTTP call and reports
  `AliasKind::Masked` and its own `id()`;
- `Core::mint_email_alias` / `rotate_email_alias` already accept any
  `&dyn AliasProvider`, so no Core change is required.

## D5c: account-anchor scanner (`crate::anchors`)

Read-only analysis over a USER-CURATED inventory of real accounts.

- HARD GUARDRAIL: it NEVER scrapes, logs into, or automates against any real
  account. The user types in their accounts and the identity signals each
  anchors; the scanner only computes over that in-memory inventory. There is no
  method that takes a credential, drives a browser, or performs account I/O. A
  unit test asserts the analysis surface is pure (re-running never mutates the
  inventory and is deterministic).
- Signals: verified email, phone number, legal name, payment, recovery contact.
- Scoring heuristic (`anchor_score`): `score = strength + linked_accounts *
  LINK_BONUS_PER_ACCOUNT`, where `strength` is the sum of per-signal weights
  (legal name/payment = 5, recovery contact = 4, phone = 3, email = 2) and
  `linked_accounts` is how many OTHER accounts share this anchor's opaque
  `shared_contact_key` (cross-account linkage detected WITHOUT storing the raw
  contact). Higher = a more dangerous anchor.
- Prioritized recommendations (`recommendations`), ordered by linkage strength:
  split a bridging recovery contact (highest leverage), front an account with
  its own alias (feeds D3c), and isolate a high-strength anchor. Advice only;
  acting on it (e.g. minting an alias) is a separate, user-driven step.
- Persisted in the `account_anchors` table (schema v7 -> v8). No credential
  column; the raw recovery contact is never stored, only the opaque key.

## Schema migrations

The store migrates FORWARD via the existing `PRAGMA user_version` pattern in
`store/schema.rs`. C3 adds three append-only migrations, taking
`SCHEMA_VERSION` from 5 to 8 (`dsar_requests`, `email_aliases`,
`account_anchors`). Older databases still open and upgrade in place.
