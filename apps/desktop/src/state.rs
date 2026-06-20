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

//! App state: drives the iced UI tree.
//!
//! The top-level [`App`] holds a cheap-to-clone [`fauxx_core::Core`] handle and
//! a finite [`AppState`]. Transitions happen only in [`crate::update::update`];
//! nothing here calls the core (that is the job of [`crate::bg`]).

use fauxx_core::{Core, Status, SyntheticPersona};

use crate::message::{
    BrokerDiffSnapshot, CampaignDraft, CampaignsSnapshot, DashboardSnapshot, DevicesSnapshot,
    NetworkSnapshot, PrivacySnapshot, StudioSnapshot,
};
use crate::prefs::DesktopSettings;
use crate::tray::TrayHandle;

/// Top-level iced application state.
pub struct App {
    /// The shared core handle. Cloned into background tasks; never blocked on.
    pub core: Core,
    /// The GUI-local desktop settings (theme, auto-refresh cadence, close-to-tray,
    /// and the device/sync prefs applied at the next start). Loaded once at
    /// construction; the Settings screen edits a draft and writes it back here on
    /// Save. Drives [`App::theme`] and the subscription tick cadence.
    pub prefs: DesktopSettings,
    /// The current finite state.
    pub state: AppState,
    /// Non-fatal error banner shown above the body. Distinct from
    /// [`AppState::Error`], which is terminal. Dismissed by the user.
    pub error_banner: Option<String>,
    /// The resident tray icon, kept alive for the life of the process. `None`
    /// when the tray failed to initialize (a degraded but still usable window).
    /// Held purely as a liveness guard, so it is intentionally never read back:
    /// dropping `App` (at process exit) is what releases the tray thread.
    #[allow(dead_code)]
    pub tray: Option<TrayHandle>,
}

/// The finite states the UI moves through.
pub enum AppState {
    /// Boot in progress: opening the store and loading the first snapshot.
    Loading,
    /// Core reachable. Holds the latest status and persona list.
    Running {
        status: Status,
        personas: Vec<SyntheticPersona>,
        /// `true` while a refresh is in flight, so the button can show a label
        /// and avoid stacking redundant loads.
        refreshing: bool,
    },
    /// The cross-device "Devices" view: pairing QR, peer lists, and the
    /// coordination-mode control. Reached from [`AppState::Running`] and
    /// returns to it. `snapshot` is `None` until the first load resolves.
    Devices {
        /// The loaded sync snapshot, or `None` while the first load is pending.
        snapshot: Option<DevicesSnapshot>,
        /// `true` while a Devices reload, mode change, or unpair is in flight,
        /// so the view can show progress and coalesce redundant work.
        busy: bool,
    },
    /// The C4 #20 A1 efficacy dashboard: per-platform KL-divergence drift
    /// timelines and a per-category heatmap. Reached from [`AppState::Running`].
    /// `snapshot` is `None` until the first measurement load resolves.
    Dashboard {
        /// The loaded drift snapshot, or `None` while the first load is pending.
        snapshot: Option<DashboardSnapshot>,
        /// The index into `per_platform` whose heatmap is being inspected.
        selected_platform: usize,
        /// The persona/device whose drift is shown (#20 per-device filter), or
        /// `None` to use the primary persona (the default).
        selected_device: Option<String>,
        /// `true` while a reload or platform switch is in flight.
        busy: bool,
    },
    /// The C5 persona studio: editor (#24 P1), linter (#25 P2), week simulator
    /// (#26 P3), and library (#27 P4). Reached from [`AppState::Running`].
    Studio {
        /// The loaded studio snapshot, or `None` while the first load pends.
        /// Boxed: it is the largest state payload, so boxing keeps `AppState`
        /// (and so the whole UI tree's state slot) compact.
        snapshot: Option<Box<StudioSnapshot>>,
        /// `true` while a reload, save, settings change, or pack op is running.
        busy: bool,
    },
    /// The C4 #22 A3 broker-diff view: a per-broker, time-ordered field-level
    /// diff for the selected `(broker, persona)`. Reached from
    /// [`AppState::Running`]. `snapshot` is `None` until the first load resolves.
    Brokers {
        /// The loaded broker-diff snapshot, or `None` while the first load pends.
        /// Boxed: the diff timeline (every field delta across consecutive
        /// snapshots) is a large payload, so boxing keeps `AppState` compact.
        snapshot: Option<Box<BrokerDiffSnapshot>>,
        /// `true` while a reload or selection change is in flight.
        busy: bool,
    },
    /// The C8 #33 U2 campaign panel: list campaigns, create one, start/pause,
    /// show progress. Reached from [`AppState::Running`].
    Campaigns {
        /// The loaded campaign snapshot, or `None` while the first load pends.
        snapshot: Option<CampaignsSnapshot>,
        /// The new-campaign draft form (kept across edits).
        draft: CampaignDraft,
        /// `true` while a reload, create, start, or pause is in flight.
        busy: bool,
    },
    /// The C7 #30/#31 egress + DNS panel: per-persona egress + DNS-strategy
    /// config, the egress exit indicator, and the DNS observer trade-off note.
    /// Reached from [`AppState::Running`].
    Network {
        /// The loaded network snapshot, or `None` while the first load pends.
        snapshot: Option<NetworkSnapshot>,
        /// `true` while a reload, selection change, or write is in flight.
        busy: bool,
    },
    /// The C3 privacy hub: a single screen with tabs over DSAR requests +
    /// deadlines (#16), email aliases (#17), per-site GPC honoring (#18), and the
    /// account-anchor map (#19). Read-only surfaces over data the core already
    /// holds. Reached from [`AppState::Running`]. `snapshot` is `None` until the
    /// first load resolves.
    Privacy {
        /// The loaded privacy snapshot, or `None` while the first load pends.
        /// Boxed: it aggregates four lists, so boxing keeps `AppState` compact.
        snapshot: Option<Box<PrivacySnapshot>>,
        /// Which privacy tab is showing.
        tab: PrivacyTab,
        /// `true` while a reload is in flight.
        busy: bool,
    },
    /// The C8 #34 U3 skippable first-run wizard. Its key step is QR persona
    /// import from the phone. Reached only on first run; completing or skipping
    /// it records the first-run-completed flag and lands in Running.
    Wizard {
        /// Which step is showing.
        step: WizardStep,
        /// The pairing-payload text the user pasted/scanned in the import step.
        payload: String,
        /// A short status line (last import result), shown under the import box.
        import_note: Option<String>,
        /// `true` while a pairing import is in flight.
        busy: bool,
    },
    /// The app + device Settings screen: the appearance/behavior prefs (theme,
    /// auto-refresh, close-to-tray) and the device/sync prefs (device name,
    /// LAN-sync, port) applied at the next start. `draft` is the in-progress edit
    /// buffer; Save persists it and copies it into [`App::prefs`].
    Settings {
        /// The edit buffer the form mutates locally before Save.
        draft: DesktopSettings,
        /// The raw text buffer for the sync-port field, so partial edits (while
        /// typing a port) are allowed; parsed to `Option<u16>` on Save (empty =
        /// core default, invalid = a surfaced error that aborts the save).
        port_text: String,
        /// `true` while a save is in flight.
        busy: bool,
    },
    /// The in-app Help / FAQ screen: static, scrollable reference content. Holds
    /// no payload (it makes no core call) and returns to Running.
    Faq,
    /// Unrecoverable boot error. The window renders the message; the user can
    /// quit from the tray. A store that fails to open lands here (we render an
    /// error rather than panicking, per the task spec).
    Error(String),
}

/// The tabs of the C3 privacy hub (#16/#17/#18/#19). `Dsar` is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrivacyTab {
    /// DSAR requests + deadline tracking (#16).
    #[default]
    Dsar,
    /// Email-alias inventory (#17).
    Aliases,
    /// Per-site GPC honoring (#18).
    Gpc,
    /// The account-anchor map (#19).
    Anchors,
}

impl PrivacyTab {
    /// The tab's short display label.
    pub fn label(self) -> &'static str {
        match self {
            PrivacyTab::Dsar => "DSAR",
            PrivacyTab::Aliases => "Aliases",
            PrivacyTab::Gpc => "GPC",
            PrivacyTab::Anchors => "Anchors",
        }
    }

    /// All tabs in display order, for rendering the tab selector.
    pub fn all() -> [PrivacyTab; 4] {
        [
            PrivacyTab::Dsar,
            PrivacyTab::Aliases,
            PrivacyTab::Gpc,
            PrivacyTab::Anchors,
        ]
    }
}

/// The ordered steps of the first-run wizard (C8 #34 U3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStep {
    /// A short welcome / what-this-is step.
    Welcome,
    /// The key step: import a persona from the phone by pairing-QR payload.
    ImportPhonePersona,
    /// A closing step confirming setup is done.
    Done,
}

impl WizardStep {
    /// The next step, or `None` when already at the last step.
    pub fn next(self) -> Option<Self> {
        match self {
            WizardStep::Welcome => Some(WizardStep::ImportPhonePersona),
            WizardStep::ImportPhonePersona => Some(WizardStep::Done),
            WizardStep::Done => None,
        }
    }

    /// The previous step, or `None` when already at the first step.
    pub fn previous(self) -> Option<Self> {
        match self {
            WizardStep::Welcome => None,
            WizardStep::ImportPhonePersona => Some(WizardStep::Welcome),
            WizardStep::Done => Some(WizardStep::ImportPhonePersona),
        }
    }
}

impl App {
    /// Construct the initial app in [`AppState::Loading`]. Loads the GUI-local
    /// settings up front so the theme and tick cadence apply from the first
    /// frame (a missing or malformed file falls back to defaults).
    pub fn new(core: Core, tray: Option<TrayHandle>) -> Self {
        Self {
            core,
            prefs: crate::prefs::load(),
            state: AppState::Loading,
            error_banner: None,
            tray,
        }
    }

    /// The active iced theme, derived from the user's saved theme choice. Passed
    /// to iced via `.theme(App::theme)`.
    pub fn theme(&self) -> iced::Theme {
        self.prefs.theme.to_theme()
    }

    /// Window title. A `&App -> String` fn, as iced's `.title()` expects.
    pub fn title(&self) -> String {
        match &self.state {
            AppState::Loading => "Fauxx (starting)".to_string(),
            AppState::Running { status, .. } => {
                format!("Fauxx ({} personas)", status.persona_count)
            }
            AppState::Devices { .. } => "Fauxx (devices)".to_string(),
            AppState::Dashboard { .. } => "Fauxx (efficacy dashboard)".to_string(),
            AppState::Studio { .. } => "Fauxx (persona studio)".to_string(),
            AppState::Brokers { .. } => "Fauxx (broker diff)".to_string(),
            AppState::Campaigns { .. } => "Fauxx (campaigns)".to_string(),
            AppState::Network { .. } => "Fauxx (egress and DNS)".to_string(),
            AppState::Privacy { .. } => "Fauxx (privacy)".to_string(),
            AppState::Settings { .. } => "Fauxx (settings)".to_string(),
            AppState::Faq => "Fauxx (help)".to_string(),
            AppState::Wizard { .. } => "Fauxx (setup)".to_string(),
            AppState::Error(_) => "Fauxx (error)".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_step_navigation_is_bounded() {
        // Forward to the end, then None.
        assert_eq!(
            WizardStep::Welcome.next(),
            Some(WizardStep::ImportPhonePersona)
        );
        assert_eq!(
            WizardStep::ImportPhonePersona.next(),
            Some(WizardStep::Done)
        );
        assert_eq!(WizardStep::Done.next(), None);
        // Backward to the start, then None.
        assert_eq!(
            WizardStep::Done.previous(),
            Some(WizardStep::ImportPhonePersona)
        );
        assert_eq!(
            WizardStep::ImportPhonePersona.previous(),
            Some(WizardStep::Welcome)
        );
        assert_eq!(WizardStep::Welcome.previous(), None);
    }

    #[test]
    fn title_reflects_the_current_state() {
        let app = App::new(Core::new(), None);
        // Default boot state.
        assert_eq!(app.title(), "Fauxx (starting)");
    }
}
