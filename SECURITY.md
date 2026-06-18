# Security Policy

Fauxx Desktop is a privacy tool, and some of the people who rely on it are at elevated personal risk. Security and privacy reports are taken seriously and handled with that in mind.

## Supported versions

Security fixes target the latest release. Please confirm an issue still reproduces on the most recent version before reporting.

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a vulnerability

**Please do not open a public issue for a security or privacy vulnerability.** A public report can put users at risk before a fix is available.

Use one of these private channels:

1. **GitHub private vulnerability reporting** (preferred). Go to the **Security** tab of this repository and choose **Report a vulnerability**. This opens a private advisory that only the maintainers can see.
2. **Email**: dg@digitalgrease.net. If you want to encrypt the report, ask for a key in a first short message.

Helpful things to include:

- The affected version (from `fauxx --version` or the release tag) and the OS (Linux, macOS, or Windows).
- Whether it involves the headless `fauxx` CLI, the desktop GUI, the `serve` homelab mode, or the companion browser extension.
- The component or screen involved, and a file or line reference if you have one.
- What an attacker or another local process could observe or do, and what access or user action it requires.
- Steps to reproduce, or a short proof of concept.

## What to expect

- An acknowledgement within a few days.
- An assessment of severity and scope, and a request for any missing detail.
- A coordinated fix. A private advisory is published once a fix has shipped, with credit to the reporter unless anonymity is requested.

## Scope notes

This project cares about more than classic exploitability. Findings that weaken the privacy guarantees the tool exists to provide are in scope. Examples:

- Personal or identifying data that survives into a shared artifact, such as a `fauxx logs export` bug-report file, a persona pack, or an efficacy snapshot, that should have been redacted.
- Data that survives a user-initiated wipe, or a secret (a key, a passphrase, a proxy credential) that reaches the SQLite plaintext, a log, or any output rather than staying in the OS keystore.
- A way to make decoy traffic read as real activity under the user's identity, or to link a persona's decoy traffic back to the real user (for example a per-persona egress that silently falls back to the direct route).
- Cross-device sync accepting a frame from an unpaired peer, or any sealed-channel weakness that exposes plaintext persona state on the wire.

If you are unsure whether something qualifies, report it privately and ask.
