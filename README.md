# Fauxx Desktop Companion

A privacy command center for the desktop: it generates realistic decoy web activity through synthetic personas to pollute the profiles that data brokers and ad-tech build about you. It is the cross-device companion to the [Fauxx Android app](https://github.com/digital-grease/fauxx), sharing the same persona model so a household can present one coherent, deliberately misleading picture across phone and desktop.

It is decoy-only and local-first by design: it never touches your real accounts, never logs in anywhere, sends no telemetry, and keeps all state in an encrypted store on your own machine.

> Status: early and under active development. Interfaces and on-disk formats can still change. See [Status](#status).

## What it does

- **Synthetic personas.** Coherent, plausible decoy identities (demographics, interests) drawn from a real US Census ACS-PUMS distribution, mirroring the Android persona contract so the two stay in lockstep.
- **Real-browser decoy.** Drives a dedicated, isolated Chromium profile over the DevTools Protocol so a persona's interests actually influence the Topics API and similar surfaces, on a throwaway profile that is verifiably separate from your real browser.
- **Cross-device coordination.** Pairs with the phone over the LAN (sealed crypto_box channel, QR pairing) so devices can run the same persona and rotate together, or deliberately fragment.
- **Deterministic-channel defense.** Helpers for data-subject access requests, per-site masked aliases, and a read-only account inventory (no automation against real services).
- **Measurement.** KL-divergence and per-category drift, a treated-versus-control A/B measure, and CSV/JSON/PDF efficacy snapshots, so you can see whether the noise is working.
- **Network and identity.** Optional per-persona egress (HTTP/SOCKS proxy, Tor, VPN) and DNS strategy (system, DoH, DoT), applied to the isolated decoy profile and fail-closed (an unreachable egress pauses the persona, it never falls back to your real IP).
- **Orchestration.** Household timeline scheduling, goal-driven campaigns, and an optional Home Assistant (MQTT) bridge for a 24/7 homelab deployment.
- **WebExtension (optional, secondary).** A standalone Manifest V3 extension for lighter-weight in-browser decoy injection, talking to the core through a native-messaging host. See [`extension/`](./extension).

## How it works

The real work lives in a headless library so every surface shares one implementation and the same guarantees:

- **`crates/fauxx-core`** is the headless core: personas, the encrypted store, sync, the decoy browser, measurement, orchestration. It holds no UI types.
- **`apps/cli`** is the `fauxx-cli` binary: a clap command surface over the core, plus a `serve` mode for headless homelab use and a `native-host` subcommand for the WebExtension.
- **`apps/desktop`** is an [Iced](https://iced.rs) GUI, behind the opt-in `gui` feature, so a default build links no windowing libraries.
- **`extension/`** is the standalone WebExtension (not a Cargo member, plain JS).

## Privacy guarantees

These are enforced in code, not just intended:

- **Decoy-only.** No real-account sign-in flows are ever driven. A hard blocklist of authentication endpoints is enforced fail-closed at browser launch and on every navigation.
- **Local-first.** No analytics, no telemetry, no remote endpoint. The only thing that leaves the machine is the decoy traffic itself.
- **Encrypted at rest.** State lives in a SQLCipher database whose key is held in the OS keystore, with an Argon2id passphrase-file fallback for headless hosts. Secrets are never written to the database or logs.
- **Fail closed.** When a configured egress, keystore, or guardrail check cannot be satisfied, the affected action stops rather than degrading to a less-private path.

## Install

Prebuilt binaries for Linux, macOS, and Windows are attached to each [GitHub release](https://github.com/digital-grease/fauxx-desktop/releases) (both the `fauxx-cli` CLI and the `fauxx-desktop` GUI). Each archive ships with a `.sha256` checksum and a Sigstore build-provenance attestation.

This project does not ask you to pipe a remote script into a shell. Download, verify, then run.

Verify and extract an archive (Linux/macOS shown; the Windows `.zip` works the same way):

```sh
# 1. Download the archive for your platform plus its checksum from the release page.
# 2. Verify the download against the published checksum:
sha256sum -c fauxx-cli-x86_64-unknown-linux-gnu.tar.xz.sha256
# 3. Extract and run:
tar -xJf fauxx-cli-x86_64-unknown-linux-gnu.tar.xz
./fauxx-cli-x86_64-unknown-linux-gnu/fauxx-cli --version
```

You can also verify provenance with `gh attestation verify <file> --repo digital-grease/fauxx-desktop`.

An installer script (`*-installer.sh` / `*-installer.ps1`) is also attached; it verifies the archive checksum for you. Download it, review it, then run it as a local file (do not pipe it into a shell). Until code-signing certificates are provisioned, the binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will warn on first launch.

## Build

Requires Rust (see [`rust-toolchain.toml`](./rust-toolchain.toml)). All dependencies are version-pinned in the workspace.

```sh
# Headless core + CLI (no GUI, no windowing libraries):
cargo build --release

# With the GUI (opt-in feature):
cargo build --release -p fauxx-desktop --features gui
```

On Linux the GUI needs a few system libraries at build time (`libxkbcommon`, Wayland, D-Bus, `pkg-config`); see [`dist-workspace.toml`](./dist-workspace.toml) for the exact list. The real-browser decoy uses your system-installed Chromium at run time.

## Usage

The CLI is the primary surface. A few examples:

```sh
fauxx-cli status                 # show core/store status
fauxx-cli persona list           # list synthetic personas
fauxx-cli pair                   # pair with the phone (shows/scans a QR payload)
fauxx-cli run                    # run the agent in the foreground
fauxx-cli serve --config c.json  # headless homelab mode (optionally with MQTT)
fauxx-cli native-host            # the WebExtension bridge (launched by the browser)
```

Run `fauxx-cli --help` (and `fauxx-cli <command> --help`) for the full surface, which also covers egress/DNS, broker DSAR, aliases, anchors, exports, generate/mint, and campaigns.

To run the GUI, build with the `gui` feature and run `fauxx-desktop` (a graphical session is required). The system tray uses the StatusNotifierItem spec on Linux and the native tray on Windows and macOS.

## Cross-device sync

Pair the desktop with the phone over the local network: one device shows a QR payload carrying its public key and a connection hint, the other scans (or pastes) it. After pairing, personas and signed artifacts move over a sealed channel that unpaired devices cannot read or write. The wire contract and security model live in the `crate::sync::wire` and `crate::sync` modules.

## Status

This is early software. It builds and is covered by a large test suite, but it has not had a tagged release yet, and some paths still need hardware to exercise end to end (a graphical session for the GUI, a real authenticated proxy for that egress mode). Expect rough edges and changing formats. Bug reports and feature requests are welcome through the [issue forms](https://github.com/digital-grease/fauxx-desktop/issues/new/choose).

## FAQ

**Does it ever touch my real accounts or log in anywhere?**
No. It is decoy-only, and a fail-closed blocklist refuses authenticated sign-in endpoints. It never imports cookies, tokens, or logins from your real browser profile.

**Does it phone home?**
No. There is no telemetry and no remote endpoint. The only network traffic it creates is the decoy browsing itself.

**The GUI does not start.**
The GUI is behind the `gui` feature and needs a graphical session. Build with `cargo build -p fauxx-desktop --features gui`, and run it from a desktop session. The default build is intentionally headless.

**Can it use an authenticated proxy?**
Yes. The decoy browser answers the proxy authentication challenge over CDP using credentials held in the keystore (never the database or logs). If an authenticated proxy is configured but no credentials are stored, the launch fails closed.

**The WebExtension says the native host is unavailable.**
The extension needs its native-messaging host (the `fauxx-cli native-host` subcommand) installed and registered. See [`extension/native-host/README.md`](./extension/native-host/README.md). Until then the extension runs standalone with its bundled site table.

## Contributing

Issues use structured forms (bug, crash, feature) that auto-label on submit; please pick the form that fits. For open-ended privacy-theory or threat-model questions, and for speculative ideas, use [Discussions](https://github.com/digital-grease/fauxx-desktop/discussions).

## License

[GNU Affero General Public License v3.0 or later](./LICENSE) (AGPL-3.0-or-later), the same license as the rest of the Fauxx project.
