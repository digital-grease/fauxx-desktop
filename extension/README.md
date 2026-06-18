<!--
  fauxx-desktop: Fauxx Desktop Companion
  Copyright (C) 2026 Digital Grease
  Licensed under the GNU Affero General Public License v3 or later.
  See the LICENSE file at the repository root.
-->

# Fauxx Decoy Companion (WebExtension)

A lightweight, **opt-in**, **secondary** Manifest V3 WebExtension (Chromium and
Firefox) that performs **in-browser decoy injection** as a lower-friction
alternative to the desktop app's full automated decoy browser.

This is the **C2 #14 R4** companion. It is intentionally optional and clearly
secondary to the automated path: the desktop app's native decoy browser
(`crates/fauxx-core/src/browser`, an isolated Chromium profile driven over CDP)
is the primary, stronger mechanism. The extension exists for users who want some
persona-aligned decoy traffic without running the separate browser, accepting a
weaker isolation guarantee (it runs in your normal browser profile).

> **Not a Cargo crate.** This directory is a standalone WebExtension. It is not a
> member of the Cargo workspace and contains no Rust, so it can never affect any
> Rust build.

## What it does

- Receives **decoy plans** from the headless Fauxx Core over a local
  native-messaging connection (the Rust host is the `fauxx-cli native-host`
  subcommand; see [`native-host/README.md`](./native-host/README.md)), resolves
  them to a curated set of reputable HTTPS sites, and issues background fetches
  (or, opt-in, opens a tab) to build persona-category-aligned signal.
- Emits **Global Privacy Control** (`Sec-GPC: 1`) on outbound requests via a
  `declarativeNetRequest` ruleset (`src/gpc_rules.json`) plus a direct request
  header, mirroring the native path's default-on GPC.
- Reports activity back to the Core for the shared measurement store (visited /
  skipped sites, Topics read-backs, GPC observations). See
  [`PROTOCOL.md`](./PROTOCOL.md).
- Can run **standalone** (no Core connected) using its bundled
  category->site table, so you can drive a decoy session entirely offline.

## Hard guardrails (the same as the native path)

These are non-negotiable invariants, enforced in `src/guardrails.js`, which
mirrors `crates/fauxx-core/src/browser/isolation.rs` entry-for-entry:

1. **Decoy intent only.** A plan whose `intent` is not exactly `"decoy"` is
   refused. The extension exists solely to inject decoy traffic.
2. **Never touch authenticated sign-in flows.** A blocklist of sign-in
   endpoints (Google / YouTube accounts, Microsoft / Live login, Facebook
   `/login`, Apple ID) is refused, fail closed. No credential automation, ever.
3. **HTTPS public pages only.** Non-HTTPS targets are dropped before any
   request.
4. **No invalid traffic.** No ad-clicks, no fake conversions, no form
   submission. The decoy only performs `GET` requests with credentials omitted,
   or (opt-in) opens a tab to a public page.
5. **Emit GPC.** `Sec-GPC: 1` rides every decoy request by default.
6. **Local-only.** The single outbound connection is `connectNative` to the
   local host. There is no analytics and no remote endpoint. The only thing that
   leaves the machine is the decoy traffic itself.

A target that fails a guardrail is recorded as a `skipped` entry in the activity
report (with a local-only reason), never silently dropped and never retried into
a sign-in flow.

## Opt-in posture

- The extension ships **disabled**. On install, `enabled` defaults to `false`
  and nothing connects or runs.
- You turn it on from the toolbar **popup** (the toggle). Only then does the
  background worker call `connectNative` and act on plans.
- Turning it off disconnects from the host and stops all decoy activity. A Core
  `stop` message is an additional kill switch.
- **GPC trade-off.** Because this extension runs in your NORMAL browser profile
  (unlike the native path's isolated decoy profile), its `Sec-GPC: 1`
  declarativeNetRequest rule (`urlFilter: "*"`) sets Global Privacy Control on
  ALL of your own browsing while the companion is enabled, not just on decoy
  requests. This is intentional (GPC is a lawful opt-out you generally want), but
  it is a side effect to be aware of: disable the companion to stop emitting it.

## Install (developer / unpacked)

There is no build step. The extension is plain JS/JSON/HTML/CSS.

### Chromium (Chrome, Edge, Brave, ...)
1. Go to `chrome://extensions`.
2. Enable **Developer mode** (top right).
3. Click **Load unpacked** and select this `extension/` directory.
4. Note the generated extension id (you need it for the native host manifest).
5. Click the toolbar icon and flip the toggle to opt in.

### Firefox
1. Go to `about:debugging#/runtime/this-firefox`.
2. Click **Load Temporary Add-on...** and select `extension/manifest.json`.
3. Open the popup from the toolbar and flip the toggle to opt in.

> Icons referenced in the manifest (`icons/icon-48.png`, `icons/icon-128.png`)
> are placeholders you can drop in; the extension loads and runs without them
> (the browser falls back to a default action icon).

## The native-messaging host

The extension speaks to the Core through a small local **native-messaging
host** process that owns a `fauxx_core::Core` handle. That host is implemented
as the **`fauxx-cli native-host`** subcommand of the CLI (`apps/cli`); install and
registration steps are in [`native-host/README.md`](./native-host/README.md).

The wire contract it implements is specified in [`PROTOCOL.md`](./PROTOCOL.md)
and `src/protocol.js`:
- register the native-messaging manifest `com.digital_grease.fauxx` (templates
  in `native-host/com.digital_grease.fauxx.{chromium,firefox}.json.template`),
- send `hello` + `decoyPlan` / `checkGpc` / `stop`,
- receive `decoyReport` / `topicsReadback` / `gpcStatus` and persist them via
  `Core::record_topics_readback`, `Core::record_gpc_status`, and the
  measurement plumbing for an in-browser `SeedOutcome`.

A concrete end-to-end exchange is in `native-host/sample-exchange.json`.

Until the host's manifest is installed, the extension still works standalone: it
logs that the host is unavailable (visible in the popup) and uses its bundled
site table when you opt in. No traffic is generated unless you opt in.

## Files

| Path | Role |
|---|---|
| `manifest.json` | MV3 manifest (Chromium + Firefox via `browser_specific_settings`). |
| `src/background.js` | Service worker: opt-in gate, native-host bridge, plan dispatch. |
| `src/protocol.js` | Wire schema + validators for the Core contract. |
| `src/guardrails.js` | The hard guardrails (mirrors the Rust `isolation.rs`). |
| `src/decoy.js` | The decoy executor + GPC well-known + Topics read JS. |
| `src/category_sites.js` | Bundled category->HTTPS-site table (mirrors the core JSON). |
| `src/gpc_rules.json` | `declarativeNetRequest` ruleset that sets `Sec-GPC: 1`. |
| `popup/` | The opt-in control surface (popup + options page). |
| `native-host/` | Manifest templates, launcher wrapper, install README, and sample exchange for the `fauxx-cli native-host` host. |
| `PROTOCOL.md` | The full native-messaging contract. |

## License

AGPL-3.0-or-later, same as the rest of the repository. See the root `LICENSE`.
