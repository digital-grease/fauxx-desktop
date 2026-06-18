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

//! The `Running` view: the core status panel and the persona list, with a
//! manual Refresh action. Pure rendering of the snapshot already loaded into
//! state; it issues no core calls.

use fauxx_core::{Status, SyntheticPersona};
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Length};

use crate::message::Message;

pub fn view<'a>(
    status: &'a Status,
    personas: &'a [SyntheticPersona],
    refreshing: bool,
) -> Element<'a, Message> {
    column![
        row![status_panel(status), personas_panel(personas)]
            .spacing(16)
            .height(Length::FillPortion(4)),
        controls(refreshing),
    ]
    .spacing(12)
    .height(Length::Fill)
    .into()
}

fn status_panel(status: &Status) -> Element<'_, Message> {
    let store_state = if status.store_attached {
        "attached"
    } else {
        "not attached"
    };
    let col = column![
        text("Core status").size(18),
        labeled("Version", status.version.to_string()),
        labeled("Summary", status.summary.clone()),
        labeled("Store", store_state.to_string()),
        labeled("Personas", status.persona_count.to_string()),
    ]
    .spacing(6);

    container(col)
        .padding(12)
        .width(Length::FillPortion(2))
        .height(Length::Fill)
        .style(panel_style)
        .into()
}

fn personas_panel(personas: &[SyntheticPersona]) -> Element<'_, Message> {
    let mut col = column![text("Personas").size(18)].spacing(6);
    if personas.is_empty() {
        col =
            col.push(text("No personas yet. They appear here once the store holds some.").size(12));
    } else {
        for persona in personas {
            col = col.push(persona_row(persona));
        }
    }

    container(scrollable(col).height(Length::Fill))
        .padding(12)
        .width(Length::FillPortion(3))
        .height(Length::Fill)
        .style(panel_style)
        .into()
}

fn persona_row(persona: &SyntheticPersona) -> Element<'_, Message> {
    column![
        text(persona.name.clone()).size(14),
        text(format!("{} - {}", persona.region, persona.profession)).size(11),
        text(persona.id.clone()).size(10),
    ]
    .spacing(2)
    .into()
}

fn controls(refreshing: bool) -> Element<'static, Message> {
    let dashboard = button(text("Dashboard"))
        .on_press(Message::OpenDashboard)
        .padding(8);

    let studio = button(text("Studio"))
        .on_press(Message::OpenStudio)
        .padding(8);

    let devices = button(text("Devices"))
        .on_press(Message::OpenDevices)
        .padding(8);

    let brokers = button(text("Brokers"))
        .on_press(Message::OpenBrokers)
        .padding(8);

    let campaigns = button(text("Campaigns"))
        .on_press(Message::OpenCampaigns)
        .padding(8);

    let network = button(text("Network"))
        .on_press(Message::OpenNetwork)
        .padding(8);

    let privacy = button(text("Privacy"))
        .on_press(Message::OpenPrivacy)
        .padding(8);

    let refresh = button(text(if refreshing {
        "Refreshing..."
    } else {
        "Refresh"
    }))
    .on_press_maybe((!refreshing).then_some(Message::Refresh))
    .padding(8);

    // Bug-report path: export a scrubbed copy of the debug logs to attach to an
    // issue (opens a save dialog). See fauxx_core::logging.
    let export_logs = button(text("Export logs"))
        .on_press(Message::ExportLogs)
        .padding(8);

    row![
        dashboard,
        studio,
        devices,
        brokers,
        campaigns,
        network,
        privacy,
        iced::widget::Space::new().width(Length::Fill),
        export_logs,
        refresh,
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

fn labeled(label: &str, value: String) -> Element<'static, Message> {
    row![
        text(format!("{label}:"))
            .size(13)
            .width(Length::Fixed(90.0)),
        text(value).size(13),
    ]
    .spacing(8)
    .into()
}

fn panel_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Color::from_rgba8(0xf6, 0xf6, 0xf8, 1.0).into()),
        // Styled containers in iced 0.14 render child text in an undefined
        // color unless `text_color` is set; pin to dark grey for legibility.
        text_color: Some(iced::Color::from_rgba8(0x1a, 0x1a, 0x1f, 1.0)),
        border: iced::Border {
            color: iced::Color::from_rgba8(0xdd, 0xdd, 0xe0, 1.0),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}
