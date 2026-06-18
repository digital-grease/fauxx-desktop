<!--
  fauxx-desktop: Fauxx Desktop Companion
  Copyright (C) 2026 Digital Grease
  Licensed under the GNU Affero General Public License v3 or later.
  See the LICENSE file at the repository root.
-->

# Fauxx native-messaging host

This directory holds the registration artifacts for the **native-messaging
host** that bridges the Fauxx Decoy Companion WebExtension to the headless Fauxx
Core. The host itself is the `native-host` subcommand of the `fauxx` CLI binary
(`apps/cli`); it is launched by the browser, not run interactively.

The wire contract the host implements is in [`../PROTOCOL.md`](../PROTOCOL.md)
and [`../src/protocol.js`](../src/protocol.js); a concrete end-to-end exchange is
in [`sample-exchange.json`](./sample-exchange.json).

## What the host does

When the extension opts in it calls `runtime.connectNative("com.digital_grease.fauxx")`.
The browser starts the host and connects the extension to its stdin/stdout. The
host then:

- sends a `hello` handshake (Core version + schema version);
- on a `requestPlan`, builds a **decoy-only** plan biased by the named persona's
  interest categories (via the core's category-targeting API), enforcing the
  same hard guardrails as the native decoy path (HTTPS-only targets, the
  authenticated sign-in blocklist, decoy intent only), and returns it as a
  `decoyPlan`;
- on a `decoyReport`, persists the in-browser decoy session (visited sites) into
  the encrypted measurement store, dropping any reported visit that fails the
  guardrail re-check;
- on a `topicsReadback`, calls `Core::record_topics_readback`;
- on a `gpcStatus`, calls `Core::record_gpc_status`;
- exits cleanly on EOF (the browser closed the connection).

Everything is local. The only thing that leaves the machine is the decoy traffic
the extension issues; the host is purely local stdio over the encrypted store.

## Files

| File | Role |
|---|---|
| `com.digital_grease.fauxx.chromium.json.template` | Chromium manifest (uses `allowed_origins`). |
| `com.digital_grease.fauxx.firefox.json.template` | Firefox manifest (uses `allowed_extensions`). |
| `com.digital_grease.fauxx.json.template` | The original combined template referenced by `PROTOCOL.md`; the two browser-specific files above are preferred. |
| `fauxx-native-host.sh` | Launcher wrapper the manifest `path` points at; it invokes `fauxx native-host` with the store flags. |
| `sample-exchange.json` | A concrete reference exchange. |

## Why a launcher wrapper

The browser launches the host by the absolute `path` in the manifest and does
not let you specify arguments (it appends the extension origin, and on Chromium a
parent-window handle, as argv). The Fauxx host is the `native-host` **subcommand**
of the `fauxx` binary, so the manifest `path` points at the small
[`fauxx-native-host.sh`](./fauxx-native-host.sh) wrapper, which `exec`s
`fauxx native-host` with the right store environment. The browser's appended argv
is ignored by the subcommand. On Windows, use an equivalent `.bat`/`.cmd`
wrapper (`fauxx.exe native-host`) and point the registry `path` at it.

## The store key source (unattended)

The browser starts the host with **no TTY**, so it cannot prompt for a
passphrase. Use the headless **encrypted-key-file** key source: a passphrase file
unlocking an Argon2id-wrapped key (the `fauxx` global `--passphrase-file` /
`FAUXX_PASSPHRASE_FILE` flag), as the wrapper sets up. Lock the passphrase file
down to your user (`chmod 600`). The OS keystore default is only viable here if
your keystore unlocks without an interactive prompt for the browser's session.

## Install

1. **Build the binary.**

   ```fish
   cargo build --release -p fauxx-cli
   # the binary is target/release/fauxx
   ```

2. **Set up the launcher.** Copy `fauxx-native-host.sh` somewhere stable (a path
   with no spaces), make it executable, and edit the three `REPLACE` values:
   `FAUXX_BIN` (absolute path to the built `fauxx`), `FAUXX_DB`, and
   `FAUXX_PASSPHRASE_FILE`.

   ```fish
   chmod +x /path/to/fauxx-native-host.sh
   ```

3. **Get the extension id.** Load the extension unpacked (see
   [`../README.md`](../README.md)) and note its id.
   - Chromium: the id shown on `chrome://extensions` (a 32-char string); the
     `allowed_origins` entry is `chrome-extension://<id>/`.
   - Firefox: the gecko id `fauxx-decoy@digital-grease.github.io` (already filled
     in the Firefox template).

4. **Fill in a manifest.** Copy the matching template to
   `com.digital_grease.fauxx.json`, set `path` to the absolute path of your
   launcher wrapper, and set the extension id.

5. **Install the manifest** in the per-browser location:

   | Browser / OS | Manifest directory |
   |---|---|
   | Chrome (Linux) | `~/.config/google-chrome/NativeMessagingHosts/` |
   | Chromium (Linux) | `~/.config/chromium/NativeMessagingHosts/` |
   | Chrome (macOS) | `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/` |
   | Firefox (Linux) | `~/.mozilla/native-messaging-hosts/` |
   | Firefox (macOS) | `~/Library/Application Support/Mozilla/NativeMessagingHosts/` |
   | Windows | a registry key `HKCU\Software\<Chromium\|Mozilla>\NativeMessagingHosts\com.digital_grease.fauxx` whose default value is the absolute path to the manifest file |

   The file must be named exactly `com.digital_grease.fauxx.json`.

   ```fish
   # Chrome on Linux, for example:
   mkdir -p ~/.config/google-chrome/NativeMessagingHosts
   cp com.digital_grease.fauxx.json ~/.config/google-chrome/NativeMessagingHosts/
   ```

6. **Opt in.** Open the extension popup and flip the toggle. The extension calls
   `connectNative` and you should see the `hello` handshake clear the connection
   error in the popup.

## Verify the host without a browser

The host speaks the native-messaging framing on stdin/stdout (a 4-byte
native-endian length prefix per JSON message), so it is awkward to drive by hand.
The hermetic unit tests in `apps/cli/src/commands/native_host.rs` exercise the
frame codec and the request dispatch over in-memory streams (no browser, temp
encrypted store):

```fish
cargo test -p fauxx-cli native_host
```

`fauxx native-host --help` shows the global store flags the wrapper passes.

## Troubleshooting

- **"Specified native messaging host not found" / "No such native application".**
  The manifest is missing, misnamed, or in the wrong directory; or the extension
  id under `allowed_origins` / `allowed_extensions` does not match. Re-check
  steps 3-5.
- **The host starts then exits immediately.** The store could not be opened: a
  wrong/missing passphrase file, an unreadable db path, or an interactive
  keystore prompt the browser session cannot answer. Run the wrapper from a
  terminal to see the error on stderr.
- **Nothing happens after opting in.** Confirm `path` in the manifest is the
  absolute path to an executable wrapper (`chmod +x`), with no spaces.
