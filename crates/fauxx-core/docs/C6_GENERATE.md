# C6 Generate-on-Desktop, Execute-on-Phone (H1, #28)

This note documents the C6 GENERATE core: the desktop runs the heavy generation
work and pushes SIGNED artifacts to the paired phone over the existing O1 sealed
channel; the phone verifies the signature and freshness, then either REPLAYS the
artifact or FALLS BACK to its own on-device generation. All of it lives in
`fauxx-core` behind the clean async `Core` API. There is NO GUI/CLI code here.
Everything is 100% local (no network, no telemetry) and is reachable headless.

The module is `crate::generate`, with submodules `allocator` (the
adversarial-allocation surrogate + weight normalizer) and `plan` (the
category-targeted query-plan generator), plus the signed-artifact envelope and
the ed25519 sign/verify primitives.

## What the Android side froze (and we match)

- E4 `AdversarialAllocator.allocate`: coordinate descent over the per-category
  weights, the multiplicative factors `[0.4, 0.6, 0.8, 1.25, 1.7, 2.5]`, 10
  passes, an `EPS` of `1e-6`, a 0.15-nat KL budget, the `SensitiveAttributes`
  denylist left unperturbed, then a `WeightNormalizer` MIN_WEIGHT (0.001)
  two-pass clamp-and-divide. This is ported FAITHFULLY in
  `crate::generate::allocator`.
- E6 `GrammarQueryGenerator`: the on-device query-string generator. We do NOT
  port its query banks + grammar (see "Honesty" below).
- The frozen 32-value `CategoryPool`, the 85/15 persona-follow weighting
  constants (`crate::constants`), and the circadian Poisson timing
  (`crate::orchestration::scheduler`) are REUSED, not re-implemented.

## Step 1: the adversarial-allocation weight map (E4)

`allocator::allocate(combined, protected_interests, kl_budget)` runs coordinate
descent:

- It visits every `CategoryPool` in the fixed `CategoryPool::all` order, for
  `PASSES = 10` sweeps.
- On each PERTURBABLE category it tries `FACTORS = [0.4, 0.6, 0.8, 1.25, 1.7,
  2.5]` in order and takes the FIRST factor that strictly lowers the adversary
  loss surrogate AND keeps the candidate's KL divergence from the input baseline
  within `kl_budget` (0.15 nats by default). It is fully DETERMINISTIC (no
  randomness), so a known input yields one fixed allocation.
- The loss surrogate is the negative log-mass placed on the protected interests:
  minimizing it spreads probability toward the user's pinned interests, while
  the KL budget bounds how far it can drift.
- The `SENSITIVE_ATTRIBUTES` denylist (`MEDICAL`, `LEGAL`, `POLITICS`,
  `RELATIONSHIPS_DATING`, `WELLNESS_ALTERNATIVE`) is NEVER perturbed: those
  categories are skipped entirely in descent, so their relative mass (their
  pairwise ratios) is preserved exactly through the final uniform rescale.
- After descent, `WeightNormalizer::normalize` applies the MIN_WEIGHT two-pass
  clamp-and-divide: pass 1 normalizes and clamps every entry up to `MIN_WEIGHT`
  (so no category is ever truly zero), pass 2 divides by the clamped sum. The
  divide is a UNIFORM scaling, which is why it preserves the unperturbed
  sensitive ratios; a clamped entry can dip a hair below `MIN_WEIGHT` after the
  rescale but stays STRICTLY POSITIVE (the contract is "no category ever zero",
  with the floor preserved up to the rescale).

The output covers EVERY `CategoryPool`, sums to ~1.0, never zeroes a category,
and is within the KL budget of the input. This is the signed WEIGHT MAP artifact.

## Step 2: the category-targeted, timed query plan

`plan::generate_query_plan(persona, weight_map, intensity, seed)` produces a
schedule of query INTENTS, the desktop-side analogue of the phone's on-device
generation. It reuses the two frozen models the studio week-simulator and the
C1 household scheduler use:

- TIMING: the circadian Poisson model (`-ln(1 - u) / rate`, activity only inside
  the 07:00-23:00 active window).
- CATEGORY SELECTION: the persona-following weight blend (`0.85 * w + 0.15 *
  0.6`, with `w = 2.0` for an interest else `0.3`), BIASED by multiplying in the
  signed weight map from step 1, so the allocator's protected-interest emphasis
  carries into the plan.

It is seed-deterministic (the persona id is mixed into the seed, exactly as the
week simulator does).

### Honesty: intents, not full query strings

The plan emits category-targeted INTENTS (`{ atSecs, category, intensity, query
}`) with `query = None`. We deliberately do NOT port the Android
`GrammarQueryGenerator` query banks + grammar, so we make NO claim of full E6
fidelity. The phone EXPANDS each intent into a concrete query string with its own
on-device generator on replay. Porting the E6 query banks + grammar to desktop
(to fill in `QueryIntent::query`) is a focused FOLLOW-UP; the wire field already
exists, so that follow-up needs no schema change.

## Step 3: signing (reusing the persona-pack key pattern)

Both artifacts are wrapped in a `SignedArtifact`: an `ArtifactContent` (schema
version, `generatedAt`, `expiresAt`, persona id, payload) plus the signer's
ed25519 public key and a signature over the content's CANONICAL bytes. Signing
reuses the persona-pack `PackSigningKey` pattern: a device artifact-signing key
whose 32-byte seed lives in the OS keystore (or the headless passphrase-file
fallback), built via `SigningKey::from_bytes`; signatures and keys travel as
STANDARD base64; verification uses `verify_strict`. `sign_artifact` /
`verify_artifact` are hermetic low-level primitives a test signs with a fixed
seed.

### Canonicalization caveat (f64)

The weight map carries `f64` weights, and `serde_json`'s float PARSER is not
always the exact inverse of its (ryu) SERIALIZER (parsing a serialized `f64` can
land one ULP away). So `ArtifactContent::canonical_bytes` STABILIZES the bytes
through one serialize -> parse -> serialize round trip, reaching the parser's
fixed point. The signer (which canonicalizes here) and the verifier (which
parses the artifact, then canonicalizes the parsed content) thus converge on
identical bytes, and the signature does not spuriously fail.

## Step 4: delivery over O1 + freshness fallback

A `SyncBody::SignedArtifact` wire kind carries the artifact over the existing
LanSync sealed channel to paired peers (see the `sync::wire` module).
The unknown-kind fail-closed posture is preserved: a peer that does not know the
kind rejects it at parse and falls back.

The phone's consumer logic is modeled by
`generate::select_artifact_or_fallback(bytes, now)` and, end to end through the
channel, `Core::receive_artifact_frame`:

- REPLAY only when the artifact VERIFIES (signature + integrity) AND is FRESH
  (`generatedAt <= now < expiresAt`).
- FALL BACK to on-device generation otherwise, with a typed reason: `Absent`
  (no artifact), `Invalid` (failed verification, fail closed), or `Stale` (past
  the freshness window). The default freshness window is 24 hours
  (`DEFAULT_FRESHNESS_MS`); the desktop regenerates and re-pushes well within it.

## Headless API

- `Core::generate_signed_artifacts(persona_id, intensity, seed, freshness_ms)`
  runs a pass and returns both signed artifacts WITHOUT pushing.
- `Core::run_generation_pass(...)` generates, signs, AND pushes both to every
  paired peer over the sealed channel; returns the artifacts and the peer count.
- `Core::push_signed_artifact(artifact)` pushes one out-of-band artifact.
- `Core::receive_artifact_frame(sender_key, frame, now)` opens + verifies a
  sealed frame and returns an `ArtifactDecision` (replay or fall back).

All of these require an open store (they fail closed otherwise) and touch no
network beyond the local sealed channel.

## Tests

Hermetic, fixed-seed, temp `EncryptedFile` store (never the OS keystore): the
allocator output sums to ~1.0, never zeroes a category, leaves the sensitive
categories' relative mass unperturbed, and stays within the 0.15-nat budget; a
known input is deterministic; the query plan is category-targeted, stays in the
active window, and biases toward the persona/weight-map categories; both
artifacts sign and verify, a tampered/wrong-key/future-version/malformed artifact
fails verification; the freshness check selects replay vs fallback correctly; the
`SignedArtifact` kind round-trips through the sealed channel; and an unknown kind
still fails closed at parse.
