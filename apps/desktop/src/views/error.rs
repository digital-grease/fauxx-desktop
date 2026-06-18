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

//! The terminal `Error` view: a message shown when boot failed
//! unrecoverably. The user can quit from the tray.

use iced::widget::{column, text};
use iced::Element;

use crate::message::Message;

pub fn view(msg: &str) -> Element<'_, Message> {
    column![
        text("Fauxx could not start.").size(20),
        text(msg.to_string()).size(14),
        text("Quit from the system tray, or relaunch.").size(12),
    ]
    .spacing(12)
    .into()
}
