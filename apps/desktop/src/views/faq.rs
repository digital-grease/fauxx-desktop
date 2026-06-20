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

//! The in-app Help / FAQ screen: static, scrollable reference content. It makes
//! no core call and holds no state. The content is kept accurate to the tool's
//! actual guarantees (decoy-only, 100% local, fail-closed) and its documented
//! limitations, so it never overstates what the companion does.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length};

use crate::message::Message;

/// The FAQ, grouped into sections of (question, answer) pairs.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "About",
        &[
            (
                "What is Fauxx Desktop?",
                "It is a privacy tool that runs synthetic decoy personas. It drives a dedicated, \
                 isolated Chromium profile to browse as plausible fake identities, so ad and \
                 tracking profiles fill with noise instead of your real behavior. It is the \
                 desktop companion to the Fauxx Android app.",
            ),
            (
                "Does it ever touch my real accounts or log in anywhere?",
                "No. It is decoy-only. A fail-closed blocklist refuses authenticated sign-in \
                 endpoints, and it never imports cookies, tokens, or logins from your real \
                 browser profile.",
            ),
            (
                "Does it phone home?",
                "No. There is no telemetry and no remote endpoint. The only network traffic it \
                 creates is the decoy browsing itself.",
            ),
        ],
    ),
    (
        "Privacy and security",
        &[
            (
                "Where is my data stored?",
                "In an encrypted SQLCipher database under your OS data directory (on Linux \
                 ~/.local/share/fauxx, on macOS ~/Library/Application Support/fauxx, on Windows \
                 the per-user app data dir). The database key lives in your OS keystore, with an \
                 Argon2id passphrase-file fallback for headless use.",
            ),
            (
                "What keeps secrets out of the database?",
                "Proxy credentials, the device pairing key, and the persona-pack signing key live \
                 only in the OS keystore, never in the database plaintext or the logs.",
            ),
            (
                "What does fail closed mean?",
                "If a guardrail, the keystore, or a configured network egress cannot be satisfied, \
                 the affected action stops rather than silently doing something less safe. For \
                 example, an unreachable proxy pauses that persona instead of falling back to your \
                 direct connection.",
            ),
            (
                "How is the decoy browser isolated?",
                "It launches only from a dedicated throwaway profile directory that is verified to \
                 be distinct from every real browser profile on the machine. It never reads or \
                 imports your real cookies, tokens, or logins.",
            ),
            (
                "Does it stop harmful searches?",
                "Yes. A blocklist shared with the Android app refuses queries that could draw \
                 scrutiny or create false distress signals. If the blocklist cannot load, it fails \
                 closed and emits nothing.",
            ),
        ],
    ),
    (
        "Cross-device sync",
        &[
            (
                "How does pairing with my phone work?",
                "Devices discover each other on the local network over mDNS, and you pair out of \
                 band by scanning a QR code (or pasting its payload). After pairing, persona data \
                 moves inside an authenticated public-key sealed channel.",
            ),
            (
                "Can an unpaired device read my data?",
                "No. The channel seals only to, and opens only from, paired peers. An unpaired \
                 peer cannot decrypt it (wrong key) or forge it (the authentication tag fails). \
                 Nothing leaves the local network, and there is no backend.",
            ),
        ],
    ),
    (
        "Network and identity",
        &[
            (
                "Can each persona use a different proxy or DNS?",
                "Yes. Per persona you can set an egress (HTTP or SOCKS proxy, Tor, or a VPN via a \
                 local proxy) and a DNS strategy (system, DoH, or DoT). If a configured egress is \
                 unreachable, that persona is paused, never silently sent over your direct route.",
            ),
            (
                "Can it use an authenticated proxy?",
                "Authenticated-proxy support in the browser is a follow-up. Today a persona \
                 configured with proxy credentials is refused launch (fail closed). \
                 Unauthenticated proxies, Tor, and direct connections work today.",
            ),
            (
                "Does it bundle Tor or a VPN?",
                "No. Tor egress expects a local Tor SOCKS proxy that you run separately, and VPN \
                 egress routes through a local proxy your VPN exposes.",
            ),
        ],
    ),
    (
        "Running it",
        &[
            (
                "How do I install it?",
                "Download the archive for your OS from the Releases page, verify its sha256 \
                 checksum, extract it, and run the binary. The installer script also verifies the \
                 checksum; download and run it as a local file rather than piping it into a shell.",
            ),
            (
                "Why does my OS warn the app is unsigned?",
                "Until code-signing certificates are provisioned the binaries are unsigned, so \
                 macOS Gatekeeper and Windows SmartScreen warn on first launch. This is expected \
                 for a pre-release build.",
            ),
            (
                "What do I need to run it?",
                "The GUI needs a graphical session. The real-browser decoy uses your system \
                 installed Chromium at run time (it is not bundled). The headless CLI needs no \
                 display.",
            ),
            (
                "The browser extension says the native host is unavailable.",
                "The extension needs its native-messaging host installed and registered. It is the \
                 native-host subcommand of the fauxx-cli CLI.",
            ),
        ],
    ),
    (
        "Data control",
        &[
            (
                "How do I export logs for a bug report?",
                "Use Export logs on the main screen (or the CLI logs export command). The export is \
                 scrubbed of your home path, username, and persona names; the on-disk log keeps \
                 full detail for your own debugging.",
            ),
            (
                "How do I remove data?",
                "Deleting a persona drops its record and settings, and clearing a persona egress \
                 also removes its keystore credential. Secrets are zeroized in memory when dropped.",
            ),
        ],
    ),
    (
        "What it does not do yet",
        &[
            (
                "Does it opt me out of data brokers automatically?",
                "No. It generates data-subject access and deletion letters, tracks their statutory \
                 deadlines, keeps a read-only account inventory, and manages email aliases, but it \
                 never logs into or automates against real services and never auto-sends anything. \
                 Live broker automation is out of scope by design.",
            ),
            (
                "Is it finished?",
                "It is early and under active development. Interfaces and on-disk formats can still \
                 change, so expect rough edges.",
            ),
        ],
    ),
];

pub fn view() -> Element<'static, Message> {
    let mut body = column![].spacing(14);
    for (section_title, entries) in SECTIONS {
        let mut card = column![text(*section_title).size(16)].spacing(10);
        for (question, answer) in *entries {
            card = card.push(
                column![
                    text(*question).size(14),
                    text(*answer).size(12).style(crate::style::muted_text),
                ]
                .spacing(3)
                .width(Length::Fill),
            );
        }
        body = body.push(
            container(card)
                .padding(12)
                .width(Length::Fill)
                .style(crate::style::panel),
        );
    }

    column![toolbar(), scrollable(body).height(Length::Fill)]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar() -> Element<'static, Message> {
    row![
        text("Help and FAQ").size(20),
        iced::widget::Space::new().width(Length::Fill),
        button(text("< Back"))
            .on_press(Message::CloseFaq)
            .padding(8),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}
