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

// Flat ESLint config for the MV3 WebExtension. The extension source runs in a
// browser/service-worker context (chrome.*, fetch, console), while the tests
// run under Node's built-in test runner. We declare the right globals per area
// so recommended no-undef does not flag platform APIs.

import js from "@eslint/js";
import globals from "globals";

export default [
  // Ignore vendored deps so eslint . does not walk node_modules.
  {
    ignores: ["node_modules/**"],
  },
  js.configs.recommended,
  // Extension source: browser + WebExtension globals (chrome.*, etc.).
  {
    files: ["src/**/*.js", "popup/**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: {
        ...globals.browser,
        ...globals.webextensions,
      },
    },
  },
  // Tests run under Node's built-in runner (node:test, node:assert).
  {
    files: ["test/**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: {
        ...globals.node,
      },
    },
  },
  // This config file itself is Node-evaluated.
  {
    files: ["eslint.config.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: {
        ...globals.node,
      },
    },
  },
];
