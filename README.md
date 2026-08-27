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
- **Local-first.** No analytics and no telemetry. The only thing that leaves the machine on its own is the decoy traffic itself. The single network request the app can make about *itself* is the update check, and it happens only when you press the button (see the FAQ).
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

### Linux: AppImage

The desktop GUI is also published as a portable `Fauxx_Desktop_Companion-<version>-x86_64.AppImage` (with a matching `.zsync` for delta updates and a `.sha256` checksum). Download it, verify the checksum, make it executable, and run it, no installation or store needed:

```sh
sha256sum -c Fauxx_Desktop_Companion-*-x86_64.AppImage.sha256
chmod +x Fauxx_Desktop_Companion-*-x86_64.AppImage
./Fauxx_Desktop_Companion-*-x86_64.AppImage
```

The AppImage carries update information, so tools like [AppImageLauncher](https://github.com/TheAssassin/AppImageLauncher) and AppImageUpdate can integrate it and update it in place. It bundles the app's own libraries but uses your system's GPU drivers and Chromium at run time, so a system-installed Chromium is still required for the decoy browser.

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

Pairing is **per-device and must be done both ways**. Scanning the desktop's code pairs the desktop *on the phone*; for the desktop to accept the phone's pushes, pair the phone back from the Devices screen (or `fauxx-cli pair add <phone-code>`). Until both directions are done, an inbound push is refused because neither side can authenticate a peer it has not paired.

The pairing code also carries the desktop's dialable `IP:port` addresses, and the Devices screen lists them with a copy button, so pairing still works when the phone cannot resolve the desktop's `.local.` name over mDNS.

## Status

This is early software. It builds, is covered by a large test suite, and has tagged releases, but some paths still need hardware to exercise end to end (a graphical session for the GUI, a real authenticated proxy for that egress mode). Expect rough edges and changing formats. Bug reports and feature requests are welcome through the [issue forms](https://github.com/digital-grease/fauxx-desktop/issues/new/choose).

## FAQ

**Does it ever touch my real accounts or log in anywhere?**
No. It is decoy-only, and a fail-closed blocklist refuses authenticated sign-in endpoints. It never imports cookies, tokens, or logins from your real browser profile.

**Does it phone home?**
No. There is no telemetry, and nothing is sent anywhere on a timer, at startup, or in the background. The only traffic it creates on its own is the decoy browsing itself.

There is exactly one exception, and it only happens if you ask for it: pressing **Check for updates** in Settings (or running `fauxx-cli check-update`) makes a single request to the GitHub releases API. GitHub then sees your IP address and a User-Agent carrying the app name and version, and nothing else: no machine identifier, no OS string, no install id, and no persona data. If you never press it, nothing is ever contacted. The check also deliberately ignores any per-persona proxy, VPN, or Tor egress, **and any proxy set in your environment** (`ALL_PROXY`, `HTTP_PROXY`, `HTTPS_PROXY`), so a real request identifying this app never shares an exit with a persona's decoy traffic. Genuine network-layer routing you have configured yourself, such as a system VPN, still applies.

**The phone will not pair, or says it has no route to the desktop.**
Two things to check, in order. First, pairing is two-way: after the phone scans the desktop's code, pair the phone back on the desktop's Devices screen, or the desktop will refuse the phone's pushes. Second, if the phone reports no route, it cannot resolve the desktop's `.local.` name; open **Devices** on the desktop, copy one of the addresses under "Reach this device", and enter it on the phone as a manual address. Try the first address listed first.

**How do I know when there is a new version?**
Open **Settings** and press **Check for updates**, or run `fauxx-cli check-update`. Nothing checks automatically, so this is the only way the app will tell you. It reports what the latest release is and gives you the link; it does not download or install anything. On Linux the AppImage carries update information, so AppImageUpdate or AppImageLauncher can update it in place.

**The window opens but shows no text.**
Fixed in 0.3.0, which bundles its own UI font. Earlier versions asked the host for a font family they assumed every system had, and drew nothing on a host without it. The bundled font removes that assumption; text in scripts it does not cover still uses your system fonts.

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

### Third-party components

The desktop GUI embeds **Noto Sans Regular**, Copyright 2022 The Noto Project Authors (<https://github.com/notofonts/latin-greek-cyrillic>), licensed under the [SIL Open Font License 1.1](./apps/desktop/assets/fonts/LICENSE-NotoSans.txt). It ships renamed to the family `Fauxx UI` (name records only, no glyph changes) so a same-named font on your system cannot shadow the copy the app was tested with; `packaging/fonts/build-ui-font.py` performs that rename reproducibly. The font is compiled into the binary so the interface renders on hosts with no fonts configured (see the FAQ), and your own system fonts still cover any script it does not. Every distributed artifact carries the license text alongside the binary: the AppImage under `usr/share/doc/`, and the release archives at `apps/desktop/assets/fonts/LICENSE-NotoSans.txt`.
