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

// Manifest tests. The extension ships as ONE unpacked directory that both
// Chromium and Gecko load directly (there is no build step), so the single
// manifest has to satisfy both background-script schemas at once. #39 was
// exactly this going wrong: only `background.service_worker` was declared, and
// every Firefox fork refused the install with "background.service_worker is
// currently disabled. Add background.scripts." These tests pin the
// cross-browser shape so it cannot regress back to one engine.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const manifest = JSON.parse(
  readFileSync(new URL("../manifest.json", import.meta.url), "utf8"),
);

test("background declares BOTH engines' entry points (#39)", () => {
  const background = manifest.background;
  assert.ok(background, "manifest has no background section");

  // Gecko reads `scripts` (MV3 event page). Without it, every Firefox fork
  // refuses the install outright: this is the #39 regression.
  assert.deepEqual(
    background.scripts,
    ["src/background.js"],
    "Gecko needs background.scripts; without it Firefox refuses to install (#39)",
  );

  // Chromium reads `service_worker` and ignores `scripts`.
  assert.equal(
    background.service_worker,
    "src/background.js",
    "Chromium needs background.service_worker",
  );

  // Both engines must load the same entry point, or the two browsers would
  // silently run different code.
  assert.equal(
    background.scripts[0],
    background.service_worker,
    "the two engines must point at the same background entry point",
  );
});

test("background is declared as an ES module", () => {
  // Every file under src/ uses import/export, so both the Chromium service
  // worker and the Gecko event page have to be loaded as modules or the
  // background script throws on its first import.
  assert.equal(manifest.background.type, "module");
});

test("the Gecko entry is a real, listed file path", () => {
  // A typo here fails at install time in the browser rather than in CI, which
  // is how #39 reached two users. Resolve it for real.
  const entry = manifest.background.scripts[0];
  const resolved = fileURLToPath(new URL(`../${entry}`, import.meta.url));
  assert.doesNotThrow(
    () => readFileSync(resolved, "utf8"),
    `background entry ${entry} does not exist on disk`,
  );
});

test("the manifest still declares its Gecko identity", () => {
  // Firefox needs an explicit add-on id for an unsigned/temporary install and
  // for the native-messaging host allowlist to match.
  const gecko = manifest.browser_specific_settings?.gecko;
  assert.ok(gecko, "browser_specific_settings.gecko is required by Firefox");
  assert.equal(gecko.id, "fauxx-decoy@digital-grease.github.io");
  assert.ok(gecko.strict_min_version, "a strict_min_version pins the MV3 baseline");
});

test("manifest stays on MV3", () => {
  assert.equal(manifest.manifest_version, 3);
});
