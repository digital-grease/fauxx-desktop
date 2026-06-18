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

// The communication contract between this WebExtension and the headless Fauxx
// Core, carried over a Chromium/Firefox NATIVE MESSAGING connection to a local
// host process (the Rust-side host is a documented FOLLOW-UP; see
// extension/PROTOCOL.md). This module defines the wire shapes and the
// validators, so the extension and the future host agree on one schema.
//
// Transport: connectNative("com.digital_grease.fauxx"). Every message is a JSON
// object with a `type` discriminator and a `v` schema version. All traffic is
// local-only (the native host binds to the Core on the same machine); no
// telemetry leaves the box other than the decoy traffic itself.

// The protocol schema version this extension speaks.
export const PROTOCOL_VERSION = 1;

// The native-messaging application name both sides register under. Must match
// the `name` field in the native-host manifest the follow-up host installs.
export const NATIVE_HOST_NAME = "com.digital_grease.fauxx";

// ---------------------------------------------------------------------------
// Host -> extension messages (the Core hands the extension decoy work)
// ---------------------------------------------------------------------------

// "hello": handshake the host sends on connect. Carries the host/Core version
// and the negotiated schema version so the extension can refuse a mismatch.
//   { v, type: "hello", coreVersion: string, schemaVersion: number }
//
// "decoyPlan": the authoritative decoy plan for a tick. INTENT IS DECOY-ONLY by
// construction; the extension still re-checks every target against the
// guardrails before acting. Shape:
//   {
//     v, type: "decoyPlan",
//     planId: string,            // opaque id the extension echoes in reports
//     personaId: string,         // which synthetic persona this serves
//     intent: "decoy",           // MUST be "decoy"; any other value is refused
//     intensity: "Low"|"Medium"|"High"|"Extreme",  // IntensityLevel name
//     targetSegment: string,     // optional CategoryPool name to bias toward
//     categories: string[],      // CategoryPool names to resolve to sites
//     targets: string[],         // optional explicit HTTPS urls (override sites)
//     mode: "fetch"|"visit",     // background fetch (default) or open-a-tab visit
//     gpc: boolean,              // emit GPC on this plan's traffic (default true)
//     maxTargets: number,        // cap on sites touched this tick
//     seed: number               // determinism hint, echoed in reports
//   }
//
// "checkGpc": ask the extension to read a site's /.well-known/gpc.json and
// report the parsed support. Shape: { v, type: "checkGpc", origin: string }.
//
// "stop": stop all in-flight decoy activity (kill switch). { v, type: "stop" }.

export const HOST_MESSAGE_TYPES = Object.freeze({
  HELLO: "hello",
  DECOY_PLAN: "decoyPlan",
  CHECK_GPC: "checkGpc",
  STOP: "stop",
});

// ---------------------------------------------------------------------------
// Extension -> host messages (the extension reports decoy activity)
// ---------------------------------------------------------------------------

// "ready": handshake reply on connect.
//   { v, type: "ready", extensionVersion: string, schemaVersion: number }
//
// "requestPlan": the extension PULLS a decoy plan from the host (the host is
// request-driven). The host replies with a "decoyPlan". `personaId` is optional:
// when omitted the host plans for a default persona (the first in its store), so
// the common single-persona case needs no configuration.
//   { v, type: "requestPlan", personaId?: string, intensity?: string,
//     seed?: number, maxTargets?: number }
//
// "decoyReport": activity report for a completed plan. Maps onto the Core's
// SeedOutcome (visited / skipped) so the host can fold it into the shared
// measurement store via Core.record_* and the seeding APIs.
//   {
//     v, type: "decoyReport",
//     planId: string, personaId: string, seed: number,
//     visited: string[],                 // urls that loaded/fetched ok
//     skipped: [{ url: string, reason: string }],  // url + local reason
//     startedAt: number, finishedAt: number        // epoch millis
//   }
//
// "topicsReadback": a Privacy Sandbox Topics read from a decoy tab. Maps 1:1
// onto fauxx_core::browser::TopicsReadback so the host can call
// Core.record_topics_readback(personaId, decoyId, readback).
//   {
//     v, type: "topicsReadback",
//     personaId: string, decoyId: string,
//     readback: { available: boolean,
//                 topics: [{ topic: number, taxonomyVersion?, modelVersion?,
//                            version?, configVersion?, name? }] }
//   }
//
// "gpcStatus": a parsed /.well-known/gpc.json observation. Maps onto
// fauxx_core::browser::GpcSupport so the host can call Core.record_gpc_status.
//   {
//     v, type: "gpcStatus",
//     origin: string,
//     support: { honored: boolean, lastUpdate?: string, version?: string }
//   }
//
// "error": a non-fatal problem the extension wants surfaced locally.
//   { v, type: "error", context: string, message: string }

export const EXT_MESSAGE_TYPES = Object.freeze({
  READY: "ready",
  REQUEST_PLAN: "requestPlan",
  DECOY_REPORT: "decoyReport",
  TOPICS_READBACK: "topicsReadback",
  GPC_STATUS: "gpcStatus",
  ERROR: "error",
});

// The only acceptable intent on a decoy plan. Any other value is refused: the
// extension exists exclusively to inject DECOY traffic. This mirrors the native
// path, which is likewise scoped to DECOY intent only.
export const REQUIRED_INTENT = "decoy";

// Valid IntensityLevel names (mirrors fauxx_core::IntensityLevel).
export const INTENSITY_LEVELS = Object.freeze([
  "Low",
  "Medium",
  "High",
  "Extreme",
]);

// Validate a host->extension decoy plan. Returns { ok: boolean, reason: string,
// plan?: normalized }. Fail closed: a plan that is not strictly decoy-intent,
// or whose targets cannot be resolved to eligible HTTPS sites, is rejected here
// before any traffic is generated. The caller re-checks each resolved target
// against the guardrails too (defense in depth).
export function validateDecoyPlan(msg) {
  if (!msg || typeof msg !== "object") {
    return { ok: false, reason: "plan is not an object" };
  }
  if (msg.type !== HOST_MESSAGE_TYPES.DECOY_PLAN) {
    return { ok: false, reason: "not a decoyPlan message" };
  }
  if (typeof msg.v !== "number" || msg.v !== PROTOCOL_VERSION) {
    return { ok: false, reason: "unsupported protocol version" };
  }
  // HARD GUARDRAIL: decoy intent only.
  if (msg.intent !== REQUIRED_INTENT) {
    return {
      ok: false,
      reason:
        "refusing plan: intent must be exactly '" +
        REQUIRED_INTENT +
        "' (this path is decoy-only)",
    };
  }
  if (typeof msg.planId !== "string" || msg.planId.length === 0) {
    return { ok: false, reason: "plan missing planId" };
  }
  const intensity = msg.intensity || "Low";
  if (!INTENSITY_LEVELS.includes(intensity)) {
    return { ok: false, reason: "unknown intensity: " + intensity };
  }
  const mode = msg.mode === "visit" ? "visit" : "fetch";
  const gpc = msg.gpc !== false; // default ON
  const categories = Array.isArray(msg.categories) ? msg.categories : [];
  const targets = Array.isArray(msg.targets) ? msg.targets : [];
  const maxTargets =
    Number.isInteger(msg.maxTargets) && msg.maxTargets > 0
      ? msg.maxTargets
      : 12;

  return {
    ok: true,
    reason: "",
    plan: {
      planId: msg.planId,
      personaId: typeof msg.personaId === "string" ? msg.personaId : "",
      intent: REQUIRED_INTENT,
      intensity,
      targetSegment:
        typeof msg.targetSegment === "string" ? msg.targetSegment : "",
      categories,
      targets,
      mode,
      gpc,
      maxTargets,
      seed: Number.isFinite(msg.seed) ? msg.seed : 0,
    },
  };
}

// Build a wire-shaped extension->host message of `type`, stamping the schema
// version. Pure helper so every outbound message carries `v`.
export function envelope(type, fields) {
  return Object.assign({ v: PROTOCOL_VERSION, type }, fields || {});
}
