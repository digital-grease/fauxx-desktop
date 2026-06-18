# C4 measurement and analytics (the measurement core)

This note documents the measurement core the dashboard renders: the efficacy
metric COMPUTATION (#20, A1), the control-profile A/B comparison at scale (#21,
A2), the broker diff view computation (#22, A3), and the efficacy-snapshot
export (#23, A4). All of it lives in `fauxx-core` behind the clean async `Core`
API; the Iced GUI rendering of these series is a SEPARATE later batch, so there
is no charting or GUI code here. Everything is 100% local (no network, no
telemetry), reachable headless (CLI, and later MQTT), and degrades gracefully on
empty data (no panic, no divide-by-zero, no `NaN`/`inf`).

The measurement module is `crate::measurement`, with submodules `distribution`,
`platform`, `stats`, `shadow`, and `export`, orchestrated by `MeasurementEngine`
over the same encrypted store the rest of the core uses. The broker diff view
(A3) lives beside the broker code in `crate::brokers::scan`, since it diffs the
broker scan snapshots.

## A1: the efficacy-metric computation (#20)

### Category distributions (`measurement::distribution`)

A `CategoryDistribution` is a tally of how often each category label was observed
at one point in time. Category labels are opaque strings, so the same machinery
serves every platform. Counts are `f64` so weighted baselines and integer tallies
share one type; non-finite or negative counts are dropped so the tally stays
valid. An empty distribution is well-formed.

### The drift metric: KL(baseline || observed)

Profile drift is the Kullback-Leibler divergence FROM the baseline `p` TO the
observed distribution `q`:

```text
D_KL(p || q) = sum over categories c of  p(c) * ln( p(c) / q(c) )
```

It is `0` exactly when `p == q` (no drift) and grows as the observed picture
diverges from the baseline. We measure `KL(baseline || observed)` so the baseline
is the reference the observed profile is scored against, with each term weighted
by the baseline mass `p(c)` ("how surprised is the baseline model to see the
observed profile"). The sign convention and direction are frozen so every
platform series and every A/B cohort is directly comparable.

### Smoothing (no infinities, no NaN)

Raw KL is undefined when a category has zero probability under one distribution
but not the other (`ln(p/0)` is `+inf`). Observed profiles routinely have zero
mass on categories the baseline cares about and vice versa, so we apply additive
(Laplace) smoothing with a small `epsilon` pseudo-count (default `0.5`) to BOTH
distributions over the UNION of their category supports before normalizing. With
a positive epsilon every category has positive probability under both `p` and
`q`, so every term is finite and the sum is a well-defined, non-negative real
number. A misused zero/negative/`NaN` epsilon is floored to a tiny positive
constant, and the `0 * ln(0/q) = 0` convention is applied as a defensive guard,
so the metric can never reintroduce an infinity.

Note a mathematical consequence: when the union support is a SINGLE category,
both smoothed distributions become the same point mass on it, so the divergence
is `0`. Drift only appears once the support spans more than one category.

### Per-category contributions (the heatmap)

`kl_divergence_breakdown` returns the scalar `total` AND each category's term
`p(c) * ln(p(c) / q(c))` (`CategoryContribution`). The per-category contributions
SUM (up to floating-point rounding) to the total, which is what makes the
per-category heatmap (`HeatmapSeries`) and the scalar timeline (`DriftSeries`)
two views of the same number. An individual category's contribution may be
negative (when `q(c) > p(c)`); the SUM is the non-negative total.

### Platforms (`measurement::platform`)

A `Platform` is one tracked profiling surface, and the set is EXTENSIBLE via
`Platform::Other(label)`:

- `Google`: driven by the R2 Privacy Sandbox Topics read-backs
  (`TopicsMeasurement`). Each read-back is one timestamped distribution; every
  assigned topic is one observation, labeled by its human-readable `name` when
  the browser reported one, else by `topic:<id>`.
- `Brokers`: driven by the D1c broker scan/submission history
  (`BrokerSubmission`). The category is the broker id; the distribution at a
  timestamp is the CUMULATIVE set of brokers the persona is currently listed on,
  so an opt-out that reaches `removed` visibly shrinks the picture.
- `Meta`: no desktop data source yet, so it yields an EMPTY (no-data) series
  gracefully rather than failing.

### Baseline

`Baseline` documents the reference distribution `p`:

- `PersonaIntent` (preferred): the persona's declared interests become the
  reference, each weighted by the aligned topic weight
  (`constants::ALIGNED_WEIGHT`). Drift then measures distance from the persona's
  INTENT. Built via `Baseline::from_persona`.
- `FirstSnapshot`: the first observed snapshot; drift measures movement away from
  the starting picture. Used when no persona intent is available.
- `Explicit`: a caller-supplied distribution.

A baseline that cannot be resolved (empty persona intent, no first snapshot)
degrades to an all-zero-drift series over the snapshot timestamps, so the
timeline still renders rather than vanishing.

### The device dimension

`aggregate_devices` merges multiple devices' snapshots for the same platform into
one combined, time-ordered stream; snapshots sharing a timestamp are merged by
summing their category counts. It DEGRADES to single-device data (one device
passes through unchanged) and to no-data (an empty input yields an empty stream)
without panicking. `MeasurementEngine::combined_platform_drift` exposes this over
the async API.

## A2: control-profile A/B at scale (#21)

### Shadow profiles (`measurement::shadow`)

A `ShadowProfile` is one experimental arm: an `Arm` tag (`Treated` noised, or
untreated `Control`) bound to its OWN persona, so the arms run independently and
their drift metrics are tracked separately. Definitions persist in the new
`shadow_profiles` table (schema v9, migrated forward via the `user_version`
pattern) and round-trip as their exact JSON.

### Statistics (`measurement::stats`)

The comparison reuses the A1 drift metric: each profile contributes a SAMPLE of
drift values (one per snapshot), so the A/B numbers are in the dashboard's own
units. Across the treated and control cohorts we compute:

- EFFECT SIZE: Cohen's `d`, the difference in means in pooled standard-deviation
  units. Plain `f64` math.
- SIGNIFICANCE: a two-sample t-test p-value. The statistic and degrees of freedom
  are plain math; the two-sided p-value is read off the Student-t CDF from
  `statrs` as `2 * (1 - CDF(|t|; df))`, clamped to `[0, 1]`.

Welch vs pooled: the default is WELCH's unequal-variance t-test
(`TTestKind::Welch`), which does not assume the cohorts share a variance, the
safer default for a treated-vs-control comparison. The pooled (Student) variant
(`TTestKind::Pooled`) assumes equal variances and pools them with `n1 + n2 - 2`
degrees of freedom; Welch uses the Welch-Satterthwaite fractional degrees of
freedom.

Degenerate samples are guarded WITHOUT panicking: a sample with `n < 2`, or both
variances zero (no spread), yields the conservative result, effect size `0.0` and
the significance `p = 1` with `well_defined = false`. A `p ~= 1` is expected for
identical cohorts; well-separated cohorts give a small `p`.

### The plainly-readable comparison

`compare_cohorts` (and `MeasurementEngine::compare_shadow_cohorts` over the store)
returns a `CohortComparison` that carries the raw numbers for the chart AND
human-facing fields a non-statistician can read: which arm drifted more
(`direction`), the plain-words effect magnitude
(`negligible`/`small`/`medium`/`large`), a confidence statement derived from the
p-value, and a one-line `summary`.

## The async API surface (on `Core`)

- `platform_drift`, `all_platform_drift`, `combined_platform_drift` (A1 series +
  heatmap; the device dimension).
- `save_shadow_profile`, `list_shadow_profiles`, `get_shadow_profile`,
  `delete_shadow_profile`, `compare_shadow_cohorts` (A2).
- `record_broker_scan_snapshot`, `list_broker_scan_snapshots`,
  `broker_diff_timeline` (A3).
- `efficacy_snapshot_data`, `export_efficacy_snapshot` (A4).

No GUI/CLI types cross this boundary; the GUI rendering of these series lands in a
later batch.

## A3: the broker diff view computation (#22)

The C3 D1c re-listing seam (`ListingCheck`) only answers a single boolean ("is
this persona still listed"). The A3 broker diff view needs something richer to
DIFF over time, so it introduces a `BrokerScanSnapshot` (in
`crate::brokers::scan`): per `(broker, persona)` at a point in time, the SET of
identity fields/records the broker exposes about that persona. Snapshots persist
in the new `broker_scan_snapshots` table (schema v10, migrated FORWARD via the
`user_version` pattern) and round-trip as their exact JSON. The exposed fields
are opaque, deduplicated, ordered strings, so any future field shape is a
first-class set member without a schema change.

The live scanning that POPULATES a snapshot from a broker site is DEFERRED,
exactly like the C3 live `ListingCheck`; A3 computes diffs from STORED snapshots
only (no scraping).

`compute_broker_diff_timeline` builds a time-ordered diff: snapshots are sorted
oldest first, then each consecutive pair is diffed over the union of their
fields, classifying every field `added` / `removed` / `unchanged`. RE-LISTING is
flagged distinctly: a field REMOVED in an earlier diff that later REAPPEARS is
marked `relisted` rather than a plain `added`, tying back to the C3 re-listing
motivation (the case opt-out tracking most needs to surface). A broker with zero
or one snapshot yields the clear `no_diff_yet` state (empty diffs), never a
panic.

## A4: the efficacy-snapshot export (#23)

`crate::measurement::export` exports the A1 efficacy data (the per-platform KL
drift time-series + per-category drift) to three formats, each EMBEDDING the
as-of date (via the `time` crate, formatted ISO `YYYY-MM-DD`):

- CSV (`csv` crate): a self-describing table of the underlying time-series and
  per-category drift rows. The frozen header is
  `as_of_date,platform,timestamp,kind,category,value`; `kind` is `drift` for a
  scalar timeline point or `category` for a per-category contribution. Every row
  carries the as-of date.
- JSON (`serde_json`): the same underlying `EfficacySnapshotData`, structured;
  it round-trips back into the type.
- PDF (`printpdf`, BUILT-IN Helvetica, no bundled TTF): a human-readable dated
  snapshot, a title, the as-of date, and a text/table summary of the drift per
  platform plus the top per-category movers. Embedded chart IMAGES are out of
  scope (future work); the text/table summary is the deliverable. The built-in
  standard fonts path works via `Op::SetFont { font: PdfFontHandle::Builtin(...)
  }`, so no font is bundled and no dependency is added.

### The signing seam (deliberately not implemented)

Export is a TWO-STEP pipeline by design: `export_efficacy_snapshot` produces an
in-memory `ExportArtifact` (the serialized `bytes` plus typed `ExportMetadata`:
format, content type, as-of millis/date, suggested filename), and a SEPARATE
`ExportArtifact::write_to` writes it. That split is the clean seam a future
ed25519 signing layer slots into: it can take the produced artifact, sign the
`bytes`, and wrap the output without reworking the producers. No signing,
hashing, or timestamping is done now.
