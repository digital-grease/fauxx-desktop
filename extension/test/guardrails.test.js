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

// Unit tests for the decoy guardrail. These pin the fail-closed invariants so a
// regression that loosens the guardrail (allowing a blocked auth flow, a
// non-HTTPS target, or an unparseable url) fails CI. All checks are pure, so no
// network or browser stubbing is needed except a quiet console.warn for the
// throwing helper, which writes to the local console only.

import test from "node:test";
import assert from "node:assert/strict";

import {
  AUTH_FLOW_BLOCKLIST,
  hostAndPath,
  isBlockedAuthFlow,
  isDecoyEligible,
  ensureDecoyAllowed,
} from "../src/guardrails.js";

test("isDecoyEligible BLOCKS an authenticated sign-in endpoint", () => {
  // accounts.google.com is on the blocklist with a null path (whole host).
  const verdict = isDecoyEligible("https://accounts.google.com/signin");
  assert.equal(verdict.allowed, false);
  assert.match(verdict.reason, /authenticated-account endpoint/);
});

test("isDecoyEligible BLOCKS a subdomain of a blocked auth host", () => {
  // Dot-boundary suffix match: subdomains of a blocked host are blocked too.
  const verdict = isDecoyEligible("https://mail.login.live.com/");
  assert.equal(verdict.allowed, false);
});

test("isDecoyEligible BLOCKS a path-scoped login endpoint", () => {
  // facebook.com is blocked only under /login (and /login.php).
  const blocked = isDecoyEligible("https://www.facebook.com/login.php");
  assert.equal(blocked.allowed, false);
});

test("isDecoyEligible BLOCKS non-HTTPS schemes (fail closed)", () => {
  const verdict = isDecoyEligible("http://example.com/");
  assert.equal(verdict.allowed, false);
  assert.match(verdict.reason, /https/);
});

test("isDecoyEligible BLOCKS empty or non-string input (fail closed)", () => {
  assert.equal(isDecoyEligible("").allowed, false);
  assert.equal(isDecoyEligible(undefined).allowed, false);
  assert.equal(isDecoyEligible(null).allowed, false);
});

test("isDecoyEligible ALLOWS a benign public HTTPS page", () => {
  const verdict = isDecoyEligible("https://example.com/articles/privacy");
  assert.equal(verdict.allowed, true);
  assert.equal(verdict.reason, "");
});

test("isDecoyEligible ALLOWS a look-alike that is not a real subdomain", () => {
  // notaccounts.google.com.evil.test must NOT match accounts.google.com.
  const verdict = isDecoyEligible(
    "https://notaccounts.google.com.evil.test/path",
  );
  assert.equal(verdict.allowed, true);
});

test("isBlockedAuthFlow returns false for a url with no parseable host", () => {
  assert.equal(isBlockedAuthFlow("about:blank"), false);
  assert.equal(isBlockedAuthFlow("data:text/html,hi"), false);
});

test("hostAndPath strips userinfo and port, defaults missing path to /", () => {
  const parsed = hostAndPath("https://user:pass@example.com:443");
  assert.deepEqual(parsed, { host: "example.com", path: "/" });
});

test("hostAndPath returns null for non-string and authority-less urls", () => {
  assert.equal(hostAndPath(42), null);
  assert.equal(hostAndPath("not a url"), null);
});

test("AUTH_FLOW_BLOCKLIST keeps the core sign-in hosts", () => {
  const hosts = AUTH_FLOW_BLOCKLIST.map(([host]) => host);
  for (const required of [
    "accounts.google.com",
    "login.live.com",
    "login.microsoftonline.com",
    "appleid.apple.com",
  ]) {
    assert.ok(hosts.includes(required), "missing blocked host: " + required);
  }
});

test("ensureDecoyAllowed returns the url for an eligible target", () => {
  const url = "https://example.com/";
  assert.equal(ensureDecoyAllowed(url), url);
});

test("ensureDecoyAllowed THROWS for a blocked target", () => {
  // Silence the local-only console.warn the helper emits on refusal.
  const originalWarn = console.warn;
  console.warn = () => {};
  try {
    assert.throws(
      () => ensureDecoyAllowed("https://accounts.google.com/signin"),
      /fauxx decoy guardrail/,
    );
  } finally {
    console.warn = originalWarn;
  }
});
