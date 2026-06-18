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

// The opt-in control surface. The popup holds NO logic of its own beyond
// rendering state and relaying enable/disable to the background worker (the
// thin-client rule applies here too: the worker is the only place that talks to
// the Core, and the Core does the real work).

const ext = typeof browser !== "undefined" ? browser : chrome;

const enabledInput = document.getElementById("enabled");
const enabledLabel = document.getElementById("enabledLabel");
const connectedEl = document.getElementById("connected");
const lastReportEl = document.getElementById("lastReport");
const errorRow = document.getElementById("errorRow");
const lastErrorEl = document.getElementById("lastError");

function sendCmd(cmd) {
  return new Promise((resolve) => {
    ext.runtime.sendMessage({ cmd }, (resp) => resolve(resp || {}));
  });
}

function render(state) {
  const enabled = state.enabled === true;
  enabledInput.checked = enabled;
  enabledLabel.textContent = enabled
    ? "Decoy injection is ON"
    : "Decoy injection is OFF";
  connectedEl.textContent = state.connected ? "connected" : "not connected";

  if (state.lastReport) {
    const r = state.lastReport;
    const when = new Date(r.finishedAt).toLocaleTimeString();
    lastReportEl.textContent =
      r.visited + " visited, " + r.skipped + " skipped (" + when + ")";
  } else {
    lastReportEl.textContent = "none yet";
  }

  if (state.lastError) {
    errorRow.hidden = false;
    lastErrorEl.textContent = state.lastError;
  } else {
    errorRow.hidden = true;
    lastErrorEl.textContent = "";
  }
}

enabledInput.addEventListener("change", async () => {
  const state = await sendCmd(enabledInput.checked ? "enable" : "disable");
  render(state);
});

(async () => {
  render(await sendCmd("getStatus"));
})();
