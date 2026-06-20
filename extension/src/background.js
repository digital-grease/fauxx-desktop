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

// Background service worker (MV3): the opt-in gate and the bridge between the
// native Fauxx Core host and the in-browser decoy executor.
//
// Posture, enforced here:
//   - STRICTLY OPT-IN: nothing connects or runs until the operator flips
//     `enabled` in storage from the popup. On install we default to OFF.
//   - SECONDARY: this is the lower-friction alternative to the native decoy
//     browser; it does no business logic of its own. All plans come from the
//     Core via the native host; the extension only resolves+guards them and
//     reports activity back.
//   - LOCAL-ONLY: the sole outbound connection is connectNative() to the local
//     host. No analytics, no remote endpoints. The only thing that leaves the
//     machine is the decoy traffic itself (public HTTPS GETs), with GPC set.

// Use the WebExtension `browser` namespace if present (Firefox / polyfilled),
// else fall back to `chrome` (Chromium). Both expose the same MV3 surface here.
const ext = typeof browser !== "undefined" ? browser : chrome;

import {
  PROTOCOL_VERSION,
  NATIVE_HOST_NAME,
  HOST_MESSAGE_TYPES,
  EXT_MESSAGE_TYPES,
  envelope,
  validateDecoyPlan,
} from "./protocol.js";
import { runDecoyPlan, parseGpcWellKnown, wellKnownUrlFor } from "./decoy.js";
import { ensureDecoyAllowed } from "./guardrails.js";

const STORAGE_KEY = "fauxxDecoyState";

// The periodic plan-pull (C2 #14). The Core host is REQUEST-DRIVEN, so the
// extension must ASK for a plan; it does so on enable, on each alarm tick, and
// whenever the ephemeral worker is respawned while enabled. Chromium clamps
// alarm periods to >= 1 minute.
const PLAN_ALARM = "fauxxDecoyPlanTick";
const PLAN_PERIOD_MINUTES = 30;

// In-memory connection handle to the native host. Recreated on demand; service
// workers are ephemeral, so we never assume this survives a restart.
let nativePort = null;
// A simple flag so a "stop" command can short-circuit an in-flight plan loop.
let stopped = false;

// Read the persisted opt-in state. Defaults to disabled (opt-in posture).
async function getState() {
  const stored = await ext.storage.local.get(STORAGE_KEY);
  const s = stored[STORAGE_KEY] || {};
  return {
    enabled: s.enabled === true,
    lastReport: s.lastReport || null,
    lastError: s.lastError || null,
    connected: nativePort !== null,
  };
}

async function patchState(patch) {
  const stored = await ext.storage.local.get(STORAGE_KEY);
  const next = Object.assign({}, stored[STORAGE_KEY] || {}, patch);
  await ext.storage.local.set({ [STORAGE_KEY]: next });
  return next;
}

// Send an envelope to the native host if connected; otherwise drop it (the host
// is the only sink, and there is intentionally no remote fallback).
function sendToHost(type, fields) {
  if (!nativePort) {
    return;
  }
  try {
    nativePort.postMessage(envelope(type, fields));
  } catch (e) {
    console.warn("[fauxx-decoy] failed to post to native host:", e);
  }
}

// Connect to the local native-messaging host (the Core bridge). The host is the
// `fauxx-cli native-host` subcommand; until its native-messaging MANIFEST is
// installed (see native-host/README.md), connectNative throws and we record a
// friendly local error. Strictly opt-in: only called when enabled.
function connectNative() {
  if (nativePort) {
    return nativePort;
  }
  try {
    nativePort = ext.runtime.connectNative(NATIVE_HOST_NAME);
  } catch (e) {
    nativePort = null;
    patchState({
      lastError:
        "native host '" +
        NATIVE_HOST_NAME +
        "' not available (install its manifest; see native-host/README.md): " +
        String(e.message || e),
    });
    return null;
  }

  nativePort.onMessage.addListener(handleHostMessage);
  nativePort.onDisconnect.addListener(() => {
    const err = ext.runtime.lastError;
    console.info("[fauxx-decoy] native host disconnected", err || "");
    nativePort = null;
  });

  // Announce ourselves; the host replies with "hello".
  sendToHost(EXT_MESSAGE_TYPES.READY, {
    extensionVersion: ext.runtime.getManifest().version,
    schemaVersion: PROTOCOL_VERSION,
  });
  return nativePort;
}

function disconnectNative() {
  stopped = true;
  if (nativePort) {
    try {
      nativePort.disconnect();
    } catch {
      // best-effort
    }
    nativePort = null;
  }
}

// ---------------------------------------------------------------------------
// The plan-pull loop (C2 #14). Without this nothing ever requests work, so no
// decoy traffic flows even when connected. The host is request-driven: the
// extension pulls a plan, the host replies with a `decoyPlan` (handled above).
// ---------------------------------------------------------------------------

// Pull one decoy plan from the host. `personaId` is omitted by default so the
// host plans for its default persona (the single-persona case needs no config);
// a multi-persona operator can pin one in storage as `personaId`.
async function requestPlan() {
  const { enabled } = await getState();
  if (!enabled) {
    return;
  }
  if (!nativePort && !connectNative()) {
    return;
  }
  const stored = await ext.storage.local.get(STORAGE_KEY);
  const personaId = (stored[STORAGE_KEY] || {}).personaId;
  const fields = {};
  if (typeof personaId === "string" && personaId.length > 0) {
    fields.personaId = personaId;
  }
  sendToHost(EXT_MESSAGE_TYPES.REQUEST_PLAN, fields);
}

// Bring the loop up: register the recurring alarm and pull a plan now.
// Idempotent (alarms.create replaces any existing alarm of the same name).
async function startLoop() {
  ext.alarms.create(PLAN_ALARM, { periodInMinutes: PLAN_PERIOD_MINUTES });
  if (connectNative()) {
    await requestPlan();
  }
}

// Tear the loop down: stop the alarm and disconnect from the host.
function stopLoop() {
  ext.alarms.clear(PLAN_ALARM);
  disconnectNative();
}

// The recurring driver: each alarm tick pulls a fresh plan, reconnecting the
// ephemeral worker to the host if it was torn down since the last tick.
ext.alarms.onAlarm.addListener((alarm) => {
  if (alarm && alarm.name === PLAN_ALARM) {
    requestPlan();
  }
});

// Dispatch a message FROM the native host (the Core).
async function handleHostMessage(msg) {
  if (!msg || typeof msg !== "object") {
    return;
  }
  switch (msg.type) {
    case HOST_MESSAGE_TYPES.HELLO:
      console.info(
        "[fauxx-decoy] connected to Core",
        msg.coreVersion || "(unknown version)",
      );
      await patchState({ lastError: null });
      break;

    case HOST_MESSAGE_TYPES.DECOY_PLAN:
      await onDecoyPlan(msg);
      break;

    case HOST_MESSAGE_TYPES.CHECK_GPC:
      await onCheckGpc(msg);
      break;

    case HOST_MESSAGE_TYPES.STOP:
      stopped = true;
      console.info("[fauxx-decoy] received stop from Core");
      break;

    default:
      console.warn("[fauxx-decoy] unknown host message type:", msg.type);
  }
}

// Validate, run, and report one decoy plan.
async function onDecoyPlan(msg) {
  const { enabled } = await getState();
  if (!enabled) {
    // Defense in depth: never act on a plan while disabled.
    return;
  }
  const result = validateDecoyPlan(msg);
  if (!result.ok) {
    console.warn("[fauxx-decoy] rejected decoy plan:", result.reason);
    sendToHost(EXT_MESSAGE_TYPES.ERROR, {
      context: "validateDecoyPlan",
      message: result.reason,
    });
    return;
  }

  stopped = false;
  let report;
  try {
    report = await runDecoyPlan(result.plan, (url, opts) =>
      stopped ? Promise.reject(new Error("stopped")) : fetch(url, opts),
    );
  } catch (e) {
    sendToHost(EXT_MESSAGE_TYPES.ERROR, {
      context: "runDecoyPlan",
      message: String(e.message || e),
    });
    return;
  }

  await patchState({
    lastReport: {
      planId: report.planId,
      visited: report.visited.length,
      skipped: report.skipped.length,
      finishedAt: report.finishedAt,
    },
  });
  sendToHost(EXT_MESSAGE_TYPES.DECOY_REPORT, report);
}

// Fetch and parse a site's /.well-known/gpc.json, then report the observation.
async function onCheckGpc(msg) {
  const { enabled } = await getState();
  if (!enabled) {
    return;
  }
  const origin = typeof msg.origin === "string" ? msg.origin : "";
  const url = wellKnownUrlFor(origin);
  // The well-known probe must clear the same guardrail navigation does.
  try {
    ensureDecoyAllowed(url);
  } catch (e) {
    sendToHost(EXT_MESSAGE_TYPES.ERROR, {
      context: "checkGpc",
      message: String(e.message || e),
    });
    return;
  }

  let body = null;
  try {
    const resp = await fetch(url, {
      method: "GET",
      credentials: "omit",
      cache: "no-store",
      headers: { "Sec-GPC": "1" },
    });
    if (resp.ok) {
      body = (await resp.text()).slice(0, 8192);
    }
  } catch {
    body = null; // a missing/failed probe is "not advertised", not an error
  }

  const support = parseGpcWellKnown(body);
  sendToHost(EXT_MESSAGE_TYPES.GPC_STATUS, { origin, support });
}

// ---------------------------------------------------------------------------
// Popup <-> worker control messages (enable/disable, status)
// ---------------------------------------------------------------------------
ext.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  (async () => {
    switch (msg && msg.cmd) {
      case "getStatus":
        sendResponse(await getState());
        break;
      case "enable":
        await patchState({ enabled: true, lastError: null });
        stopped = false;
        await startLoop();
        sendResponse(await getState());
        break;
      case "disable":
        await patchState({ enabled: false });
        stopLoop();
        sendResponse(await getState());
        break;
      default:
        sendResponse({ error: "unknown command" });
    }
  })();
  // Keep the message channel open for the async response.
  return true;
});

// Default to OFF on install (opt-in posture). Never auto-connect.
ext.runtime.onInstalled.addListener(async () => {
  const stored = await ext.storage.local.get(STORAGE_KEY);
  if (!stored[STORAGE_KEY]) {
    await ext.storage.local.set({ [STORAGE_KEY]: { enabled: false } });
  }
  console.info(
    "[fauxx-decoy] installed (disabled by default; opt in from the popup)",
  );
});

// On worker startup, reconnect to the host ONLY if the operator previously
// opted in (the worker is ephemeral and may be respawned at any time).
ext.runtime.onStartup.addListener(async () => {
  const { enabled } = await getState();
  if (enabled) {
    await startLoop();
  }
});

// Also resume the loop when the worker is freshly evaluated while enabled (the
// alarm survives worker restarts, but this gives an immediate pull and ensures
// the alarm exists after an update/reinstall).
(async () => {
  const { enabled } = await getState();
  if (enabled) {
    await startLoop();
  }
})();
