<!--
  fauxx-desktop: Fauxx Desktop Companion
  Copyright (C) 2026 Digital Grease
  Licensed under the GNU Affero General Public License v3 or later.
  See the LICENSE file at the repository root.
-->

# Fauxx Decoy Companion: native-messaging contract

This document is the communication contract between the **WebExtension** (this
`extension/` directory) and the **headless Fauxx Core**. The extension never
talks to the Core directly: it talks to a small local **native-messaging host**
process that owns a `fauxx_core::Core` handle and translates between the wire
messages defined here and the Core async API.

The Rust-side native-messaging **host is implemented** as the `native-host`
subcommand of the `fauxx-cli` CLI (`apps/cli`); see
[`native-host/README.md`](./native-host/README.md) for installation. This
document and `src/protocol.js` define the exact wire schema the host implements.

## Transport

- Chromium / Firefox **native messaging**: the extension calls
  `runtime.connectNative("com.digital_grease.fauxx")` and exchanges newline-free
  JSON objects, each framed by the platform's 32-bit native-byte-order length
  prefix (the browser does the framing; both sides see whole JSON objects).
- The connection is **local-only**. The host runs on the same machine as the
  browser and binds to a local `Core`. There is no network endpoint and no
  remote fallback in the extension. The only thing that leaves the machine is
  the **decoy traffic itself** (public HTTPS GETs the decoy issues), with GPC
  set.
- Every message is a JSON object carrying:
  - `v`: integer schema version (currently `1`, `PROTOCOL_VERSION`).
  - `type`: string discriminator.

A version mismatch (`v !== 1`) is refused by the extension.

## Host registration

The host is registered with a native-messaging manifest named
`com.digital_grease.fauxx.json` in the per-browser location
(`~/.config/google-chrome/NativeMessagingHosts/`,
`~/.mozilla/native-messaging-hosts/`, the Windows registry, etc.).
Browser-specific templates are at
`native-host/com.digital_grease.fauxx.{chromium,firefox}.json.template`; fill in
the absolute `path` to the launcher wrapper and the extension id / gecko id under
`allowed_origins` / `allowed_extensions`. See
[`native-host/README.md`](./native-host/README.md) for the full install steps.

## Messages: host -> extension

The Core (via the host) hands the extension decoy work.

### `hello` (handshake)
```json
{ "v": 1, "type": "hello", "coreVersion": "0.1.0", "schemaVersion": 1 }
```
Sent on connect. The extension logs the Core version locally and clears any
prior connection error.

### `decoyPlan`
The authoritative decoy plan for one tick. **Intent is decoy-only by
construction**: `intent` MUST equal `"decoy"`; any other value is refused. The
extension still re-checks every resolved target against the guardrails.

```json
{
  "v": 1,
  "type": "decoyPlan",
  "planId": "uuid",
  "personaId": "persona-id",
  "intent": "decoy",
  "intensity": "Low | Medium | High | Extreme",
  "targetSegment": "TECHNOLOGY",
  "categories": ["TECHNOLOGY", "TRAVEL"],
  "targets": ["https://example.com/"],
  "mode": "fetch | visit",
  "gpc": true,
  "maxTargets": 6,
  "seed": 42
}
```

| Field | Meaning | Core source |
|---|---|---|
| `personaId` | Which synthetic persona this serves | `Core::get_persona` |
| `intensity` | `IntensityLevel` name | `fauxx_core::IntensityLevel` / `CampaignDirective::intensity` |
| `targetSegment` | `CategoryPool` name to bias toward | `CampaignDirective::target_segment` |
| `categories` | `CategoryPool` names to resolve to sites | `fauxx_core::browser::sites_for_categories` (host can resolve, or let the extension use its bundled table) |
| `targets` | Explicit HTTPS urls (override categories) | `sites_for_persona` / `sites_for_categories` |
| `mode` | `fetch` (background GET, default) or `visit` (opt-in tab) | n/a (extension-side) |
| `gpc` | Emit GPC on this plan's traffic (default `true`) | mirrors `BrowserLaunchConfig::gpc_enabled` (default ON) |
| `maxTargets` | Cap on sites touched this tick | scheduler budget |
| `seed` | Determinism hint, echoed back | mirrors the `seed` arg on `seed_history` |

### `checkGpc`
Ask the extension to fetch and parse a site's `/.well-known/gpc.json`.
```json
{ "v": 1, "type": "checkGpc", "origin": "https://example.com" }
```

### `stop`
Kill switch. The extension aborts any in-flight plan loop.
```json
{ "v": 1, "type": "stop" }
```

## Messages: extension -> host

The extension reports activity; the host folds it into the shared measurement
store via the Core API.

### `ready` (handshake reply)
```json
{ "v": 1, "type": "ready", "extensionVersion": "0.1.0", "schemaVersion": 1 }
```

### `requestPlan`
The extension PULLS a decoy plan from the host (the host is request-driven). The
host replies with a `decoyPlan`. The extension drives this on enable, on a
recurring `alarms` tick, and whenever the ephemeral service worker is respawned
while enabled. `personaId` is optional: when omitted the host plans for a default
persona (the first in its store), so the common single-persona case needs no
configuration; a multi-persona operator pins one.
```json
{ "v": 1, "type": "requestPlan", "personaId": "persona-id", "intensity": "Medium", "seed": 42, "maxTargets": 12 }
```

### `decoyReport`
Activity report for a completed plan. The `visited` / `skipped` shape mirrors
`fauxx_core::browser::SeedOutcome` exactly.
```json
{
  "v": 1,
  "type": "decoyReport",
  "planId": "uuid",
  "personaId": "persona-id",
  "seed": 42,
  "visited": ["https://www.theverge.com/"],
  "skipped": [{ "url": "https://...", "reason": "Failed to fetch" }],
  "startedAt": 1765600000000,
  "finishedAt": 1765600032000
}
```
**Host action:** record the in-browser decoy session into the measurement store.
This is the same `SeedOutcome` shape the native path produces from
`DecoyBrowser::seed_history`, so the host can persist it through the same
measurement plumbing (e.g. attribute the visited sites to the persona's shadow
profile / drift series the C4 `MeasurementEngine` consumes).

### `topicsReadback`
A Privacy Sandbox Topics read from a decoy tab. The `readback` field maps 1:1
onto `fauxx_core::browser::TopicsReadback` (and each topic onto
`AssignedTopic`), and `parse_topics_payload` accepts this exact shape.
```json
{
  "v": 1,
  "type": "topicsReadback",
  "personaId": "persona-id",
  "decoyId": "ext-decoy-default",
  "readback": { "available": true, "topics": [{ "topic": 57, "taxonomyVersion": "1" }] }
}
```
**Host action:** `Core::record_topics_readback(personaId, decoyId, &readback)`.

### `gpcStatus`
A parsed `/.well-known/gpc.json` observation. The `support` field maps onto
`fauxx_core::browser::GpcSupport`, and `parse_gpc_well_known` produces the same
shape.
```json
{
  "v": 1,
  "type": "gpcStatus",
  "origin": "https://example.com",
  "support": { "honored": true, "lastUpdate": "2022-06-01" }
}
```
**Host action:** `Core::record_gpc_status(origin, support)`.

### `error`
A non-fatal problem the extension surfaces locally.
```json
{ "v": 1, "type": "error", "context": "validateDecoyPlan", "message": "..." }
```

## Guardrails are enforced on BOTH sides

The extension enforces the same hard guardrails as the native decoy browser
(see `src/guardrails.js`, which mirrors
`crates/fauxx-core/src/browser/isolation.rs`):

- decoy-only: `intent` must be `"decoy"`; non-HTTPS targets are dropped;
- authenticated-account sign-in endpoints are blocklisted (the same
  `AUTH_FLOW_BLOCKLIST` entries) and refused, fail closed;
- no credential automation, no form submission, no ad-clicks / invalid traffic /
  fake conversions; the decoy only fetches (or visits) a public page;
- GPC (`Sec-GPC: 1`) is emitted by default.

The host SHOULD also validate plans it emits, but the extension does not trust
the host blindly: it re-checks every target. A guardrail-blocked target is a
recorded `skipped` entry in the report, never a silent failure.

## Mapping summary (extension report -> Core API)

| Extension message | Core method the host calls |
|---|---|
| `decoyReport` | measurement plumbing for an in-browser `SeedOutcome` (persist visited/skipped for the persona) |
| `topicsReadback` | `Core::record_topics_readback` |
| `gpcStatus` | `Core::record_gpc_status` |

The Core API surface referenced here is read-only context for the host
implementer; this lane builds **no Rust**.
