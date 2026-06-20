# C6 Persona-Pack Minting: the PUMS generator on desktop (H2, #29)

This note documents the C6 MINT core: the desktop runs the PUMS-microdata persona
generator, mints COHERENT synthetic personas from a US-only joint distribution,
bundles them into a SIGNED persona pack (the C5 P4 format), and distributes the
pack to paired peers over the existing O1 sealed channel. The phone VERIFIES the
pack signature before importing the personas (verify-before-write, fail closed).
All of it lives in `fauxx-core` behind the clean async `Core` API. There is NO
GUI/CLI code here. Everything is 100% local (no network, no telemetry) and is
reachable headless.

The module is `crate::mint`. It reuses the FROZEN Android E7 PUMS persona model,
the C5 P4 signed-pack format (`crate::personapack`), the C5 P2 coherence linter
(`crate::studio::lint_persona`), and the C1 O1 sealed channel (`crate::sync`).

## What the Android side froze (and we match)

- The E7 `PersonaDistribution` / `DemographicCell` model: a `DemographicCell` is a
  co-occurring `(age, profession, region)` TRIPLE with a population `weight`, and a
  persona's demographics are drawn JOINTLY from one cell (a weighted multinomial
  draw), so the three fields CO-OCCUR realistically rather than being picked
  independently across an impossible `age x profession x region` cross-product.
- US-only joint sampling: every cell's region is a `US_*` region; a valid-enum but
  non-US region is rejected on load (fail closed).
- The frozen `SyntheticPersona` shape (the cross-device wire model), the 32-value
  `CategoryPool`, and the 8-to-10-day rotation window (`BASE_ROTATION_DAYS` +
  `ROTATION_JITTER_DAYS`, added never subtracted) are REUSED, not re-implemented.

## The bundled SEED distribution is a PLACEHOLDER

The distribution is loaded from a BUNDLED JSON (`src/mint/us_pums_seed.json`)
embedded at compile time via `include_str!` + `serde`. It is a small, hand-authored
SEED, NOT the real distribution. In production the real cells are built OFFLINE from
US Census ACS PUMS microdata, the analogue of the Android
`scripts/build_persona_distribution.py`, and this file is replaced by that export.
The file format is `{ version, source_label, cells: [ { age, profession, region,
weight } ] }`; the minter VALIDATES every cell on load:

- `weight` is finite and strictly positive (`weight > 0`);
- `age` / `profession` / `region` are valid `AgeRange` / `Profession` / `Region`
  enum NAMES;
- `region` is US-only (its enum NAME starts with `US_`).

A malformed file is `MintError::Malformed`, an empty cell list is `MintError::Empty`,
and a bad cell is `MintError::InvalidCell { index, reason }`. An invalid distribution
NEVER mints.

## Joint sampling (deterministic)

`PersonaDistribution::sample_cell` draws ONE cell via a weighted multinomial draw
(cumulative search): sample `u` in `[0, total_weight)` and walk the cells
accumulating weight, returning the cell whose cumulative band contains `u`. Because
a WHOLE cell is drawn, the minted `(age, profession, region)` is always one of the
distribution's real triples, never an impossible cross-product. Randomness flows
through a seedable `StdRng` (`seed_from_u64`), so a fixed seed yields identical
demographics, interests, and rotation windows. The one intentionally-fresh field is
the persona's UUID v4 id (ids must be globally unique).

## Coherent personas

For each persona the minter samples the cell, then samples 3-to-5 DISTINCT interests
(a partial Fisher-Yates shuffle over the frozen `CategoryPool`), stamps a fresh UUID
and the 8-to-10-day `activeUntil` window, and accepts the draw ONLY if the C5
coherence linter (`lint_persona`) finds NO `HardImplausible` finding. A flagged draw
is re-sampled, bounded by `MAX_RESAMPLE_ATTEMPTS` (a persistently incoherent draw is
`MintError::Incoherent`, fail closed, never an incoherent persona). The curated
distribution makes a flag rare; the bound just prevents a pathological distribution
from spinning forever.

## Signing + provenance (reusing C5 P4)

`mint_pack` bundles the minted personas into a signed `PersonaPack` (the C5 P4
format) with a `PackProvenance::us(...)` recording the source distribution label and
the generation seed (so the draw is reproducible), signed with the device's
pack-signing key (the same key class the persona packs and C6 H1 artifacts use; its
32-byte seed lives in the OS keystore, or the headless passphrase-file fallback).
The pack deserializes into the phone's `SyntheticPersona` shape (guaranteed by P4)
and verifies with `verify_pack`.

## Delivery over O1 + verify-before-write

A `SyncBody::PersonaPack` wire kind carries the signed pack (its JSON bytes, as a
base64 string) over the existing LanSync sealed channel to paired peers (see the
`sync::wire` module). The unknown-kind fail-closed posture is
preserved: a peer that does not know the kind rejects it at parse. The receiver
VERIFIES the pack signature (P4 `verify_pack`) BEFORE importing the personas: a
tampered, unsigned, wrong-key, or unknown/newer-schema-version pack is rejected
(fail closed) and NOTHING is written; a valid pack lands the personas in the
encrypted store and records the pack in the installed-pack ledger.

## Headless API

- `Core::mint_personas(count, seed)` mints coherent personas WITHOUT packing or
  persisting (pure; works store-less).
- `Core::mint_persona_pack(count, seed)` mints + signs into a `PersonaPack` and
  returns the pack bytes (requires an open store for the signing key).
- `Core::mint_and_push_pack(count, seed)` mints, signs, AND pushes the pack to every
  paired peer over the sealed channel; returns the pack bytes and the peer count.
- `Core::receive_pack_frame(sender_key, frame)` opens + authenticates a sealed
  frame, verifies the carried pack, then imports its personas into the store
  (verify-before-write).

All of the store-backed methods fail closed on a store-less core and touch no
network beyond the local sealed channel.

## Tests

Hermetic, fixed-seed, temp `EncryptedFile` store (never the OS keystore): the
bundled distribution loads and every cell validates (US-only, valid enums,
`weight > 0`); joint sampling is deterministic for a fixed seed and a new seed
re-rolls it; every minted demographic triple is a real cell (not an impossible
cross-product); minted personas pass the C5 linter with no `HardImplausible`
finding and carry 3-to-5 distinct interests plus an 8-to-10-day window; a minted
pack signs and verifies (reusing P4) and a tampered pack is rejected; the
`PersonaPack` wire kind round-trips the sealed channel and an unknown kind still
fails closed at parse; and a received pack's personas land in the encrypted store
while a tampered pack received over the channel is rejected and writes nothing.
