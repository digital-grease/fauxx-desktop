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

// Hard guardrails for the in-browser decoy path. These are the SAME invariants
// the native decoy browser enforces (C2 #13 R3, and the project privacy
// invariants), ported to the WebExtension so the two paths cannot diverge:
//
//   1. NEVER navigate or fetch an authenticated-account sign-in endpoint. The
//      blocklist below mirrors AUTH_FLOW_BLOCKLIST in
//      crates/fauxx-core/src/browser/isolation.rs entry-for-entry.
//   2. Decoy-only: only plain HTTPS public pages are eligible. No credential
//      entry, no form submission, no ad-clicks / invalid-traffic / fake
//      conversions; the decoy only fetches and (optionally) visits a page.
//   3. Fail closed: anything we cannot prove safe is refused, not allowed.
//
// All checks are pure and run BEFORE any network request leaves the browser.
// Nothing about a blocked attempt leaves the machine; refusals are logged to
// the local service-worker console only.

// Authenticated-account sign-in endpoints the decoy must NEVER drive. Each
// entry is [hostSuffix, optionalPathPrefix]. The host match is a dot-boundary
// suffix match (so subdomains of a blocked host are blocked, but look-alikes
// like notaccounts.google.com.evil.test are not). Mirrors the Rust core's
// AUTH_FLOW_BLOCKLIST exactly; keep the two lists in lockstep.
export const AUTH_FLOW_BLOCKLIST = [
  // Google account sign-in.
  ["accounts.google.com", null],
  ["accounts.youtube.com", null],
  // Microsoft / Live account sign-in.
  ["login.live.com", null],
  ["login.microsoft.com", null],
  ["login.microsoftonline.com", null],
  // Meta / Facebook login (bare host and the explicit login paths).
  ["facebook.com", "/login"],
  ["www.facebook.com", "/login"],
  ["facebook.com", "/login.php"],
  ["www.facebook.com", "/login.php"],
  // Apple ID sign-in.
  ["appleid.apple.com", null],
];

// Split a URL into { host, path } without depending on URL parsing quirks.
// Returns null for URLs with no authority (about:, data:, file: without host),
// which the caller treats as "not a blocked auth host" (and a non-HTTPS scheme
// is rejected separately by isDecoyEligible). Mirrors host_and_path() in the
// Rust core's isolation.rs.
export function hostAndPath(url) {
  if (typeof url !== "string") {
    return null;
  }
  const schemeSplit = url.indexOf("://");
  if (schemeSplit === -1) {
    return null;
  }
  const afterScheme = url.slice(schemeSplit + 3);
  // Authority ends at the first '/', '?' or '#'.
  const authEndMatch = afterScheme.search(/[/?#]/);
  const authEnd = authEndMatch === -1 ? afterScheme.length : authEndMatch;
  const authority = afterScheme.slice(0, authEnd);
  const rest = afterScheme.slice(authEnd);
  // Drop any userinfo ("user:pass@") and any port (":443").
  const atIdx = authority.lastIndexOf("@");
  const hostPort = atIdx === -1 ? authority : authority.slice(atIdx + 1);
  const host = hostPort.split(":")[0];
  if (!host) {
    return null;
  }
  // The path is the leading part of `rest` up to a query/fragment.
  let path;
  if (rest === "") {
    path = "/";
  } else {
    const pathEndMatch = rest.search(/[?#]/);
    const pathEnd = pathEndMatch === -1 ? rest.length : pathEndMatch;
    path = rest.slice(0, pathEnd);
  }
  return { host, path };
}

// Whether `url` is an authenticated-account endpoint the decoy must not drive.
// A URL with no parseable host (about:blank, data:) is NOT on the blocklist.
// Mirrors is_blocked_auth_flow() in the Rust core.
export function isBlockedAuthFlow(url) {
  const parsed = hostAndPath(url);
  if (!parsed) {
    return false;
  }
  const host = parsed.host.replace(/\.+$/, "").toLowerCase();
  const path = parsed.path.toLowerCase();

  return AUTH_FLOW_BLOCKLIST.some(([blockedHost, blockedPath]) => {
    const hostMatches =
      host === blockedHost || host.endsWith("." + blockedHost);
    if (!hostMatches) {
      return false;
    }
    if (blockedPath === null) {
      return true;
    }
    return path.startsWith(blockedPath);
  });
}

// Whether `url` is eligible for decoy visiting/fetching at all.
//
// Decoy-only invariants, fail closed:
//   - must be a plain HTTPS URL (the Topics API and a sane decoy both require a
//     secure context; the curated site set is all HTTPS),
//   - must parse to a real host,
//   - must NOT be on the authenticated sign-in blocklist.
//
// Returns { allowed: boolean, reason: string }.
export function isDecoyEligible(url) {
  if (typeof url !== "string" || url.length === 0) {
    return { allowed: false, reason: "empty or non-string url" };
  }
  if (!url.startsWith("https://")) {
    return { allowed: false, reason: "decoy targets must be https://" };
  }
  const parsed = hostAndPath(url);
  if (!parsed) {
    return { allowed: false, reason: "url has no parseable host" };
  }
  if (isBlockedAuthFlow(url)) {
    return {
      allowed: false,
      reason:
        "refusing decoy visit to authenticated-account endpoint " +
        "(decoy automation must never drive real sign-in flows)",
    };
  }
  return { allowed: true, reason: "" };
}

// Guard a decoy target: returns the url if eligible, else throws after a
// local-only console warning. The warning never leaves the machine. Mirrors
// ensure_navigation_allowed() in the Rust core.
export function ensureDecoyAllowed(url) {
  const verdict = isDecoyEligible(url);
  if (!verdict.allowed) {
    console.warn(
      "[fauxx-decoy] refused decoy target:",
      url,
      "-",
      verdict.reason,
    );
    throw new Error("fauxx decoy guardrail: " + verdict.reason);
  }
  return url;
}
