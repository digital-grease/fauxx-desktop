# C5 Persona Studio (the studio core)

This note documents the Persona Studio CORE: the editable persona model and its
desktop-local editor metadata (#24, P1), the coherence linter (#25, P2), and the
deterministic week simulator (#26, P3). All of it lives in `fauxx-core` behind
the clean async `Core` API. The Iced GUI views are a SEPARATE later batch, so
there is NO GUI/CLI code here. Everything is 100% local (no network, no
telemetry) and is reachable headless.

The studio core is `crate::studio`, with submodules `settings`, `linter`, and
`simulator`, plus the `PersonaChanged` change-event type. The editable persona
fields are additive on `crate::persona::SyntheticPersona`; the desktop-local
editor metadata persists in a new `persona_settings` store table.

## P1: the editable persona model (#24)

### Additive, optional wire fields

The cross-device `SyntheticPersona` is the FROZEN wire model that must keep
round-tripping byte-faithfully to the Android Gson JSON (the exact eight
camelCase keys; demographics as enum-NAME strings). C5 adds three desktop-only
editor fields, all ADDITIVE and OPTIONAL:

- `homeLocation` (`home_location: Option<String>`)
- `schedule` (`schedule: Option<String>`)
- `browsingStyle` (`browsing_style: Option<String>`)

Each carries `#[serde(default, skip_serializing_if = "Option::is_none")]`, so
when unset the key never appears on the wire and the phone's lenient reader never
sees it. The round-trip is proven lossless by tests: the eight Android keys
serialize exactly (plus the already-defaulted `schemaVersion`), the new fields
are omitted when `None`, and an old phone JSON with only the eight keys still
deserializes (with the new fields defaulting to `None`).

### Desktop-local editor metadata (NOT synced)

Per-field LOCKING and rotation tuning are a desktop authoring concern and must
NOT pollute the synced wire model, so they live in `crate::studio::settings` as
`PersonaSettings`, keyed by persona id, and persist in their OWN encrypted-store
table `persona_settings` (migration v10 -> v11), NEVER in the persona JSON:

- `locked_fields`: a sorted set of `PersonaField` identifiers. A locked field is
  one the user pinned; the studio's regeneration and rotation logic preserves a
  locked field's value rather than re-rolling it.
- `rotation`: a `RotationSchedule`, either the frozen 8-to-10-day `Cadence`
  (base `BASE_ROTATION_DAYS` = 7 plus jitter up to the top of
  `ROTATION_JITTER_DAYS`, so the window upper bound is 10) or `Disabled` to PIN
  the persona so it never auto-rotates.

`Core` exposes: `persona_settings`, `save_persona_settings`, `set_field_locked`,
and `set_rotation_schedule`. A store-less core returns the default settings
(nothing locked, frozen cadence) and refuses writes with `Unimplemented`.

### The change-event mechanism

Dependent views recompute when a persona changes. The core exposes a NON-GUI
stream: `Core::subscribe_persona_changes()` returns a
`tokio::sync::broadcast::Receiver<PersonaChanged>`. A `PersonaChanged` carries
the affected persona id and a `kind` (`Saved`, `Deleted`, `SettingsChanged`); it
is a pure recompute trigger and carries no persona payload (a subscriber reloads
current state through the `Core` API, avoiding stale snapshots racing the store).
`save_persona` emits `Saved`, `delete_persona` emits `Deleted` (and drops the
orphaned settings row), and saving settings emits `SettingsChanged`. The channel
is always present, even on a store-less core, so a subscriber can attach before a
store is opened. A lagging subscriber simply reloads current state.

## P2: the coherence linter (#25)

`Core::lint_persona(&persona)` (and `lint_persona_by_id`) returns a list of
`Finding`s WITHOUT mutating the persona; a clean persona returns an empty list. A
`Finding` has a `Severity` (`Warning` vs `HardImplausible`), a human-readable
`reason`, and the affected `fields`.

Two tiers:

- HARD-IMPLAUSIBLE rules port the Android `PersonaConsistencyRules`. Required-
  field completeness (seeded by the existing `SyntheticPersona::validate`
  `PersonaIssue` checks: unknown enum values, the 3..=5 interest-count window,
  an empty name) maps to `HardImplausible`, as do the hard incompatible-trait
  pairs: AGE_65_PLUS + ACADEMIC with fewer than three interests; AGE_18_24 +
  RETIREMENT without FINANCE or REAL_ESTATE; PARENTING + AGE_18_24 with a single
  interest; and AGE_18_24 with the RETIRED profession.
- WARNING rules are driven by real-distribution data: a bundled category
  co-occurrence table (`studio/category_cooccurrence.json`, embedded via
  `include_str!` + `serde`, documented as a PLACEHOLDER SEED for the real
  PUMS-derived joint distribution). Any persona interest PAIR whose stored
  co-occurrence rate is at or below the table's `warn_at_or_below` threshold is
  flagged as a `Warning`. Findings sort hard-implausible first, then warnings.

### Recompute on the P1 change event

Linting is a pure function of a persona, so a subscriber drives it: take a
`PersonaChanged` off `subscribe_persona_changes`, reload the persona with
`get_persona`, and call `lint_persona`. The GUI subscription is a later batch;
this module is the headless computation it will call.

## P3: the week simulator (#26)

`Core::simulate_week(&persona, intensity, seed)` (and `simulate_week_for` by id)
produces a deterministic, seedable synthetic `SimulatedWeek`: a timeline of decoy
`SimulatedSession`s, each a run of `SimulatedQuery`s with a time (`at_secs`), a
`category`, and a `QueryWeighting`. It performs NO real browsing or network
access.

The preview matches EXECUTION because it reuses the two frozen models:

- Category selection uses the persona-following weighting from
  `crate::constants`: with probability `PERSONA_FOLLOW_FRACTION` (0.85) the query
  follows the persona (drawn from its interest categories, each carrying
  `ALIGNED_WEIGHT` = 2.0 relative to the `NEUTRAL_WEIGHT` a non-interest would,
  with `MISALIGNED_WEIGHT` = 0.3 reserved for explicitly contradicted
  categories); the remaining 0.15 is uniform-baseline noise (the
  `UNIFORM_BASELINE_WEIGHT` = 0.6 component) drawn uniformly across all
  categories to blur the fingerprint. So category frequencies lean strongly
  toward the persona's interests.
- Timing uses the same circadian Poisson model as the C1 household scheduler:
  activity only inside the 07:00-23:00 active window, with Poisson inter-arrival
  delays `-ln(1 - u) / rate` where `rate` comes from the `IntensityLevel` ladder.

Determinism: all randomness flows through a seedable `StdRng` (`seed_from_u64`)
mixed with a stable FNV-1a hash of the persona id, so the SAME
`(persona, intensity, seed)` yields an identical week and a NEW seed re-rolls it.
An interest-less persona degrades cleanly to pure uniform-baseline noise without
panicking.

## Schema migration

The new `persona_settings` table is added by migration v10 -> v11 via the
existing forward-only `PRAGMA user_version` pattern in `store/schema.rs`; the
migration test asserts the version advances, a pre-existing row survives, and the
new table exists. There is no foreign key to `personas`: settings may briefly
outlive a deleted/rotated persona without corrupting the table (and `Core`
proactively drops the row when a persona is deleted).
