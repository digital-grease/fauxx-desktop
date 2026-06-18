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

//! Windows/macOS system-tray backend over `tray-icon`.
//!
//! # Why this is non-trivial
//!
//! iced 0.14 owns a single `winit` event loop on the thread that calls
//! `iced::application(..).run()`, and it does not yield that loop. `tray-icon`,
//! however, requires a *platform* event loop running on whichever thread
//! creates the `TrayIcon`: a Win32 loop on Windows and the AppKit loop on
//! macOS. `TrayIcon` is also neither `Send` nor `Sync`, so it must be created
//! and live on that thread. A single shared loop is not achievable on iced 0.14
//! (it does not expose its `winit` `EventLoopProxy`, the documented integration
//! seam).
//!
//! # The approach taken
//!
//! Two cooperating loops, decoupled through `tray-icon`'s own global channels:
//!
//! 1. A dedicated, detached OS thread ("tray thread") builds the `TrayIcon`
//!    and its menu, then parks for the life of the process. This thread, not
//!    the iced thread, is where the non-`Send` `TrayIcon` lives.
//! 2. `tray-icon`/`muda` publish menu clicks and icon clicks onto process-wide
//!    `crossbeam` channels reachable via [`MenuEvent::receiver`] and
//!    [`TrayIconEvent::receiver`]. A second forwarder thread blocks on those
//!    receivers and pushes decoded [`TrayMessage`]s into an `iced` subscription
//!    stream (see [`subscription`]), so a tray click becomes an ordinary
//!    `Message` in `update`. The iced thread never blocks on the tray.
//!
//! Linux does not use this backend: it uses the `ksni` StatusNotifierItem
//! backend instead (see the parent module), which keeps the GTK3 stack out of
//! the Linux build.

use std::thread::JoinHandle;
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use iced::Subscription;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::message::{Message, TrayMessage};

/// Stable menu-item ids. The forwarder maps an incoming [`MenuId`] back to a
/// [`TrayMessage`] by comparing against these.
const ID_OPEN: &str = "fauxx.tray.open";
const ID_STATUS: &str = "fauxx.tray.status";
const ID_PAUSE: &str = "fauxx.tray.pause";
const ID_RESUME: &str = "fauxx.tray.resume";
const ID_QUIT: &str = "fauxx.tray.quit";

/// A one-shot carrier for the tray handle, so `main` can build the tray once
/// and hand it to the iced `init` closure (which may be called more than once
/// in principle, hence `take`).
pub struct Tray {
    handle: Option<TrayHandle>,
}

impl Tray {
    /// Spin up the tray on its own thread and return a carrier. Never panics:
    /// if the tray thread cannot start, the app simply runs without a tray.
    pub fn new() -> Self {
        Self {
            handle: TrayHandle::spawn(),
        }
    }

    /// Take the handle for parking inside `App`. Returns `None` on a second
    /// call or if the tray failed to start.
    pub fn take(&mut self) -> Option<TrayHandle> {
        self.handle.take()
    }
}

impl Default for Tray {
    fn default() -> Self {
        Self::new()
    }
}

/// Live handle to the resident tray. Parked in `App` purely to keep the tray
/// thread alive for the life of the window; dropping it does not tear the tray
/// down (the thread is detached and runs until the process exits).
pub struct TrayHandle {
    /// The detached tray thread's join handle. Held only so the thread (and the
    /// non-`Send` `TrayIcon` it owns) is not flagged as leaked tooling-side; we
    /// never join it, as the tray lives until `Quit` calls `process::exit`.
    _thread: JoinHandle<()>,
}

impl TrayHandle {
    /// Build the tray on a dedicated thread. The `TrayIcon` is non-`Send`, so
    /// it is created and owned entirely inside the spawned thread.
    fn spawn() -> Option<Self> {
        let builder = std::thread::Builder::new().name("fauxx-tray".to_string());
        match builder.spawn(tray_thread_main) {
            Ok(thread) => Some(Self { _thread: thread }),
            Err(err) => {
                tracing::warn!("could not spawn tray thread, running without tray: {err}");
                None
            }
        }
    }
}

/// Body of the tray thread: build the icon + menu, then park forever. The
/// `TrayIcon` and `Menu` must outlive this function, so they are held in locals
/// that live until the (never-returning) park loop.
fn tray_thread_main() {
    let menu = Menu::new();
    let open = MenuItem::with_id(ID_OPEN, "Open Window", true, None);
    let status = MenuItem::with_id(ID_STATUS, "Status", true, None);
    // C8 #34 U3 quick controls: pause/resume the running campaigns without
    // bringing the window forward.
    let pause = MenuItem::with_id(ID_PAUSE, "Pause", true, None);
    let resume = MenuItem::with_id(ID_RESUME, "Resume", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    if let Err(err) = menu.append_items(&[&open, &status, &pause, &resume, &quit]) {
        tracing::warn!("tray menu build failed: {err}");
        return;
    }

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Fauxx desktop companion")
        .with_title("Fauxx");
    if let Some(icon) = default_icon() {
        builder = builder.with_icon(icon);
    }

    // Keep the built icon alive for the life of the thread (and process).
    let _tray_icon = match builder.build() {
        Ok(icon) => icon,
        Err(err) => {
            // A missing system tray is non-fatal: the window and its in-app
            // controls still work without a tray.
            tracing::warn!("tray icon build failed, continuing without tray: {err}");
            return;
        }
    };

    // The platform event loop pump belongs here if a platform needs it. The
    // icon's lifetime is held by `_tray_icon` and events flow through the global
    // channels the forwarder thread drains.
    loop {
        std::thread::park();
    }
}

/// Build a small solid-color RGBA icon so the tray has something to show
/// without shipping an image asset. Best-effort: `None` on construction error.
fn default_icon() -> Option<Icon> {
    const SIZE: u32 = 16;
    // Fauxx accent: a muted teal, fully opaque.
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[0x2a, 0x9d, 0x8f, 0xff]);
    }
    match Icon::from_rgba(rgba, SIZE, SIZE) {
        Ok(icon) => Some(icon),
        Err(err) => {
            tracing::warn!("tray icon image build failed: {err}");
            None
        }
    }
}

/// Map a menu id to the intent it represents.
fn decode_menu(id: &MenuId) -> Option<TrayMessage> {
    match id.as_ref() {
        ID_OPEN => Some(TrayMessage::OpenWindow),
        ID_STATUS => Some(TrayMessage::ShowStatus),
        ID_PAUSE => Some(TrayMessage::Pause),
        ID_RESUME => Some(TrayMessage::Resume),
        ID_QUIT => Some(TrayMessage::Quit),
        _ => None,
    }
}

/// The iced subscription that surfaces tray events as [`Message::Tray`].
///
/// `Subscription::run` takes a plain `fn` pointer (no captured state), which
/// fits here: the tray's channels are process-global, so the stream needs no
/// per-app data to find them.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(tray_event_stream)
}

/// Forward tray menu/icon events from the global `crossbeam` channels into the
/// iced runtime as `Message`s.
///
/// A blocking `recv` on a `crossbeam` receiver would stall the async executor,
/// so a dedicated forwarder thread does the blocking waits and relays through
/// an async channel. The stream half is what iced polls.
fn tray_event_stream() -> impl Stream<Item = Message> {
    use iced::futures::channel::mpsc;
    iced::stream::channel(64, |mut output: mpsc::Sender<Message>| async move {
        // Dedicated blocking forwarder: drains both global receivers and pushes
        // decoded intents onto an async mpsc the stream below awaits.
        let (tx, mut rx) = mpsc::channel::<TrayMessage>(64);
        std::thread::Builder::new()
            .name("fauxx-tray-bridge".to_string())
            .spawn(move || {
                let menu_rx = MenuEvent::receiver();
                let tray_rx = tray_icon::TrayIconEvent::receiver();
                let mut tx = tx;
                loop {
                    // Poll both channels with a short timeout so neither starves
                    // the other. `recv_timeout` returning a timeout error is the
                    // normal idle path, not a failure.
                    if let Ok(event) = menu_rx.recv_timeout(Duration::from_millis(100)) {
                        if let Some(msg) = decode_menu(event.id()) {
                            if tx.try_send(msg).is_err() {
                                // Receiver gone (app shutting down): stop.
                                break;
                            }
                        }
                    }
                    // A left-click on the icon (where the platform emits it) is
                    // treated as "open the window".
                    while let Ok(event) = tray_rx.try_recv() {
                        if let tray_icon::TrayIconEvent::Click { .. } = event {
                            if tx.try_send(TrayMessage::OpenWindow).is_err() {
                                return;
                            }
                        }
                    }
                }
            })
            .map_err(|err| {
                // If the forwarder thread cannot start, `tx` drops, the stream
                // below ends, and the tray goes silent. Surface that rather than
                // swallow it (mirrors the Linux backend's spawn-failure warning).
                tracing::warn!(
                    target: "fauxx_desktop::tray",
                    error = %err,
                    "could not spawn the tray-bridge forwarder thread; tray menu/icon events will not flow"
                );
            })
            .ok();

        use iced::futures::StreamExt;
        while let Some(intent) = rx.next().await {
            if output.send(Message::Tray(intent)).await.is_err() {
                break;
            }
        }
    })
}
