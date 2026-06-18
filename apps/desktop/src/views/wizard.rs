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

//! The C8 #34 U3 skippable FIRST-RUN WIZARD.
//!
//! Pure rendering of the wizard state already in [`crate::state::AppState::Wizard`].
//! Three steps, with a persistent Skip on every step (the wizard is skippable):
//!
//! 1. Welcome.
//! 2. The key step: import a persona from the phone by its O1 pairing payload.
//!    The user scans the phone's pairing QR and pastes the decoded payload (the
//!    same compact base64url string the Devices view's QR carries); submitting it
//!    completes pairing via the core pairing API so personas sync from the phone.
//!    A full in-app camera QR scan is out of scope for the desktop and deferred;
//!    the payload-paste path reuses the existing Devices QR plumbing.
//! 3. Done.
//!
//! Completing (Finish) or skipping records the first-run-completed flag and lands
//! the app in Running. It issues no core calls: every action is a [`Message`].

use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Element, Length};

use crate::message::Message;
use crate::state::WizardStep;

pub fn view<'a>(
    step: WizardStep,
    payload: &'a str,
    import_note: Option<&'a str>,
    busy: bool,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = match step {
        WizardStep::Welcome => welcome_step(),
        WizardStep::ImportPhonePersona => import_step(payload, import_note, busy),
        WizardStep::Done => done_step(),
    };

    let card = container(column![progress(step), body, nav(step, busy)].spacing(16))
        .padding(20)
        .width(Length::Fixed(560.0))
        .style(panel_style);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .into()
}

/// A small "step N of 3" progress line.
fn progress(step: WizardStep) -> Element<'static, Message> {
    let n = match step {
        WizardStep::Welcome => 1,
        WizardStep::ImportPhonePersona => 2,
        WizardStep::Done => 3,
    };
    text(format!("Setup - step {n} of 3")).size(12).into()
}

fn welcome_step() -> Element<'static, Message> {
    column![
        text("Welcome to Fauxx").size(22),
        text(
            "Fauxx runs synthetic browsing personas to blur your real profile across \
             ad platforms and data brokers. This quick setup links your phone so a \
             persona can come across from there. You can skip and do it later."
        )
        .size(13),
    ]
    .spacing(10)
    .into()
}

/// The key step: paste the phone's pairing payload to pair and import.
fn import_step<'a>(
    payload: &'a str,
    import_note: Option<&'a str>,
    busy: bool,
) -> Element<'a, Message> {
    let input = text_input("paste the phone's pairing code here", payload)
        .on_input(Message::WizardEditPayload)
        .padding(8)
        .width(Length::Fill);

    let import = button(text(if busy {
        "Pairing..."
    } else {
        "Pair and import"
    }))
    .on_press_maybe((!busy).then_some(Message::WizardImportPayload))
    .padding(8);

    let mut col = column![
        text("Import a persona from your phone").size(18),
        text(
            "On the phone, open Fauxx and show its pairing QR. Scan it, then paste the \
             decoded pairing code below. Pairing opens the secure channel so personas \
             sync from the phone."
        )
        .size(12),
        input,
        row![Space::new().width(Length::Fill), import].align_y(iced::Alignment::Center),
    ]
    .spacing(10);

    if let Some(note) = import_note {
        col = col.push(text(note.to_string()).size(12));
    }

    col.into()
}

fn done_step() -> Element<'static, Message> {
    column![
        text("You are set up").size(22),
        text(
            "You can manage personas in the Studio, watch efficacy in the Dashboard, \
             and pair more devices from Devices. Fauxx keeps running in the tray; \
             closing the window does not quit it."
        )
        .size(13),
    ]
    .spacing(10)
    .into()
}

/// The footer navigation: Back / Skip on the left, Next or Finish on the right.
fn nav(step: WizardStep, busy: bool) -> Element<'static, Message> {
    let back = button(text("Back"))
        .on_press_maybe((!busy && step.previous().is_some()).then_some(Message::WizardBack))
        .padding(8)
        .style(button::secondary);

    let skip = button(text("Skip setup"))
        .on_press_maybe((!busy).then_some(Message::WizardSkip))
        .padding(8)
        .style(button::secondary);

    let advance: Element<'static, Message> = match step.next() {
        Some(_) => button(text("Next"))
            .on_press_maybe((!busy).then_some(Message::WizardNext))
            .padding(8)
            .into(),
        None => button(text("Finish"))
            .on_press_maybe((!busy).then_some(Message::WizardFinish))
            .padding(8)
            .into(),
    };

    row![back, skip, Space::new().width(Length::Fill), advance,]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
}

fn panel_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Color::from_rgba8(0xf6, 0xf6, 0xf8, 1.0).into()),
        text_color: Some(iced::Color::from_rgba8(0x1a, 0x1a, 0x1f, 1.0)),
        border: iced::Border {
            color: iced::Color::from_rgba8(0xdd, 0xdd, 0xe0, 1.0),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}
