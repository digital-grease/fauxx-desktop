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

//! The C7 #30/#31 egress + DNS PANEL.
//!
//! Pure rendering of a [`NetworkSnapshot`] already loaded into state. For a
//! selected persona it shows and sets the per-persona [`fauxx_core::Egress`]
//! (N1) and [`fauxx_core::DnsStrategy`] (N2), surfaces the egress EXIT indicator
//! (the configured exit label plus reachable / paused state), and always shows
//! the explicit DNS observer trade-off note (who sees this persona's lookups).
//!
//! The preset egress/DNS options here are the ones that need no extra free-text
//! (Direct, Tor on the default local SOCKS port; system / DoH / DoT on the
//! privacy-respecting default resolvers). Proxy hosts with credentials are a
//! follow-up that needs a secret-entry surface; the core API supports them.
//!
//! It issues no core calls: every control emits a [`Message`] the update fn
//! turns into a `core.set/get_persona_egress` / `set/get_persona_dns` task.

use fauxx_core::{DnsStrategy, Egress, EgressExit};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, Space};
use iced::{Color, Element, Length};

use crate::message::{Message, NetworkSnapshot};

pub fn view(snapshot: Option<&NetworkSnapshot>, busy: bool) -> Element<'_, Message> {
    let body: Element<'_, Message> = match snapshot {
        Some(snapshot) => loaded(snapshot, busy),
        None => text("Loading egress and DNS config...").size(14).into(),
    };

    column![toolbar(busy), body]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let reload = button(text(if busy { "Working..." } else { "Reload" }))
        .on_press_maybe((!busy).then_some(Message::RefreshNetwork))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::CloseNetwork)
        .padding(8);

    row![
        text("Egress and DNS").size(20),
        Space::new().width(Length::Fill),
        reload,
        back,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

fn loaded<'a>(snapshot: &'a NetworkSnapshot, busy: bool) -> Element<'a, Message> {
    if snapshot.selected_persona.is_none() {
        return container(
            text("No persona to configure yet. Add or import a persona first.").size(13),
        )
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into();
    }

    let body = column![
        persona_picker(snapshot, busy),
        exit_panel(snapshot.exit.as_ref()),
        egress_panel(&snapshot.egress, busy),
        dns_panel(&snapshot.dns, &snapshot.dns_note, busy),
    ]
    .spacing(12);

    scrollable(body).height(Length::Fill).into()
}

/// The persona selector.
fn persona_picker<'a>(snapshot: &'a NetworkSnapshot, busy: bool) -> Element<'a, Message> {
    let choices: Vec<IdChoice> = snapshot
        .personas
        .iter()
        .map(|(id, name)| IdChoice {
            id: id.clone(),
            label: name.clone(),
        })
        .collect();
    let selected = snapshot
        .selected_persona
        .as_ref()
        .and_then(|sel| choices.iter().find(|c| &c.id == sel).cloned());

    let picker: Element<'a, Message> = if busy {
        text("(working...)").size(12).width(Length::Fill).into()
    } else {
        pick_list(choices, selected, |c: IdChoice| {
            Message::NetworkSelectPersona(c.id)
        })
        .padding(6)
        .width(Length::Fill)
        .into()
    };

    container(
        row![text("Persona:").size(12).width(Length::Fixed(80.0)), picker,]
            .spacing(8)
            .align_y(iced::Alignment::Center),
    )
    .padding(12)
    .width(Length::Fill)
    .style(crate::style::panel)
    .into()
}

/// The egress exit indicator: the configured exit label plus reachable / paused
/// state and the fail-closed pause reason.
fn exit_panel(exit: Option<&EgressExit>) -> Element<'_, Message> {
    let mut col = column![text("Exit indicator").size(16)].spacing(6);
    match exit {
        Some(exit) => {
            col = col.push(labeled("Exits via", exit.label.clone()));
            let state_color: fn(&iced::Theme) -> Color = if exit.paused {
                crate::style::danger_color
            } else if exit.reachable {
                crate::style::success_color
            } else {
                crate::style::warning_color
            };
            let state = if exit.paused {
                "PAUSED"
            } else if exit.reachable {
                "reachable"
            } else {
                "unreachable"
            };
            col = col.push(
                row![
                    text("State:").size(11).width(Length::Fixed(80.0)),
                    text(state)
                        .size(11)
                        .style(move |t| crate::style::text_in(state_color(t))),
                ]
                .spacing(8),
            );
            if let Some(reason) = &exit.paused_reason {
                col = col.push(
                    text(reason.clone())
                        .size(11)
                        .style(move |t| crate::style::text_in(state_color(t))),
                );
            }
        }
        None => col = col.push(text("No exit indicator available.").size(12)),
    }

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

/// The egress (N1) preset selector plus the current value.
fn egress_panel(current: &Egress, busy: bool) -> Element<'static, Message> {
    let mut col = column![text("Egress (route to the internet)").size(16)].spacing(8);
    col = col.push(text(current.exit_label()).size(11));

    let row = row![
        egress_button("Direct (real IP)", Egress::Direct, current, busy),
        egress_button("Tor (local SOCKS)", Egress::tor(), current, busy),
    ]
    .spacing(8);
    col = col.push(row);
    col = col.push(
        text(
            "Direct uses the real public IP by design. A configured (non-Direct) \
             egress that is unreachable PAUSES the persona rather than leaking the \
             real IP. Proxy/VPN exits with credentials are configured via the core API.",
        )
        .size(10),
    );

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

fn egress_button(
    label: &'static str,
    option: Egress,
    current: &Egress,
    busy: bool,
) -> Element<'static, Message> {
    let is_active = &option == current;
    let press = (!is_active && !busy).then_some(Message::NetworkSetEgress(option));
    button(text(label).size(12))
        .on_press_maybe(press)
        .padding(8)
        .style(if is_active {
            button::primary
        } else {
            button::secondary
        })
        .into()
}

/// The DNS-strategy (N2) preset selector, the current value, and the always-on
/// observer trade-off note.
fn dns_panel(current: &DnsStrategy, note: &str, busy: bool) -> Element<'static, Message> {
    let mut col = column![text("DNS resolution").size(16)].spacing(8);

    let buttons = row![
        dns_button("System default", DnsStrategy::SystemDefault, current, busy),
        dns_button("DoH (default)", DnsStrategy::doh_default(), current, busy),
        dns_button("DoT (default)", DnsStrategy::dot_default(), current, busy),
    ]
    .spacing(8);
    col = col.push(buttons);

    // The explicit observer trade-off note, never hidden.
    col = col.push(
        container(text(note.to_string()).size(11))
            .padding(8)
            .width(Length::Fill)
            .style(crate::style::warning_pill),
    );

    container(col)
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

fn dns_button(
    label: &'static str,
    option: DnsStrategy,
    current: &DnsStrategy,
    busy: bool,
) -> Element<'static, Message> {
    let is_active = &option == current;
    let press = (!is_active && !busy).then_some(Message::NetworkSetDns(option));
    button(text(label).size(12))
        .on_press_maybe(press)
        .padding(8)
        .style(if is_active {
            button::primary
        } else {
            button::secondary
        })
        .into()
}

fn labeled(label: &str, value: String) -> Element<'static, Message> {
    row![
        text(format!("{label}:"))
            .size(11)
            .width(Length::Fixed(80.0)),
        text(value).size(11),
    ]
    .spacing(8)
    .into()
}

/// A `(id, label)` pick-list choice (selection on the id).
#[derive(Clone, PartialEq, Eq)]
pub struct IdChoice {
    pub id: String,
    pub label: String,
}

impl std::fmt::Display for IdChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}
