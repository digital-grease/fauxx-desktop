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

//! `fauxx-desktop`: the GUI client, a thin client over `fauxx-core`.
//!
//! The GUI is gated behind the opt-in `gui` feature so the default build links
//! no windowing dependencies (the headless invariant). With `gui` enabled this
//! is an Iced MVU shell plus a resident system tray; without it the binary
//! prints guidance and exits. All real work goes through the `fauxx-core`
//! async API: the modules here only translate Messages into core calls and
//! core results into view state (the thin-client rule).

#![forbid(unsafe_code)]

#[cfg(feature = "gui")]
mod bg;
#[cfg(feature = "gui")]
mod firstrun;
#[cfg(feature = "gui")]
mod message;
#[cfg(feature = "gui")]
mod state;
#[cfg(feature = "gui")]
mod tray;
#[cfg(feature = "gui")]
mod update;
#[cfg(feature = "gui")]
mod view;
#[cfg(feature = "gui")]
mod views;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Stderr logging (RUST_LOG, default info) PLUS a persisted, rotating debug
    // log file and a crash-capturing panic hook; the GUI's "Export logs" action
    // ships it, scrubbed, to a bug report. See fauxx_core::logging.
    fauxx_core::logging::init();
    run()
}

#[cfg(feature = "gui")]
fn run() -> ExitCode {
    match gui_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("fauxx-desktop GUI exited with error: {err}");
            eprintln!("fauxx-desktop: GUI error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Boot the Iced application. The system tray is created before the iced event
/// loop starts (see [`tray`]); the loop owns winit and we bridge the tray's
/// global menu/event channels into an iced subscription.
#[cfg(feature = "gui")]
fn gui_main() -> iced::Result {
    use iced::window;
    use iced::Task;

    use crate::message::Message;
    use crate::state::App;

    // Build the resident tray icon before iced takes over the event loop. The
    // returned handle must be kept alive for the whole process, so it is moved
    // into the init closure and parked inside `App`. See the `tray` module for
    // the per-OS backend split (ksni on Linux, tray-icon on Windows/macOS) and
    // the event-loop co-existence rationale.
    //
    // iced's `init` closure must be `Fn` (it may be invoked more than once),
    // but taking the tray handle is a one-shot move. A `Mutex<Option<_>>` gives
    // the interior mutability that bridges the two: the first init takes the
    // handle, any later init gets `None` and simply runs tray-less.
    let tray = std::sync::Mutex::new(tray::Tray::new());

    iced::application(
        move || {
            let core = fauxx_core::Core::new();
            let tray_handle = match tray.lock() {
                Ok(mut guard) => guard.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            };
            // Kick off the first store-open + status load immediately.
            let app = App::new(core.clone(), tray_handle);
            let boot = Task::perform(bg::open_and_load(core), Message::Booted);
            (app, boot)
        },
        update::update,
        view::view,
    )
    .title(App::title)
    .subscription(subscription)
    // Close-to-tray: the window-manager close button hides the window instead
    // of exiting. The agent and tray stay resident; the tray's "Quit" item is
    // the real exit path.
    .exit_on_close_request(false)
    .window(window::Settings {
        size: iced::Size::new(720.0, 520.0),
        min_size: Some(iced::Size::new(480.0, 360.0)),
        ..window::Settings::default()
    })
    .run()
}

/// Combined subscription: the periodic status tick (only while Running) plus
/// the tray bridge (always, so a tray click can re-open a hidden window) plus
/// window close-requests (to implement close-to-tray).
#[cfg(feature = "gui")]
fn subscription(app: &crate::state::App) -> iced::Subscription<crate::message::Message> {
    use crate::message::Message;
    use crate::state::AppState;

    let mut subs = vec![
        tray::subscription(),
        iced::window::close_requests().map(Message::CloseRequested),
    ];
    if matches!(app.state, AppState::Running { .. }) {
        subs.push(iced::time::every(std::time::Duration::from_secs(2)).map(|_| Message::Tick));
    }
    iced::Subscription::batch(subs)
}

#[cfg(not(feature = "gui"))]
fn run() -> ExitCode {
    eprintln!(
        "fauxx-desktop was built without the `gui` feature.\n\
         Rebuild with `cargo run -p fauxx-desktop --features gui`, or run the \
         headless `fauxx-cli` CLI."
    );
    // Exit code 2: usage / wrong build configuration.
    ExitCode::from(2)
}
