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

//! The Settings screen: app appearance/behavior prefs (theme, auto-refresh
//! cadence, close-to-tray) and device/sync prefs (device name, LAN-sync, port).
//!
//! Pure rendering of the in-progress [`DesktopSettings`] draft already in state;
//! form changes emit [`Message`]s that [`crate::update`] applies to the draft,
//! and Save persists it. The appearance/behavior prefs take effect on Save; the
//! device/sync prefs take effect at the next start (noted in the UI).

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};

use crate::message::Message;
use crate::prefs::{DesktopSettings, ThemeChoice, MAX_REFRESH_SECS, MIN_REFRESH_SECS};

pub fn view<'a>(
    draft: &'a DesktopSettings,
    port_text: &'a str,
    busy: bool,
) -> Element<'a, Message> {
    let body = column![
        appearance_section(draft),
        behavior_section(draft),
        device_section(draft, port_text),
    ]
    .spacing(16);

    column![toolbar(busy), scrollable(body).height(Length::Fill)]
        .spacing(12)
        .height(Length::Fill)
        .into()
}

fn toolbar(busy: bool) -> Element<'static, Message> {
    let save = button(text(if busy { "Saving..." } else { "Save" }))
        .on_press_maybe((!busy).then_some(Message::SettingsSave))
        .padding(8);
    let back = button(text("< Back"))
        .on_press(Message::CloseSettings)
        .padding(8);
    row![
        text("Settings").size(20),
        iced::widget::Space::new().width(Length::Fill),
        save,
        back,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// A titled card wrapping a section's controls in the theme-aware panel style.
fn section<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(column![text(title).size(16), content].spacing(10))
        .padding(12)
        .width(Length::Fill)
        .style(crate::style::panel)
        .into()
}

fn appearance_section(draft: &DesktopSettings) -> Element<'_, Message> {
    let mut picker = row![text("Theme").width(Length::Fixed(160.0))].spacing(8);
    for choice in ThemeChoice::all() {
        let active = choice == draft.theme;
        let label = if active {
            format!("[{}]", choice.label())
        } else {
            choice.label().to_string()
        };
        picker = picker.push(
            button(text(label))
                .on_press_maybe((!active).then_some(Message::SettingsSetTheme(choice)))
                .padding(6),
        );
    }
    section("Appearance", picker.align_y(Alignment::Center).into())
}

fn behavior_section(draft: &DesktopSettings) -> Element<'_, Message> {
    let secs = draft.auto_refresh_secs;
    let dec = button(text("-"))
        .on_press_maybe(
            (secs > MIN_REFRESH_SECS).then_some(Message::SettingsSetAutoRefresh(secs - 1)),
        )
        .padding(6);
    let inc = button(text("+"))
        .on_press_maybe(
            (secs < MAX_REFRESH_SECS).then_some(Message::SettingsSetAutoRefresh(secs + 1)),
        )
        .padding(6);
    let refresh = row![
        text("Auto-refresh (seconds)").width(Length::Fixed(220.0)),
        dec,
        text(secs.to_string()).width(Length::Fixed(36.0)),
        inc,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let close_to_tray = checkbox(draft.close_to_tray)
        .label("Close button hides to tray (off = quit)")
        .on_toggle(Message::SettingsToggleCloseToTray);

    section(
        "Behavior",
        column![refresh, close_to_tray].spacing(10).into(),
    )
}

fn device_section<'a>(draft: &'a DesktopSettings, port_text: &'a str) -> Element<'a, Message> {
    let name = row![
        text("Device name").width(Length::Fixed(160.0)),
        text_input(
            "(derived from hostname)",
            draft.device_name.as_deref().unwrap_or("")
        )
        .on_input(Message::SettingsSetDeviceName)
        .padding(6)
        .width(Length::Fixed(260.0)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let lan_sync = checkbox(draft.lan_sync)
        .label("Enable LAN sync at start")
        .on_toggle(Message::SettingsToggleLanSync);

    let port = row![
        text("Sync port").width(Length::Fixed(160.0)),
        text_input("(core default)", port_text)
            .on_input(Message::SettingsSetSyncPort)
            .padding(6)
            .width(Length::Fixed(120.0)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let note = text("These device and sync settings apply at the next start.")
        .size(12)
        .style(crate::style::muted_text);

    section(
        "Device and sync",
        column![name, lan_sync, port, note].spacing(10).into(),
    )
}
