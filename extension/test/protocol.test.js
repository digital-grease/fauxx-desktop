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

// Unit tests for the native-messaging protocol module. These pin the wire
// constants, the message envelope, and the fail-closed decoy-plan validator so
// a regression (wrong host name, bumped version, or a relaxed plan check that
// accepts a non-decoy intent) fails CI. The module is pure with no browser
// globals at import time, so no stubbing is needed.

import test from "node:test";
import assert from "node:assert/strict";

import {
  PROTOCOL_VERSION,
  NATIVE_HOST_NAME,
  HOST_MESSAGE_TYPES,
  EXT_MESSAGE_TYPES,
  REQUIRED_INTENT,
  INTENSITY_LEVELS,
  validateDecoyPlan,
  envelope,
} from "../src/protocol.js";

test("protocol constants have their pinned wire values", () => {
  assert.equal(PROTOCOL_VERSION, 1);
  assert.equal(NATIVE_HOST_NAME, "com.digital_grease.fauxx");
  assert.equal(REQUIRED_INTENT, "decoy");
  assert.deepEqual(INTENSITY_LEVELS, ["Low", "Medium", "High", "Extreme"]);
});

test("message-type maps carry the documented discriminators", () => {
  assert.equal(HOST_MESSAGE_TYPES.DECOY_PLAN, "decoyPlan");
  assert.equal(HOST_MESSAGE_TYPES.HELLO, "hello");
  assert.equal(HOST_MESSAGE_TYPES.CHECK_GPC, "checkGpc");
  assert.equal(HOST_MESSAGE_TYPES.STOP, "stop");
  assert.equal(EXT_MESSAGE_TYPES.READY, "ready");
  assert.equal(EXT_MESSAGE_TYPES.REQUEST_PLAN, "requestPlan");
  assert.equal(EXT_MESSAGE_TYPES.DECOY_REPORT, "decoyReport");
  assert.equal(EXT_MESSAGE_TYPES.TOPICS_READBACK, "topicsReadback");
  assert.equal(EXT_MESSAGE_TYPES.GPC_STATUS, "gpcStatus");
  assert.equal(EXT_MESSAGE_TYPES.ERROR, "error");
});

test("message-type maps are frozen (cannot be mutated at runtime)", () => {
  assert.ok(Object.isFrozen(HOST_MESSAGE_TYPES));
  assert.ok(Object.isFrozen(EXT_MESSAGE_TYPES));
  assert.ok(Object.isFrozen(INTENSITY_LEVELS));
});

test("envelope stamps the schema version and merges fields", () => {
  const msg = envelope(EXT_MESSAGE_TYPES.REQUEST_PLAN, {
    personaId: "persona-1",
    maxTargets: 5,
  });
  assert.deepEqual(msg, {
    v: PROTOCOL_VERSION,
    type: "requestPlan",
    personaId: "persona-1",
    maxTargets: 5,
  });
});

test("envelope works with no fields and round-trips through JSON", () => {
  const msg = envelope(EXT_MESSAGE_TYPES.READY);
  assert.deepEqual(msg, { v: 1, type: "ready" });
  // JSON round-trip is the native-messaging serialization the host receives.
  const decoded = JSON.parse(JSON.stringify(msg));
  assert.deepEqual(decoded, msg);
});

test("validateDecoyPlan ACCEPTS a well-formed decoy plan and normalizes it", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    planId: "plan-123",
    personaId: "persona-1",
    intent: REQUIRED_INTENT,
    intensity: "Medium",
    categories: ["news", "shopping"],
    targets: ["https://example.com/"],
    mode: "visit",
    gpc: false,
    maxTargets: 7,
    seed: 99,
  });
  assert.equal(result.ok, true);
  assert.equal(result.reason, "");
  assert.equal(result.plan.planId, "plan-123");
  assert.equal(result.plan.intent, "decoy");
  assert.equal(result.plan.intensity, "Medium");
  assert.equal(result.plan.mode, "visit");
  assert.equal(result.plan.gpc, false);
  assert.equal(result.plan.maxTargets, 7);
  assert.equal(result.plan.seed, 99);
  assert.deepEqual(result.plan.categories, ["news", "shopping"]);
  assert.deepEqual(result.plan.targets, ["https://example.com/"]);
});

test("validateDecoyPlan applies safe defaults for omitted fields", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    planId: "plan-defaults",
    intent: REQUIRED_INTENT,
  });
  assert.equal(result.ok, true);
  assert.equal(result.plan.intensity, "Low");
  assert.equal(result.plan.mode, "fetch");
  assert.equal(result.plan.gpc, true); // default ON
  assert.equal(result.plan.maxTargets, 12);
  assert.equal(result.plan.seed, 0);
  assert.deepEqual(result.plan.categories, []);
  assert.deepEqual(result.plan.targets, []);
});

test("validateDecoyPlan REJECTS a non-decoy intent (hard guardrail)", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    planId: "plan-evil",
    intent: "click_ads",
  });
  assert.equal(result.ok, false);
  assert.match(result.reason, /intent must be exactly 'decoy'/);
});

test("validateDecoyPlan REJECTS an unsupported protocol version", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION + 1,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    planId: "plan-x",
    intent: REQUIRED_INTENT,
  });
  assert.equal(result.ok, false);
  assert.match(result.reason, /unsupported protocol version/);
});

test("validateDecoyPlan REJECTS the wrong message type", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.STOP,
    planId: "plan-x",
    intent: REQUIRED_INTENT,
  });
  assert.equal(result.ok, false);
  assert.match(result.reason, /not a decoyPlan/);
});

test("validateDecoyPlan REJECTS a missing planId and a non-object", () => {
  const missingId = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    intent: REQUIRED_INTENT,
  });
  assert.equal(missingId.ok, false);
  assert.match(missingId.reason, /planId/);

  assert.equal(validateDecoyPlan(null).ok, false);
  assert.equal(validateDecoyPlan("nope").ok, false);
});

test("validateDecoyPlan REJECTS an unknown intensity", () => {
  const result = validateDecoyPlan({
    v: PROTOCOL_VERSION,
    type: HOST_MESSAGE_TYPES.DECOY_PLAN,
    planId: "plan-x",
    intent: REQUIRED_INTENT,
    intensity: "Ludicrous",
  });
  assert.equal(result.ok, false);
  assert.match(result.reason, /unknown intensity/);
});
