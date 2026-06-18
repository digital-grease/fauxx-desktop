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

// The in-browser decoy executor. Given a validated, decoy-only plan it performs
// background fetches (or opt-in tab visits) against guardrail-eligible HTTPS
// sites, with GPC carried on every request, and produces a SeedOutcome-shaped
// report (visited / skipped) the background worker forwards to the native host.
//
// This is deliberately LIGHTWEIGHT and SECONDARY to the native decoy browser:
// it runs in the user's normal browser profile, so it does NOT attempt the
// strict isolated-profile guarantees the native path provides. It only ever
// fetches/visits public HTTPS pages, never authenticates, never submits forms,
// never clicks ads, and emits GPC. Those guardrails are the contract; the
// profile is the user's own, and that is documented as the tradeoff in
// extension/README.md.

import { ensureDecoyAllowed, isDecoyEligible } from "./guardrails.js";
import { sitesForCategories } from "./category_sites.js";

// The well-known path a site uses to advertise GPC honoring (per the GPC spec).
// Mirrors fauxx_core::browser::gpc::GPC_WELL_KNOWN_PATH.
export const GPC_WELL_KNOWN_PATH = "/.well-known/gpc.json";

// Build the /.well-known/gpc.json URL for an `https://host` origin, keeping only
// scheme+authority. Mirrors well_known_url_for() in the Rust core.
export function wellKnownUrlFor(origin) {
  const trimmed = String(origin || "").trim();
  let base;
  const schemeSplit = trimmed.indexOf("://");
  if (schemeSplit !== -1) {
    const scheme = trimmed.slice(0, schemeSplit);
    const rest = trimmed.slice(schemeSplit + 3);
    const end = rest.search(/[/?#]/);
    const authority = end === -1 ? rest : rest.slice(0, end);
    base = scheme + "://" + authority;
  } else {
    const end = trimmed.search(/[/?#]/);
    const authority = end === -1 ? trimmed : trimmed.slice(0, end);
    base = "https://" + authority;
  }
  return base + GPC_WELL_KNOWN_PATH;
}

// Parse a /.well-known/gpc.json body into a GpcSupport-shaped object. Tolerant
// by design: null/empty/garbage/non-object all yield the "not advertised"
// observation { honored: false } rather than throwing. Mirrors
// parse_gpc_well_known() in the Rust core, including the string-or-number
// `version` handling.
export function parseGpcWellKnown(body) {
  const notAdvertised = { honored: false };
  if (body === null || body === undefined) {
    return notAdvertised;
  }
  const trimmed = String(body).trim();
  if (trimmed.length === 0) {
    return notAdvertised;
  }
  let value;
  try {
    value = JSON.parse(trimmed);
  } catch (e) {
    return notAdvertised;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return notAdvertised;
  }
  const support = { honored: value.gpc === true };
  if (typeof value.lastUpdate === "string") {
    support.lastUpdate = value.lastUpdate;
  }
  if (typeof value.version === "string") {
    support.version = value.version;
  } else if (typeof value.version === "number") {
    support.version = String(value.version);
  }
  return support;
}

// The JS evaluated in a decoy tab to read the profile's assigned Privacy
// Sandbox Topics. Returns { available, topics } and never throws. Mirrors
// fauxx_core::browser::topics::BROWSING_TOPICS_READ_JS so the host's
// record_topics_readback sees the same shape from either path.
export const BROWSING_TOPICS_READ_JS = `(async () => {
  if (typeof document === 'undefined' ||
      typeof document.browsingTopics !== 'function') {
    return { available: false, topics: [] };
  }
  try {
    const result = await document.browsingTopics();
    const topics = Array.isArray(result) ? result : [];
    return { available: true, topics: topics };
  } catch (e) {
    return { available: true, topics: [] };
  }
})()`;

// Resolve a validated plan's targets into the ordered, de-duplicated, eligible
// HTTPS site list to touch this tick. Explicit `targets` take precedence over
// resolved category sites; the result is capped at plan.maxTargets. Every
// candidate is run through the guardrail and an ineligible one is dropped here
// (so it never even becomes a fetch).
export function resolvePlanTargets(plan) {
  const fromCategories =
    plan.targets && plan.targets.length > 0
      ? plan.targets
      : sitesForCategories(plan.categories);

  const eligible = [];
  const seen = new Set();
  for (const url of fromCategories) {
    if (seen.has(url)) {
      continue;
    }
    seen.add(url);
    if (isDecoyEligible(url).allowed) {
      eligible.push(url);
    }
    if (eligible.length >= plan.maxTargets) {
      break;
    }
  }
  return eligible;
}

// A small async sleep used to pace decoy traffic so it is not a synchronous
// burst (a crude analog of the native path's BrowsingCadence). Derives a
// per-target delay from the plan's intensity name.
function paceMsForIntensity(intensity) {
  switch (intensity) {
    case "Extreme":
      return 250;
    case "High":
      return 750;
    case "Medium":
      return 2000;
    case "Low":
    default:
      return 5000;
  }
}

function sleep(ms) {
  if (!ms || ms <= 0) {
    return Promise.resolve();
  }
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Execute a validated decoy plan in "fetch" mode: a background, no-credentials
// GET to each eligible target. GPC rides every request via the
// declarativeNetRequest ruleset (and we also send a Sec-GPC header directly as
// belt-and-suspenders where the platform allows request headers on fetch).
//
// Returns a SeedOutcome-shaped report: { planId, personaId, seed, visited,
// skipped, startedAt, finishedAt }. A failed/blocked target is a recorded skip,
// never a thrown error, exactly like the native SeedOutcome.
//
// `fetchImpl` is injectable so this is unit-testable without a browser.
export async function runDecoyPlan(plan, fetchImpl) {
  const doFetch = fetchImpl || fetch;
  const startedAt = Date.now();
  const visited = [];
  const skipped = [];

  const targets = resolvePlanTargets(plan);
  const pace = paceMsForIntensity(plan.intensity);

  for (const url of targets) {
    // Re-assert the guardrail right before the request (defense in depth).
    try {
      ensureDecoyAllowed(url);
    } catch (e) {
      skipped.push({ url, reason: String(e.message || e) });
      continue;
    }
    try {
      // credentials:'omit' guarantees no cookies/auth from the user's profile
      // ride along; cache:'no-store' keeps it a genuine fresh request; mode
      // 'no-cors' tolerates opaque cross-origin responses (we only need the
      // request to have HAPPENED, like the native path's history seeding).
      const headers = plan.gpc ? { "Sec-GPC": "1" } : {};
      await doFetch(url, {
        method: "GET",
        credentials: "omit",
        cache: "no-store",
        redirect: "follow",
        mode: "no-cors",
        headers,
      });
      visited.push(url);
    } catch (e) {
      skipped.push({ url, reason: String(e.message || e) });
    }
    await sleep(pace);
  }

  return {
    planId: plan.planId,
    personaId: plan.personaId,
    seed: plan.seed,
    visited,
    skipped,
    startedAt,
    finishedAt: Date.now(),
  };
}
