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

//! Linux system-tray backend over `ksni` (freedesktop StatusNotifierItem).
//!
//! Unlike the `tray-icon` backend on Windows/macOS, this needs no platform
//! event loop and no extra OS thread: `ksni` (with its `tokio` feature) runs the
//! StatusNotifierItem D-Bus service on the GUI's own tokio runtime, and the
//! whole thing lives inside the iced [`subscription`]. It pulls in `zbus` only,
//! so the Linux GUI graph carries none of the archived gtk-rs GTK3 crates (nor
//! the `glib` unsoundness) that `tray-icon`'s `libappindicator` backend would.
//!
//! A menu selection (or a left-click on the icon) runs a small `activate`
//! closure that pushes a [`TrayMessage`] into a bounded async channel; the
//! subscription stream forwards it as [`Message::Tray`], so a tray action
//! becomes an ordinary message in `update` exactly like the other backend.

use iced::futures::Stream;
use iced::Subscription;
use ksni::TrayMethods;

use crate::message::{Message, TrayMessage};

/// One-shot carrier, mirroring the other backend's surface. On Linux the tray
/// runs inside [`subscription`], so this carries only a marker [`TrayHandle`].
pub struct Tray {
    handle: Option<TrayHandle>,
}

impl Tray {
    /// Build the carrier. Never fails: the actual StatusNotifierItem service is
    /// started (best-effort) by [`subscription`].
    pub fn new() -> Self {
        Self {
            handle: Some(TrayHandle),
        }
    }

    /// Take the handle for parking inside `App`. `None` on a second call.
    pub fn take(&mut self) -> Option<TrayHandle> {
        self.handle.take()
    }
}

impl Default for Tray {
    fn default() -> Self {
        Self::new()
    }
}

/// Marker parked in `App` to keep the cross-platform surface uniform. On Linux
/// the tray lives in the iced subscription (ksni on the GUI's tokio runtime), so
/// there is no resident thread or handle to own; holding this is a no-op.
pub struct TrayHandle;

/// The resident StatusNotifierItem. Its `activate` closures push intents into
/// `tx`, which the [`subscription`] stream drains.
struct FauxxTray {
    tx: iced::futures::channel::mpsc::Sender<TrayMessage>,
}

/// Push an intent toward the subscription. `try_send` never blocks: if the
/// bounded channel is momentarily full or the app is shutting down, the intent
/// is dropped rather than stalling ksni's D-Bus task.
fn send(tray: &mut FauxxTray, intent: TrayMessage) {
    let _ = tray.tx.try_send(intent);
}

impl ksni::Tray for FauxxTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        "Fauxx".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Fauxx desktop companion".into(),
            ..Default::default()
        }
    }

    /// A small solid teal pixmap so the tray shows something without shipping an
    /// image asset or depending on the system icon theme.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        const SIZE: i32 = 16;
        let pixels = (SIZE * SIZE) as usize;
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            // ARGB32, network (big-endian) byte order: [A, R, G, B]. The Fauxx
            // accent is a muted teal (0x2a9d8f), fully opaque.
            data.extend_from_slice(&[0xff, 0x2a, 0x9d, 0x8f]);
        }
        vec![ksni::Icon {
            width: SIZE,
            height: SIZE,
            data,
        }]
    }

    /// The C8 #34 U3 quick-control menu, identical in intent to the other
    /// backend: Open Window, Status, Pause, Resume, Quit.
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Window".into(),
                activate: Box::new(|t: &mut Self| send(t, TrayMessage::OpenWindow)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Status".into(),
                activate: Box::new(|t: &mut Self| send(t, TrayMessage::ShowStatus)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Pause".into(),
                activate: Box::new(|t: &mut Self| send(t, TrayMessage::Pause)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Resume".into(),
                activate: Box::new(|t: &mut Self| send(t, TrayMessage::Resume)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| send(t, TrayMessage::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    /// A left-click on the icon opens (or re-opens) the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        send(self, TrayMessage::OpenWindow);
    }
}

/// The iced subscription that runs the ksni tray and surfaces its events as
/// [`Message::Tray`].
///
/// `Subscription::run` keys off the plain `fn` pointer, so the tray is started
/// exactly once and lives for the life of the subscription (the whole app).
pub fn subscription() -> Subscription<Message> {
    Subscription::run(tray_event_stream)
}

fn tray_event_stream() -> impl Stream<Item = Message> {
    use iced::futures::channel::mpsc;
    use iced::futures::{SinkExt, StreamExt};

    iced::stream::channel(64, |mut output: mpsc::Sender<Message>| async move {
        // Channel from the ksni menu/activate closures into this async task.
        let (tx, mut rx) = mpsc::channel::<TrayMessage>(64);

        // `ksni`'s `tokio` feature runs the StatusNotifierItem service on the
        // GUI's tokio runtime (this subscription executes within it).
        let tray = FauxxTray { tx };
        match tray.spawn().await {
            Ok(handle) => {
                // Keep the handle in scope alongside the event loop below. ksni
                // runs its service as a detached task with no drop-based
                // teardown, so the tray lives until the process exits regardless;
                // holding the handle keeps it available (e.g. for future
                // `Handle::update` calls) rather than dropping it immediately.
                let _service = handle;
                while let Some(intent) = rx.next().await {
                    if output.send(Message::Tray(intent)).await.is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                // No StatusNotifier host (a bare WM with no tray, say): the
                // window and its in-app controls still work without a tray.
                tracing::warn!("system tray unavailable, running without tray: {err}");
                // Park so iced does not treat the subscription as finished.
                std::future::pending::<()>().await;
            }
        }
    })
}
