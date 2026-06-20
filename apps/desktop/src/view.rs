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

//! `view`: the pure render function. It dispatches on [`AppState`] to a
//! per-state render function under [`crate::views`] and frames the result with
//! a shared header and optional error banner. It reads state only; it never
//! calls the core.

use iced::widget::{button, column, container, row, text};
use iced::{Element, Length};

use crate::message::Message;
use crate::state::{App, AppState};

pub fn view(app: &App) -> Element<'_, Message> {
    let body: Element<'_, Message> = match &app.state {
        AppState::Loading => crate::views::loading::view(),
        AppState::Running {
            status,
            personas,
            refreshing,
        } => crate::views::running::view(status, personas, *refreshing),
        AppState::Devices { snapshot, busy } => {
            crate::views::devices::view(snapshot.as_ref(), *busy)
        }
        AppState::Dashboard {
            snapshot,
            selected_platform,
            busy,
            ..
        } => crate::views::dashboard::view(snapshot.as_ref(), *selected_platform, *busy),
        AppState::Studio { snapshot, busy } => {
            crate::views::studio::view(snapshot.as_deref(), *busy)
        }
        AppState::Brokers { snapshot, busy } => {
            crate::views::brokers::view(snapshot.as_deref(), *busy)
        }
        AppState::Campaigns {
            snapshot,
            draft,
            busy,
        } => crate::views::campaigns::view(snapshot.as_ref(), draft, *busy),
        AppState::Network { snapshot, busy } => {
            crate::views::network::view(snapshot.as_ref(), *busy)
        }
        AppState::Privacy {
            snapshot,
            tab,
            busy,
        } => crate::views::privacy::view(snapshot.as_deref(), *tab, *busy),
        AppState::Settings {
            draft,
            port_text,
            busy,
        } => crate::views::settings::view(draft, port_text, *busy),
        AppState::Faq => crate::views::faq::view(),
        AppState::Wizard {
            step,
            payload,
            import_note,
            busy,
        } => crate::views::wizard::view(*step, payload, import_note.as_deref(), *busy),
        AppState::Error(msg) => crate::views::error::view(msg),
    };

    let mut layout = column![header()].spacing(8);
    if let Some(err) = &app.error_banner {
        layout = layout.push(error_banner(err));
    }
    layout = layout.push(body);

    container(layout)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn header() -> Element<'static, Message> {
    row![
        text("Fauxx").size(24),
        iced::widget::Space::new().width(Length::Fill),
        text("desktop companion").size(12),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .into()
}

fn error_banner(msg: &str) -> Element<'_, Message> {
    container(
        row![
            text(msg.to_string()),
            iced::widget::Space::new().width(Length::Fill),
            button("Dismiss").on_press(Message::ErrorDismissed),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .padding(8)
    // Theme-aware danger styling so the banner is legible in Light and Dark.
    .style(crate::style::error_banner)
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PrivacyTab;
    use fauxx_core::Core;

    // The MVU shell (#5) renders every state through the top-level dispatch +
    // shared header + optional banner, without panicking. (iced has no headless
    // renderer; this guards the view-construction path.)
    #[test]
    fn top_level_view_renders_every_pending_screen_and_the_banner() {
        let mut app = App::new(Core::new(), None);
        let states = || {
            vec![
                AppState::Loading,
                AppState::Error("boom".to_string()),
                AppState::Devices {
                    snapshot: None,
                    busy: true,
                },
                AppState::Dashboard {
                    snapshot: None,
                    selected_platform: 0,
                    selected_device: None,
                    busy: true,
                },
                AppState::Studio {
                    snapshot: None,
                    busy: true,
                },
                AppState::Brokers {
                    snapshot: None,
                    busy: true,
                },
                AppState::Network {
                    snapshot: None,
                    busy: true,
                },
                AppState::Privacy {
                    snapshot: None,
                    tab: PrivacyTab::Dsar,
                    busy: true,
                },
                AppState::Settings {
                    draft: crate::prefs::DesktopSettings::default(),
                    port_text: String::new(),
                    busy: false,
                },
                AppState::Faq,
            ]
        };
        for state in states() {
            app.state = state;
            let _ = view(&app);
        }
        // The non-fatal error banner renders above the body.
        app.error_banner = Some("non-fatal".to_string());
        app.state = AppState::Loading;
        let _ = view(&app);
    }
}
