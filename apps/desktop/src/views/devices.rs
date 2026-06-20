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

//! The `Devices` view: the cross-device sync surface (C1).
//!
//! Pure rendering of a [`DevicesSnapshot`] already loaded into state. It shows
//! this device's pairing QR (rendered as the crisp SVG from
//! [`fauxx_core::PairingQr`]) and fingerprint, the paired and discovered peer
//! lists, and a segmented control for the household
//! [`fauxx_core::CoordinationMode`]. It issues no core calls: every action is a
//! [`Message`] the update fn turns into a background task.
//!
//! There is deliberately no "send persona to phone" control: the live byte
//! transport that would deliver a sealed frame to the phone is a documented
//! follow-up, so this view exposes only the store-backed surface (pair record,
//! peers, mode).

use fauxx_core::{CoordinationMode, DiscoveredPeer, PairedPeer};
use iced::widget::{button, column, container, row, scrollable, svg, text};
use iced::{Element, Length};

use crate::message::{DevicesSnapshot, Message};

pub fn view(snapshot: Option<&DevicesSnapshot>, busy: bool) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, busy),
        None => text("Loading device and pairing details...")
            .size(14)
            .into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

/// The top bar: back to Running plus a reload action and a busy hint.
fn toolbar(busy: bool) -> Element<'static, Message> {
    let back = button(text("< Back"))
        .on_press(Message::CloseDevices)
        .padding(8);

    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshDevices))
        .padding(8);

    row![
        text("Devices").size(20),
        iced::widget::Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The two-column body shown once a snapshot is loaded: pairing on the left,
/// peers + mode on the right.
fn loaded<'a>(snapshot: &'a DevicesSnapshot, busy: bool) -> Element<'a, Message> {
    let right = column![
        mode_panel(snapshot.mode, busy),
        paired_panel(&snapshot.paired, busy),
        discovered_panel(&snapshot.discovered),
    ]
    .spacing(12)
    .width(Length::FillPortion(3));

    row![
        pairing_panel(snapshot),
        scrollable(right).height(Length::Fill)
    ]
    .spacing(16)
    .height(Length::Fill)
    .into()
}

/// This device's pairing QR + fingerprint.
fn pairing_panel<'a>(snapshot: &'a DevicesSnapshot) -> Element<'a, Message> {
    let mut col = column![text("This device").size(16)].spacing(8);

    match &snapshot.pairing_qr {
        Some(qr) => {
            // The crisp vector form: render the SVG document the core produced.
            let handle = svg::Handle::from_memory(qr.svg.clone().into_bytes());
            col = col.push(
                container(
                    svg(handle)
                        .width(Length::Fixed(220.0))
                        .height(Length::Fixed(220.0)),
                )
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center),
            );
            col = col.push(text("Scan this with the Fauxx phone app to pair.").size(11));
        }
        None => {
            col = col.push(
                text("No pairing code yet. Open the encrypted store to generate one.").size(12),
            );
        }
    }

    if let Some(fingerprint) = &snapshot.fingerprint {
        col = col.push(labeled("Fingerprint", fingerprint.clone()));
    }

    container(col)
        .padding(12)
        .width(Length::FillPortion(2))
        .height(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The coordination-mode segmented control.
fn mode_panel(active: CoordinationMode, busy: bool) -> Element<'static, Message> {
    let col = column![
        text("Coordination mode").size(16),
        row![
            mode_button(
                "Coherent",
                CoordinationMode::CoherentHousehold,
                active,
                busy,
            ),
            mode_button(
                "Fragmentation",
                CoordinationMode::Fragmentation,
                active,
                busy
            ),
        ]
        .spacing(8),
        text(mode_blurb(active)).size(11),
    ]
    .spacing(8);

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// One segment of the mode control. The active mode is disabled (already set);
/// any in-flight work also disables both to coalesce.
fn mode_button(
    label: &'static str,
    mode: CoordinationMode,
    active: CoordinationMode,
    busy: bool,
) -> Element<'static, Message> {
    let is_active = mode == active;
    let press = (!is_active && !busy).then_some(Message::SetMode(mode));
    button(text(label))
        .on_press_maybe(press)
        .padding(8)
        .style(if is_active {
            button::primary
        } else {
            button::secondary
        })
        .into()
}

fn mode_blurb(mode: CoordinationMode) -> &'static str {
    match mode {
        CoordinationMode::CoherentHousehold => {
            "One shared persona; every paired device advances together."
        }
        CoordinationMode::Fragmentation => {
            "Each paired device runs a distinct persona with independent timing."
        }
        // `CoordinationMode` is `#[non_exhaustive]`; describe any future mode
        // generically rather than failing to render.
        _ => "Custom coordination mode.",
    }
}

/// The paired (trusted) peers, each with an Unpair action.
fn paired_panel<'a>(paired: &'a [PairedPeer], busy: bool) -> Element<'a, Message> {
    let mut col = column![text("Paired devices").size(16)].spacing(8);
    if paired.is_empty() {
        col = col.push(text("No paired devices yet.").size(12));
    } else {
        for peer in paired {
            col = col.push(paired_row(peer, busy));
        }
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

fn paired_row<'a>(peer: &'a PairedPeer, busy: bool) -> Element<'a, Message> {
    let host = peer
        .host
        .as_deref()
        .map(|h| format!("{h}:{}", peer.port))
        .unwrap_or_else(|| format!("port {}", peer.port));

    let details = column![
        text(peer.name.clone()).size(14),
        text(peer.fingerprint.clone()).size(11),
        text(host).size(11),
    ]
    .spacing(2);

    let unpair = button(text("Unpair"))
        .on_press_maybe((!busy).then_some(Message::Unpair(peer.public_key.clone())))
        .padding(6)
        .style(button::danger);

    row![
        details,
        iced::widget::Space::new().width(Length::Fill),
        unpair,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Peers seen over LAN discovery (untrusted until paired).
fn discovered_panel<'a>(discovered: &'a [DiscoveredPeer]) -> Element<'a, Message> {
    let mut col = column![text("Discovered on LAN").size(16)].spacing(8);
    if discovered.is_empty() {
        col = col.push(text("No devices discovered.").size(12));
    } else {
        for peer in discovered {
            let fingerprint = peer
                .fingerprint
                .clone()
                .unwrap_or_else(|| "(no fingerprint advertised)".to_string());
            col = col.push(
                column![text(peer.name.clone()).size(14), text(fingerprint).size(11),].spacing(2),
            );
        }
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

fn labeled(label: &str, value: String) -> Element<'static, Message> {
    column![text(format!("{label}:")).size(11), text(value).size(11),]
        .spacing(2)
        .into()
}
