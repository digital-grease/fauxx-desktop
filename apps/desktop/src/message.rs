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

//! Message enum: every event that can drive a state transition in the iced
//! app. Pure data; [`crate::update::update`] interprets it. The enum is
//! `Clone` because iced 0.14 requires it, so every payload here is owned.

use fauxx_core::{
    BrokerDiffTimeline, Campaign, Comparator, CoordinationMode, DiscoveredPeer, DnsStrategy,
    Egress, EgressExit, Finding, InstalledPack, PairedPeer, PairingQr, PersonaField,
    PersonaSettings, PlatformDrift, RotationSchedule, SimulatedWeek, Status, SyntheticPersona,
};

use crate::prefs::ThemeChoice;

/// One thing that can happen in the app.
#[derive(Clone, Debug)]
pub enum Message {
    /// The background store-open finished. `Ok` carries the opened core handle
    /// (or the original store-less handle if opening was skipped) so we can
    /// transition out of `Loading`; `Err` carries a human-readable message and
    /// lands the app in `AppState::Error`.
    Booted(Result<BootOutcome, String>),
    /// User asked for a manual refresh, or the 2s tick fired.
    Tick,
    /// User clicked the Refresh button.
    Refresh,
    /// A status + persona-list load finished.
    Loaded(Result<Snapshot, String>),
    /// User opened the cross-device "Devices" view (from the Running screen).
    OpenDevices,
    /// User returned from the Devices view to the Running screen.
    CloseDevices,
    /// User asked the Devices view to reload its sync snapshot.
    RefreshDevices,
    /// A Devices snapshot (pairing QR, peers, mode) finished loading.
    DevicesLoaded(Result<DevicesSnapshot, String>),
    /// User picked a coordination mode in the Devices view. Carries the target
    /// mode; the result is reported back via [`Message::ModeSet`].
    SetMode(CoordinationMode),
    /// A `set_coordination_mode` call finished. `Ok` carries the now-active mode
    /// so the view reflects it; `Err` carries a message for the banner.
    ModeSet(Result<CoordinationMode, String>),
    /// User asked to unpair a peer, identified by its base64url public key.
    Unpair(String),
    /// An `unpair` call finished. `Err` carries a message for the banner; on
    /// success the Devices view is reloaded so the peer list updates.
    Unpaired(Result<(), String>),
    // --- C4 #20 A1 efficacy dashboard --------------------------------------
    /// User opened the efficacy "Dashboard" view (from the Running screen).
    OpenDashboard,
    /// User returned from the Dashboard to the Running screen.
    CloseDashboard,
    /// User asked the Dashboard to reload its drift snapshot.
    RefreshDashboard,
    /// A Dashboard snapshot (per-platform drift + combined aggregate) loaded.
    DashboardLoaded(Result<DashboardSnapshot, String>),
    /// User picked which platform's heatmap to inspect on the Dashboard.
    DashboardSelectPlatform(usize),
    /// User picked which persona/device's drift to view (#20 per-device filter),
    /// by persona id.
    DashboardSelectDevice(String),

    // --- C5 persona studio -------------------------------------------------
    /// User opened the persona "Studio" view (from the Running screen).
    OpenStudio,
    /// User returned from the Studio to the Running screen.
    CloseStudio,
    /// User asked the Studio to reload its persona library + per-persona detail.
    RefreshStudio,
    /// A Studio snapshot (library packs + selected-persona detail) loaded.
    /// Boxed because the studio snapshot (personas + pack ledger + per-persona
    /// detail) is by far the largest payload, so boxing keeps `Message` compact.
    StudioLoaded(Result<Box<StudioSnapshot>, String>),
    /// User selected a persona to edit in the Studio (by id).
    StudioSelectPersona(String),
    /// User edited one freeform text field of the selected persona.
    StudioEditField(PersonaTextField, String),
    /// User picked a new value for one enum-typed identity field (age range,
    /// profession, region) from its dropdown. Carries the field and the chosen
    /// enum NAME (e.g. `"AGE_35_44"`). A local edit of the in-memory buffer.
    StudioSetEnumField(PersonaEnumField, String),
    /// User toggled membership of one interest category (a `CategoryPool` name)
    /// in the selected persona's interest set. A local edit of the in-memory
    /// buffer; the 3..=5 count rule is enforced in `update` (a toggle that would
    /// break the bounds is refused with a banner note).
    StudioToggleInterest(String),
    /// User pressed Save on the edited persona.
    StudioSavePersona,
    /// A persona save finished. On success the Studio reloads.
    StudioPersonaSaved(Result<(), String>),
    /// User toggled the lock on one persona field.
    StudioToggleLock(PersonaField, bool),
    /// User chose a rotation schedule (enabled cadence vs pinned).
    StudioSetRotation(RotationSchedule),
    /// A settings change (lock toggle or rotation) finished, carrying the
    /// updated [`PersonaSettings`] on success.
    StudioSettingsSaved(Result<PersonaSettings, String>),
    /// User pressed "Re-roll" on the week simulator (new seed).
    StudioRerollWeek,
    /// User asked to import a persona pack (opens a native file dialog).
    StudioImportPack,
    /// User asked to export the selected persona as a signed pack.
    StudioExportPack,
    /// User asked to remove an installed pack from the library, by pack id.
    StudioRemovePack(String),
    /// A pack import/export/remove finished. `Ok` carries a short human summary;
    /// the Studio reloads on success.
    StudioPackDone(Result<String, String>),

    // --- C4 #22 A3 broker-diff view ----------------------------------------
    /// User opened the broker-diff view (from the Running screen).
    OpenBrokers,
    /// User returned from the broker-diff view to the Running screen.
    CloseBrokers,
    /// User asked the broker-diff view to reload its timeline.
    RefreshBrokers,
    /// A broker-diff snapshot finished loading. Boxed so the timeline payload
    /// (every consecutive-snapshot field delta) keeps `Message` compact.
    BrokersLoaded(Result<Box<BrokerDiffSnapshot>, String>),
    /// User picked which persona to inspect broker diffs for (by id).
    BrokersSelectPersona(String),
    /// User picked which broker's timeline to inspect (by id).
    BrokersSelectBroker(String),

    // --- C8 #33 U2 campaign panel ------------------------------------------
    /// User opened the campaign panel (from the Running screen).
    OpenCampaigns,
    /// User returned from the campaign panel to the Running screen.
    CloseCampaigns,
    /// User asked the campaign panel to reload its campaign list.
    RefreshCampaigns,
    /// A campaign snapshot (list + draft form context) finished loading.
    CampaignsLoaded(Result<CampaignsSnapshot, String>),
    /// User edited the new-campaign draft label.
    CampaignDraftLabel(String),
    /// User picked the new-campaign persona (by id).
    CampaignDraftPersona(String),
    /// User picked the new-campaign target segment (a `CategoryPool` name).
    CampaignDraftSegment(String),
    /// User picked the new-campaign goal comparator.
    CampaignDraftComparator(Comparator),
    /// User edited the new-campaign goal threshold (raw text, parsed on submit).
    CampaignDraftThreshold(String),
    /// User submitted the new-campaign draft to create it.
    CampaignCreate,
    /// User pressed Start/Resume on a campaign (by id).
    CampaignStart(String),
    /// User pressed Pause on a campaign (by id).
    CampaignPause(String),
    /// A campaign create/start/pause finished. `Ok` carries a short summary; the
    /// panel reloads on success.
    CampaignActionDone(Result<String, String>),

    // --- C7 #30/#31 egress + DNS panel -------------------------------------
    /// User opened the egress/DNS panel (from the Running screen).
    OpenNetwork,
    /// User returned from the egress/DNS panel to the Running screen.
    CloseNetwork,
    /// User asked the egress/DNS panel to reload.
    RefreshNetwork,
    /// A network snapshot (per-persona egress + DNS + exit indicator) loaded.
    NetworkLoaded(Result<NetworkSnapshot, String>),
    /// User picked which persona to configure egress/DNS for (by id).
    NetworkSelectPersona(String),
    /// User chose an egress option for the selected persona.
    NetworkSetEgress(Egress),
    /// User chose a DNS strategy for the selected persona.
    NetworkSetDns(DnsStrategy),
    /// An egress/DNS write finished. On success the panel reloads.
    NetworkSaved(Result<(), String>),

    /// A tray menu item was activated, or the tray icon was clicked to show the
    /// window. See [`crate::tray`].
    Tray(TrayMessage),
    /// A tray quick control (Pause/Resume) finished. `Ok` carries a short status
    /// line for the banner; `Err` carries a failure message.
    QuickControlDone(Result<String, String>),

    // --- C8 #34 U3 first-run wizard ----------------------------------------
    /// User advanced the first-run wizard to the next step.
    WizardNext,
    /// User pressed Back in the first-run wizard.
    WizardBack,
    /// User skipped the wizard (still records first-run completed).
    WizardSkip,
    /// User finished the wizard. Records the first-run-completed flag and lands
    /// in Running.
    WizardFinish,
    /// User edited the QR pairing-payload text in the wizard import step.
    WizardEditPayload(String),
    /// User submitted the wizard's pairing payload to import the phone persona.
    WizardImportPayload,
    /// A wizard QR/pairing import finished. `Ok` carries a short summary.
    WizardImported(Result<String, String>),
    /// The first-run-completed flag finished persisting (best-effort). A pure
    /// acknowledgement; the parallel status reload lands the app in Running.
    FirstRunResolved,

    // --- Debug log export (the bug-report path) ----------------------------
    /// User asked to export a scrubbed copy of the debug logs for a bug report
    /// (opens a save dialog).
    ExportLogs,
    /// The log export finished. `Ok` carries a short summary to surface.
    LogsExported(Result<String, String>),

    // --- C3 privacy hub: DSAR / aliases / GPC / anchors --------------------
    /// User opened the Privacy hub (from the Running screen).
    OpenPrivacy,
    /// User returned from the Privacy hub to the Running screen.
    ClosePrivacy,
    /// User asked the Privacy hub to reload its snapshot.
    RefreshPrivacy,
    /// A Privacy snapshot (DSAR/alias/GPC/anchor rows) finished loading. Boxed:
    /// it aggregates four lists, the largest read-only payload here.
    PrivacyLoaded(Result<Box<PrivacySnapshot>, String>),
    /// User switched the active Privacy hub tab.
    SetPrivacyTab(crate::state::PrivacyTab),

    // --- Settings screen (app + device prefs) ------------------------------
    /// User opened the Settings screen (from the Running screen). Seeds the edit
    /// buffer from the live [`crate::prefs::DesktopSettings`].
    OpenSettings,
    /// User returned from Settings to the Running screen (discarding unsaved
    /// edits in the draft buffer).
    CloseSettings,
    /// User picked a theme in the Settings draft.
    SettingsSetTheme(ThemeChoice),
    /// User stepped the auto-refresh interval (seconds); the view sends the new
    /// value already clamped into the allowed range.
    SettingsSetAutoRefresh(u64),
    /// User toggled close-to-tray vs quit-on-close.
    SettingsToggleCloseToTray(bool),
    /// User edited the device name (blank means "let the core derive it").
    SettingsSetDeviceName(String),
    /// User toggled LAN sync on/off (applies at next start).
    SettingsToggleLanSync(bool),
    /// User edited the sync port (raw text; blank means the core default).
    SettingsSetSyncPort(String),
    /// User pressed Save: persist the draft and copy it into the live prefs.
    SettingsSave,
    /// A settings save finished. `Ok` carries a short confirmation for the
    /// banner; `Err` carries the failure message.
    SettingsSaved(Result<(), String>),

    // --- In-app Help / FAQ screen ------------------------------------------
    /// User opened the Help/FAQ screen (from the Running screen).
    OpenFaq,
    /// User returned from the Help/FAQ screen to the Running screen.
    CloseFaq,

    /// The window-manager close button was pressed. We hide rather than exit
    /// (close-to-tray). Carries the window id from `window::close_requests`.
    CloseRequested(iced::window::Id),
    /// Dismiss the non-fatal error banner.
    ErrorDismissed,
}

/// What a successful boot produced: the (possibly store-attached) core handle
/// plus the first status/persona snapshot, and whether this is the first run
/// (so the app shows the C8 #34 U3 wizard rather than Running).
#[derive(Clone, Debug)]
pub struct BootOutcome {
    pub core: fauxx_core::Core,
    pub snapshot: Snapshot,
    /// `true` on first run (the first-run-completed marker is absent), so the
    /// boot lands in the skippable wizard instead of Running.
    pub first_run: bool,
}

/// A point-in-time view of the core, loaded off the async API. Owned and
/// `Clone` so it can ride inside a `Message`.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub personas: Vec<SyntheticPersona>,
}

/// A point-in-time view of the cross-device sync surface, loaded off the async
/// API for the Devices view. Owned and `Clone` so it can ride inside a
/// `Message`. The pairing QR is `None` only when a store-less core could not
/// produce one (the view then shows guidance rather than a code).
#[derive(Clone, Debug)]
pub struct DevicesSnapshot {
    /// This device's pairing QR (unicode + SVG forms) and fingerprint.
    pub pairing_qr: Option<PairingQr>,
    /// This device's pairing fingerprint, surfaced even when the QR is absent.
    pub fingerprint: Option<String>,
    /// Trusted, paired peers.
    pub paired: Vec<PairedPeer>,
    /// Peers seen over LAN discovery (untrusted until paired).
    pub discovered: Vec<DiscoveredPeer>,
    /// The active household coordination mode.
    pub mode: CoordinationMode,
}

/// A point-in-time view of the C4 #20 A1 efficacy dashboard, loaded off the
/// measurement API. Owned and `Clone` so it can ride inside a `Message`.
///
/// `per_platform` is the all-devices (single persona) per-platform drift bundle
/// in display order (Google, Brokers, Meta); `combined` is the cross-device
/// aggregate for the platform the user is inspecting (degrades to single-device
/// and to no-data without panicking). An empty `per_platform` is the well-formed
/// no-data state (no store, or no persona/read-backs yet).
#[derive(Clone, Debug)]
pub struct DashboardSnapshot {
    /// The persona id this dashboard was built for, or `None` for the no-data
    /// (no persona available) state.
    pub persona_id: Option<String>,
    /// How many personas (devices) fed the combined aggregate. `1` is the
    /// single-device fallback; `0` is the no-data state.
    pub device_count: usize,
    /// Per-platform drift bundles in display order (Google, Brokers, Meta).
    pub per_platform: Vec<PlatformDrift>,
    /// The cross-device combined drift bundle for the inspected platform.
    pub combined: PlatformDrift,
    /// The selectable personas/devices `(id, name)` for the #20 per-device
    /// filter, in stored order. The currently-shown one is [`Self::persona_id`].
    pub devices: Vec<(String, String)>,
}

/// A point-in-time view of the C5 persona studio, loaded off the persona +
/// studio APIs. Owned and `Clone` so it can ride inside a `Message`.
#[derive(Clone, Debug)]
pub struct StudioSnapshot {
    /// Every stored persona (the library list / editor target set).
    pub personas: Vec<SyntheticPersona>,
    /// The installed persona packs (the #27 P4 library ledger), newest first.
    pub installed_packs: Vec<InstalledPack>,
    /// The detail for the currently-selected persona, if one is selected.
    pub detail: Option<PersonaDetail>,
}

/// The studio's per-persona working detail: the editor settings, the linter
/// findings (#25 P2), and the week-simulator preview (#26 P3).
#[derive(Clone, Debug)]
pub struct PersonaDetail {
    /// The selected persona (the edit buffer the editor mutates locally).
    pub persona: SyntheticPersona,
    /// The locked-field + rotation settings (#24 P1).
    pub settings: PersonaSettings,
    /// The coherence-linter findings for the persona (#25 P2).
    pub findings: Vec<Finding>,
    /// The simulated week preview (#26 P3) for the current re-roll seed.
    pub week: SimulatedWeek,
    /// The seed the preview week was rolled with (re-rollable).
    pub seed: u64,
}

/// A point-in-time view of the C4 #22 A3 broker-diff surface. Owned and `Clone`
/// so it can ride inside a `Message`. The `timeline` is the diff for the
/// currently-selected `(broker, persona)`; `personas` and `brokers` populate the
/// two selectors. An empty `personas` is the well-formed no-persona state.
#[derive(Clone, Debug)]
pub struct BrokerDiffSnapshot {
    /// The persona ids (and display names) available to inspect.
    pub personas: Vec<(String, String)>,
    /// The broker `(id, display_name)` registry entries available to inspect.
    pub brokers: Vec<(String, String)>,
    /// The selected persona id, or `None` when no persona exists.
    pub selected_persona: Option<String>,
    /// The selected broker id.
    pub selected_broker: String,
    /// The diff timeline for the current `(broker, persona)`, or `None` while no
    /// persona is selectable.
    pub timeline: Option<BrokerDiffTimeline>,
}

/// A point-in-time view of the C8 #33 U2 campaign panel. Owned and `Clone`.
#[derive(Clone, Debug)]
pub struct CampaignsSnapshot {
    /// Every campaign, most-recently-updated first.
    pub campaigns: Vec<Campaign>,
    /// The personas a new campaign can target (id + display name).
    pub personas: Vec<(String, String)>,
}

/// The new-campaign draft form state, kept in the panel's [`crate::state`] slot
/// (not in a `Message`) so the user can fill it across several edits.
#[derive(Clone, Debug)]
pub struct CampaignDraft {
    /// The campaign label.
    pub label: String,
    /// The targeted persona id (empty until picked).
    pub persona_id: String,
    /// The target segment (`CategoryPool` name, empty until picked).
    pub segment: String,
    /// The goal comparator.
    pub comparator: Comparator,
    /// The raw goal-threshold text (parsed to `f64` on submit).
    pub threshold: String,
}

impl Default for CampaignDraft {
    fn default() -> Self {
        Self {
            label: String::new(),
            persona_id: String::new(),
            segment: String::new(),
            comparator: Comparator::AtLeast,
            threshold: "0.5".to_string(),
        }
    }
}

/// A point-in-time view of the C7 #30/#31 egress + DNS panel. Owned and `Clone`.
#[derive(Clone, Debug)]
pub struct NetworkSnapshot {
    /// The personas available to configure (id + display name).
    pub personas: Vec<(String, String)>,
    /// The selected persona id, or `None` when no persona exists.
    pub selected_persona: Option<String>,
    /// The selected persona's bound egress (defaults to `Direct`).
    pub egress: Egress,
    /// The selected persona's bound DNS strategy (defaults to `SystemDefault`).
    pub dns: DnsStrategy,
    /// The exit indicator (label + reachable/paused), computed with the static
    /// reachable seam so the panel needs no live network call.
    pub exit: Option<EgressExit>,
    /// The explicit DNS observer trade-off note for the current strategy.
    pub dns_note: String,
}

/// The freeform (string-typed) persona fields the editor lets the user type
/// into directly. The enum-typed fields (age range, profession, region) use a
/// pick list ([`PersonaEnumField`]) and the interest set uses a multi-select
/// toggle, so they are not part of this typed-text channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaTextField {
    /// The display name.
    Name,
    /// The desktop-only home location label.
    HomeLocation,
    /// The desktop-only schedule label.
    Schedule,
    /// The desktop-only browsing-style label.
    BrowsingStyle,
}

/// The enum-typed identity persona fields the editor edits through a dropdown
/// picker (C5 P1). The picker options come from the core enum `all()` lists
/// ([`fauxx_core::AgeRange`] / [`fauxx_core::Profession`] / [`fauxx_core::Region`]);
/// the chosen variant is stored as its wire NAME on the persona buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaEnumField {
    /// The age bracket (`ageRange` wire field).
    AgeRange,
    /// The profession (`profession` wire field).
    Profession,
    /// The coarse region (`region` wire field).
    Region,
}

/// Tray-originated intents, decoded from the tray's global menu/click channels
/// into plain data before they reach `update`.
#[derive(Clone, Debug)]
pub enum TrayMessage {
    /// Show / focus the main window.
    OpenWindow,
    /// User picked "Status" in the tray menu: refresh and show the window.
    ShowStatus,
    /// User picked "Pause": pause all running campaigns (quick control, #34 U3).
    Pause,
    /// User picked "Resume": resume all paused campaigns (quick control).
    Resume,
    /// User picked "Quit": exit the whole agent.
    Quit,
}

/// A point-in-time view of the C3 privacy surfaces for the Privacy hub: DSAR
/// requests + deadlines (#16), email aliases (#17), per-site GPC honoring (#18),
/// and account anchors (#19). The rows are PRE-FORMATTED into display strings in
/// [`crate::bg`] so the view is a dumb renderer (the enum/deadline logic stays
/// testable in `bg`, off the UI). Owned + `Clone` to ride inside a `Message`.
#[derive(Clone, Debug, Default)]
pub struct PrivacySnapshot {
    /// DSAR requests with a deadline indicator (#16).
    pub dsar: Vec<DsarRow>,
    /// Email aliases / masked addresses (#17).
    pub aliases: Vec<AliasRow>,
    /// Per-site GPC honoring observations (#18).
    pub gpc: Vec<GpcRow>,
    /// Account anchors and their linkage (#19).
    pub anchors: Vec<AnchorRow>,
    /// Prioritized account-anchor partitioning recommendations (#19): what to
    /// do, ranked by linkage score, so the hub surfaces the analysis, not just
    /// the raw inventory.
    pub anchor_recommendations: Vec<AnchorRecommendationRow>,
}

/// One DSAR request row (#16): who, what, where it is, and its deadline state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsarRow {
    /// The controller's display name.
    pub controller: String,
    /// The request kind label (e.g. "Access", "Erasure").
    pub kind: String,
    /// The lifecycle status (e.g. "drafted", "sent").
    pub status: String,
    /// A deadline indicator: "not sent", "on track", "due soon", or "overdue".
    pub deadline: String,
    /// `true` past the statutory deadline (the view flags it).
    pub overdue: bool,
}

/// One email-alias row (#17).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasRow {
    /// The site the alias was minted for.
    pub site: String,
    /// The alias address.
    pub address: String,
    /// The alias kind label.
    pub kind: String,
    /// The alias status label (e.g. "active", "revoked").
    pub status: String,
}

/// One per-site GPC honoring row (#18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpcRow {
    /// The site origin observed.
    pub origin: String,
    /// Whether the site advertised honoring Global Privacy Control.
    pub honored: bool,
}

/// One account-anchor row (#19): a real identity touchpoint and its linkage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorRow {
    /// The anchor's display label.
    pub label: String,
    /// The site the anchor lives on.
    pub site: String,
    /// How many identity signals this anchor carries.
    pub signals: usize,
    /// `true` if it shares a contact key with other anchors (a linkage edge in
    /// the anchor map).
    pub linked: bool,
}

/// One prioritized anchor partitioning recommendation row (#19): what to do
/// about a high-linkage anchor, with its score and rationale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorRecommendationRow {
    /// The account label the recommendation targets.
    pub label: String,
    /// The recommended action (e.g. "separate alias", "isolate high anchor").
    pub action: String,
    /// The anchor's linkage score that ranked this recommendation.
    pub score: u32,
    /// A human-readable rationale.
    pub rationale: String,
}
