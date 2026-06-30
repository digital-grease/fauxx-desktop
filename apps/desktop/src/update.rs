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

//! `update`: the pure state-transition function.
//!
//! It interprets a [`Message`] into a state mutation plus an optional
//! [`Task`]. The only "work" it does is translate Messages into `fauxx-core`
//! calls (dispatched through [`crate::bg`]) and core results into view state;
//! all real logic stays in the core (the thin-client rule).

use iced::window;
use iced::Task;

use crate::bg;
use crate::message::{Message, PersonaEnumField, PersonaTextField, TrayMessage};
use crate::state::{App, AppState, PrivacyTab, WizardStep};

/// The inclusive interest-count rule a well-formed persona carries (mirrors
/// `fauxx_core::persona::INTEREST_COUNT`, which is not re-exported at the crate
/// root). The interest multi-select editor enforces this so a save never drops
/// below 3 or climbs above 5.
const INTEREST_COUNT: std::ops::RangeInclusive<usize> = 3..=5;

pub fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Booted(result) => match result {
            Ok(outcome) => {
                app.core = outcome.core;
                // Bring up live LAN sync for the app's lifetime (C1 #7 / #34):
                // advertise this device over mDNS and run the inbound listener so a
                // paired phone's sealed persona frames are received and persisted
                // into the shared store. Detached; it ends when the process exits.
                let _ = app.core.spawn_background_lan_sync();
                if outcome.first_run {
                    // First launch: show the skippable C8 #34 U3 wizard. Its key
                    // step imports a persona from the phone by pairing QR.
                    app.state = AppState::Wizard {
                        step: WizardStep::Welcome,
                        payload: String::new(),
                        import_note: None,
                        busy: false,
                    };
                } else {
                    app.state = AppState::Running {
                        status: outcome.snapshot.status,
                        personas: outcome.snapshot.personas,
                        refreshing: false,
                    };
                }
                Task::none()
            }
            Err(err) => {
                app.state = AppState::Error(err);
                Task::none()
            }
        },

        Message::Tick | Message::Refresh => {
            let AppState::Running { refreshing, .. } = &mut app.state else {
                return Task::none();
            };
            // Coalesce: skip if a load is already in flight.
            if *refreshing {
                return Task::none();
            }
            *refreshing = true;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::Loaded(result) => {
            match result {
                Ok(snapshot) => {
                    app.state = AppState::Running {
                        status: snapshot.status,
                        personas: snapshot.personas,
                        refreshing: false,
                    };
                }
                Err(err) => match &mut app.state {
                    // A failed refresh from Running is non-fatal: keep the last
                    // good view, surface the error in the dismissable banner,
                    // and clear the in-flight flag so the next tick can retry.
                    AppState::Running { refreshing, .. } => {
                        *refreshing = false;
                        app.error_banner = Some(format!("Refresh failed: {err}"));
                    }
                    // A failure during the post-navigation reload (state is
                    // Loading) leaves no good snapshot to keep, so surface it
                    // as the terminal error rather than stranding in Loading.
                    _ => app.state = AppState::Error(err),
                },
            }
            Task::none()
        }

        Message::OpenDevices => {
            // Switch to the Devices screen and kick off the first sync load.
            app.state = AppState::Devices {
                snapshot: None,
                busy: true,
                pair_back_input: String::new(),
                pair_back_note: None,
            };
            Task::perform(bg::load_devices(app.core.clone()), Message::DevicesLoaded)
        }

        Message::CloseDevices => {
            // Return to the Running screen. Reuse the Loading state while the
            // snapshot reloads; `Message::Loaded` lands the app back in Running
            // (so we never fabricate a placeholder Status here).
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshDevices => {
            let AppState::Devices { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(bg::load_devices(app.core.clone()), Message::DevicesLoaded)
        }

        Message::DevicesLoaded(result) => {
            let AppState::Devices { snapshot, busy, .. } = &mut app.state else {
                // The user navigated away before the load resolved; drop it.
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Devices load failed: {err}")),
            }
            Task::none()
        }

        Message::SetMode(mode) => {
            let AppState::Devices { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(bg::set_mode(app.core.clone(), mode), Message::ModeSet)
        }

        Message::ModeSet(result) => {
            let AppState::Devices { snapshot, busy, .. } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(mode) => {
                    if let Some(snapshot) = snapshot {
                        snapshot.mode = mode;
                    }
                }
                Err(err) => app.error_banner = Some(format!("Mode change failed: {err}")),
            }
            Task::none()
        }

        Message::Unpair(public_key) => {
            let AppState::Devices { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(bg::unpair(app.core.clone(), public_key), Message::Unpaired)
        }

        Message::Unpaired(result) => {
            let AppState::Devices { busy, .. } = &mut app.state else {
                return Task::none();
            };
            match result {
                // Reload so the peer list reflects the removal. `busy` stays
                // set until that reload resolves.
                Ok(()) => Task::perform(bg::load_devices(app.core.clone()), Message::DevicesLoaded),
                Err(err) => {
                    *busy = false;
                    app.error_banner = Some(format!("Unpair failed: {err}"));
                    Task::none()
                }
            }
        }

        Message::DevicePairBackChanged(text) => {
            if let AppState::Devices {
                pair_back_input, ..
            } = &mut app.state
            {
                *pair_back_input = text;
            }
            Task::none()
        }

        Message::DevicePairBack => {
            let AppState::Devices {
                busy,
                pair_back_input,
                pair_back_note,
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let payload = pair_back_input.trim().to_string();
            if payload.is_empty() {
                *pair_back_note =
                    Some("Paste the pairing code from the other device first.".to_string());
                return Task::none();
            }
            *busy = true;
            *pair_back_note = None;
            Task::perform(
                bg::pair_back(app.core.clone(), payload),
                Message::DevicePairedBack,
            )
        }

        Message::DevicePairedBack(result) => {
            let AppState::Devices {
                busy,
                pair_back_input,
                pair_back_note,
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            match result {
                // Clear the input and reload so the newly paired device shows in
                // the peer list. `busy` stays set until that reload resolves.
                Ok(note) => {
                    *pair_back_input = String::new();
                    *pair_back_note = Some(note);
                    Task::perform(bg::load_devices(app.core.clone()), Message::DevicesLoaded)
                }
                Err(err) => {
                    *busy = false;
                    *pair_back_note = Some(format!("Pairing failed: {err}"));
                    Task::none()
                }
            }
        }

        // --- C4 #20 A1 efficacy dashboard ----------------------------------
        Message::OpenDashboard => {
            app.state = AppState::Dashboard {
                snapshot: None,
                selected_platform: 0,
                selected_device: None,
                busy: true,
            };
            Task::perform(
                bg::load_dashboard(app.core.clone(), 0, None),
                Message::DashboardLoaded,
            )
        }

        Message::CloseDashboard => {
            // Mirror the Devices close: bounce through Loading and reload Running.
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshDashboard => {
            let AppState::Dashboard {
                busy,
                selected_platform,
                selected_device,
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let index = *selected_platform;
            let device = selected_device.clone();
            Task::perform(
                bg::load_dashboard(app.core.clone(), index, device),
                Message::DashboardLoaded,
            )
        }

        Message::DashboardSelectPlatform(index) => {
            let AppState::Dashboard {
                busy,
                selected_platform,
                selected_device,
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy || *selected_platform == index {
                return Task::none();
            }
            *selected_platform = index;
            *busy = true;
            let device = selected_device.clone();
            // Reload so the combined (cross-device) bundle matches the new
            // platform; the per-platform timeline is already loaded.
            Task::perform(
                bg::load_dashboard(app.core.clone(), index, device),
                Message::DashboardLoaded,
            )
        }

        Message::DashboardSelectDevice(persona_id) => {
            let AppState::Dashboard {
                busy,
                selected_platform,
                selected_device,
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            // Already showing this device, or a load in flight: nothing to do.
            if *busy || selected_device.as_deref() == Some(persona_id.as_str()) {
                return Task::none();
            }
            *selected_device = Some(persona_id.clone());
            *busy = true;
            let index = *selected_platform;
            Task::perform(
                bg::load_dashboard(app.core.clone(), index, Some(persona_id)),
                Message::DashboardLoaded,
            )
        }

        Message::DashboardLoaded(result) => {
            let AppState::Dashboard { snapshot, busy, .. } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Dashboard load failed: {err}")),
            }
            Task::none()
        }

        // --- C5 persona studio ---------------------------------------------
        Message::OpenStudio => {
            app.state = AppState::Studio {
                snapshot: None,
                busy: true,
            };
            Task::perform(
                bg::load_studio(app.core.clone(), None, default_week_seed()),
                Message::StudioLoaded,
            )
        }

        Message::CloseStudio => {
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshStudio => {
            let AppState::Studio { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let (selected, seed) = studio_selection(snapshot.as_deref());
            Task::perform(
                bg::load_studio(app.core.clone(), selected, seed),
                Message::StudioLoaded,
            )
        }

        Message::StudioLoaded(result) => {
            let AppState::Studio { snapshot, busy } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Studio load failed: {err}")),
            }
            Task::none()
        }

        Message::StudioSelectPersona(id) => {
            let AppState::Studio { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::load_studio(app.core.clone(), Some(id), default_week_seed()),
                Message::StudioLoaded,
            )
        }

        Message::StudioEditField(field, value) => {
            // A local, pure edit of the in-memory persona buffer; no core call
            // until Save. The thin-client rule is preserved: this is view-state
            // mutation, not business logic.
            if let AppState::Studio {
                snapshot: Some(snapshot),
                ..
            } = &mut app.state
            {
                if let Some(detail) = &mut snapshot.detail {
                    apply_text_edit(&mut detail.persona, field, value);
                }
            }
            Task::none()
        }

        Message::StudioSetEnumField(field, name) => {
            // A local, pure edit of the in-memory persona buffer (the chosen
            // enum NAME), mirroring StudioEditField; no core call until Save.
            if let AppState::Studio {
                snapshot: Some(snapshot),
                ..
            } = &mut app.state
            {
                if let Some(detail) = &mut snapshot.detail {
                    apply_enum_edit(&mut detail.persona, field, name);
                }
            }
            Task::none()
        }

        Message::StudioToggleInterest(name) => {
            // A local, pure edit toggling one interest's membership, enforcing
            // the 3..=5 count rule. A toggle that would break the bounds is
            // refused with a banner note (no core call until Save).
            let AppState::Studio {
                snapshot: Some(snapshot),
                ..
            } = &mut app.state
            else {
                return Task::none();
            };
            let Some(detail) = &mut snapshot.detail else {
                return Task::none();
            };
            if let Err(reason) = toggle_interest(&mut detail.persona, &name) {
                app.error_banner = Some(reason);
            }
            Task::none()
        }

        Message::StudioSavePersona => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(detail) = &snapshot.detail else {
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::save_persona(app.core.clone(), detail.persona.clone()),
                Message::StudioPersonaSaved,
            )
        }

        Message::StudioPersonaSaved(result) => {
            let AppState::Studio { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            match result {
                Ok(()) => {
                    // Reload so the linter + library reflect the saved persona.
                    let (selected, seed) = studio_selection(snapshot.as_deref());
                    Task::perform(
                        bg::load_studio(app.core.clone(), selected, seed),
                        Message::StudioLoaded,
                    )
                }
                Err(err) => {
                    *busy = false;
                    app.error_banner = Some(format!("Save failed: {err}"));
                    Task::none()
                }
            }
        }

        Message::StudioToggleLock(field, locked) => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(detail) = &snapshot.detail else {
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::set_field_locked(app.core.clone(), detail.persona.id.clone(), field, locked),
                Message::StudioSettingsSaved,
            )
        }

        Message::StudioSetRotation(rotation) => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(detail) = &snapshot.detail else {
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::set_rotation(app.core.clone(), detail.persona.id.clone(), rotation),
                Message::StudioSettingsSaved,
            )
        }

        Message::StudioSettingsSaved(result) => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(settings) => {
                    if let Some(detail) = &mut snapshot.detail {
                        detail.settings = settings;
                    }
                }
                Err(err) => app.error_banner = Some(format!("Settings change failed: {err}")),
            }
            Task::none()
        }

        Message::StudioRerollWeek => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(detail) = &snapshot.detail else {
                return Task::none();
            };
            // Re-roll with a fresh seed by reloading the selected persona detail.
            let id = detail.persona.id.clone();
            let seed = detail.seed.wrapping_add(1);
            *busy = true;
            Task::perform(
                bg::load_studio(app.core.clone(), Some(id), seed),
                Message::StudioLoaded,
            )
        }

        Message::StudioImportPack => {
            let AppState::Studio { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(bg::import_pack(app.core.clone()), Message::StudioPackDone)
        }

        Message::StudioExportPack => {
            let AppState::Studio {
                snapshot: Some(snapshot),
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(detail) = &snapshot.detail else {
                app.error_banner = Some("Select a persona to export first.".to_string());
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::export_pack(app.core.clone(), detail.persona.id.clone()),
                Message::StudioPackDone,
            )
        }

        Message::StudioRemovePack(pack_id) => {
            let AppState::Studio { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::remove_pack(app.core.clone(), pack_id),
                Message::StudioPackDone,
            )
        }

        Message::StudioPackDone(result) => {
            let AppState::Studio { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            match result {
                Ok(summary) => {
                    app.error_banner = Some(summary);
                    // Reload so an import shows the new personas + library entry.
                    let (selected, seed) = studio_selection(snapshot.as_deref());
                    Task::perform(
                        bg::load_studio(app.core.clone(), selected, seed),
                        Message::StudioLoaded,
                    )
                }
                Err(err) => {
                    *busy = false;
                    app.error_banner = Some(err);
                    Task::none()
                }
            }
        }

        // --- C4 #22 A3 broker-diff view ------------------------------------
        Message::OpenBrokers => {
            app.state = AppState::Brokers {
                snapshot: None,
                busy: true,
            };
            Task::perform(
                bg::load_brokers(app.core.clone(), None, None),
                Message::BrokersLoaded,
            )
        }

        Message::CloseBrokers => {
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshBrokers => {
            let AppState::Brokers { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let (persona, broker) = broker_selection(snapshot.as_deref());
            Task::perform(
                bg::load_brokers(app.core.clone(), persona, broker),
                Message::BrokersLoaded,
            )
        }

        Message::BrokersSelectPersona(id) => {
            let AppState::Brokers { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let (_, broker) = broker_selection(snapshot.as_deref());
            Task::perform(
                bg::load_brokers(app.core.clone(), Some(id), broker),
                Message::BrokersLoaded,
            )
        }

        Message::BrokersSelectBroker(id) => {
            let AppState::Brokers { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let (persona, _) = broker_selection(snapshot.as_deref());
            Task::perform(
                bg::load_brokers(app.core.clone(), persona, Some(id)),
                Message::BrokersLoaded,
            )
        }

        Message::BrokersLoaded(result) => {
            let AppState::Brokers { snapshot, busy } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Broker diff load failed: {err}")),
            }
            Task::none()
        }

        // --- C8 #33 U2 campaign panel --------------------------------------
        Message::OpenCampaigns => {
            app.state = AppState::Campaigns {
                snapshot: None,
                draft: crate::message::CampaignDraft::default(),
                busy: true,
            };
            Task::perform(
                bg::load_campaigns(app.core.clone()),
                Message::CampaignsLoaded,
            )
        }

        Message::CloseCampaigns => {
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshCampaigns => {
            let AppState::Campaigns { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::load_campaigns(app.core.clone()),
                Message::CampaignsLoaded,
            )
        }

        Message::CampaignsLoaded(result) => {
            let AppState::Campaigns { snapshot, busy, .. } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Campaign load failed: {err}")),
            }
            Task::none()
        }

        Message::CampaignDraftLabel(value) => {
            if let AppState::Campaigns { draft, .. } = &mut app.state {
                draft.label = value;
            }
            Task::none()
        }

        Message::CampaignDraftPersona(id) => {
            if let AppState::Campaigns { draft, .. } = &mut app.state {
                draft.persona_id = id;
            }
            Task::none()
        }

        Message::CampaignDraftSegment(seg) => {
            if let AppState::Campaigns { draft, .. } = &mut app.state {
                draft.segment = seg;
            }
            Task::none()
        }

        Message::CampaignDraftComparator(cmp) => {
            if let AppState::Campaigns { draft, .. } = &mut app.state {
                draft.comparator = cmp;
            }
            Task::none()
        }

        Message::CampaignDraftThreshold(value) => {
            if let AppState::Campaigns { draft, .. } = &mut app.state {
                draft.threshold = value;
            }
            Task::none()
        }

        Message::CampaignCreate => {
            let AppState::Campaigns { busy, draft, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::create_campaign(
                    app.core.clone(),
                    draft.label.clone(),
                    draft.persona_id.clone(),
                    draft.segment.clone(),
                    draft.comparator,
                    draft.threshold.clone(),
                ),
                Message::CampaignActionDone,
            )
        }

        Message::CampaignStart(id) => {
            let AppState::Campaigns { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::start_campaign(app.core.clone(), id),
                Message::CampaignActionDone,
            )
        }

        Message::CampaignPause(id) => {
            let AppState::Campaigns { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::pause_campaign(app.core.clone(), id),
                Message::CampaignActionDone,
            )
        }

        Message::CampaignActionDone(result) => {
            let AppState::Campaigns { busy, draft, .. } = &mut app.state else {
                return Task::none();
            };
            match result {
                Ok(summary) => {
                    app.error_banner = Some(summary);
                    // A create clears the draft so the form is ready for the next.
                    *draft = crate::message::CampaignDraft::default();
                    // Reload so the list + progress reflect the change. `busy`
                    // stays set until the reload resolves.
                    Task::perform(
                        bg::load_campaigns(app.core.clone()),
                        Message::CampaignsLoaded,
                    )
                }
                Err(err) => {
                    *busy = false;
                    app.error_banner = Some(err);
                    Task::none()
                }
            }
        }

        // --- C7 #30/#31 egress + DNS panel ---------------------------------
        Message::OpenNetwork => {
            app.state = AppState::Network {
                snapshot: None,
                busy: true,
            };
            Task::perform(
                bg::load_network(app.core.clone(), None),
                Message::NetworkLoaded,
            )
        }

        Message::CloseNetwork => {
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshNetwork => {
            let AppState::Network { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            let persona = network_selection(snapshot.as_ref());
            Task::perform(
                bg::load_network(app.core.clone(), persona),
                Message::NetworkLoaded,
            )
        }

        Message::NetworkSelectPersona(id) => {
            let AppState::Network { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::load_network(app.core.clone(), Some(id)),
                Message::NetworkLoaded,
            )
        }

        Message::NetworkSetEgress(egress) => {
            let AppState::Network { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(pid) = network_selection(snapshot.as_ref()) else {
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::set_egress(app.core.clone(), pid, egress),
                Message::NetworkSaved,
            )
        }

        Message::NetworkSetDns(dns) => {
            let AppState::Network { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            let Some(pid) = network_selection(snapshot.as_ref()) else {
                return Task::none();
            };
            *busy = true;
            Task::perform(
                bg::set_dns(app.core.clone(), pid, dns),
                Message::NetworkSaved,
            )
        }

        Message::NetworkSaved(result) => {
            let AppState::Network { busy, snapshot } = &mut app.state else {
                return Task::none();
            };
            match result {
                Ok(()) => {
                    // Reload so the exit indicator + observer note reflect the
                    // new config. `busy` stays set until the reload resolves.
                    let persona = network_selection(snapshot.as_ref());
                    Task::perform(
                        bg::load_network(app.core.clone(), persona),
                        Message::NetworkLoaded,
                    )
                }
                Err(err) => {
                    *busy = false;
                    app.error_banner = Some(format!("Network change failed: {err}"));
                    Task::none()
                }
            }
        }

        Message::NetworkLoaded(result) => {
            let AppState::Network { snapshot, busy } = &mut app.state else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Network load failed: {err}")),
            }
            Task::none()
        }

        Message::Tray(tray) => match tray {
            TrayMessage::OpenWindow => show_window(),
            TrayMessage::ShowStatus => {
                // Show the window and trigger an immediate refresh.
                let refresh = if matches!(app.state, AppState::Running { .. }) {
                    Task::perform(bg::load(app.core.clone()), Message::Loaded)
                } else {
                    Task::none()
                };
                Task::batch([show_window(), refresh])
            }
            TrayMessage::Pause => {
                // Quick control (#34 U3): pause all running campaigns off-thread.
                // The completion is folded back into the status banner.
                Task::perform(bg::pause_all(app.core.clone()), |r| {
                    Message::QuickControlDone(r.map(|()| "Paused all campaigns.".to_string()))
                })
            }
            TrayMessage::Resume => Task::perform(bg::resume_all(app.core.clone()), |r| {
                Message::QuickControlDone(r.map(|()| "Resumed all campaigns.".to_string()))
            }),
            TrayMessage::Quit => {
                // The real exit path. The window is hidden-or-shown either way;
                // exiting the iced runtime ends the process (the tray thread is
                // detached and dies with it).
                iced::exit()
            }
        },

        Message::QuickControlDone(result) => {
            match result {
                Ok(note) => app.error_banner = Some(note),
                Err(err) => app.error_banner = Some(format!("Quick control failed: {err}")),
            }
            Task::none()
        }

        // --- C8 #34 U3 first-run wizard ------------------------------------
        Message::WizardNext => {
            if let AppState::Wizard { step, .. } = &mut app.state {
                if let Some(next) = step.next() {
                    *step = next;
                }
            }
            Task::none()
        }

        Message::WizardBack => {
            if let AppState::Wizard { step, .. } = &mut app.state {
                if let Some(prev) = step.previous() {
                    *step = prev;
                }
            }
            Task::none()
        }

        Message::WizardEditPayload(value) => {
            if let AppState::Wizard { payload, .. } = &mut app.state {
                *payload = value;
            }
            Task::none()
        }

        Message::WizardImportPayload => {
            let AppState::Wizard { payload, busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(
                bg::import_phone_persona(app.core.clone(), payload.clone()),
                Message::WizardImported,
            )
        }

        Message::WizardImported(result) => {
            let AppState::Wizard {
                import_note, busy, ..
            } = &mut app.state
            else {
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(note) => *import_note = Some(note),
                Err(err) => *import_note = Some(format!("Import failed: {err}")),
            }
            Task::none()
        }

        Message::WizardSkip | Message::WizardFinish => {
            // Either path records the first-run-completed flag, then loads
            // Running. The flag write is best-effort (see firstrun.rs).
            app.state = AppState::Loading;
            let mark = Task::perform(bg::mark_first_run_complete(), |()| {
                Message::FirstRunResolved
            });
            let load = Task::perform(bg::load(app.core.clone()), Message::Loaded);
            Task::batch([mark, load])
        }

        Message::FirstRunResolved => {
            // The flag write resolved; nothing further to do (the parallel
            // `Loaded` task lands the app in Running).
            Task::none()
        }

        Message::ExportLogs => {
            // Export a scrubbed debug log for a bug report (opens a save dialog).
            Task::perform(bg::export_logs(app.core.clone()), Message::LogsExported)
        }

        Message::LogsExported(result) => {
            app.error_banner = Some(match result {
                Ok(summary) => summary,
                Err(err) => format!("Log export failed: {err}"),
            });
            Task::none()
        }

        Message::CloseRequested(id) => {
            // Honor the close-to-tray preference (Settings screen). When on (the
            // default), hide the window so the agent keeps running and the tray
            // can bring it back; when off, the close button quits the agent.
            if app.prefs.close_to_tray {
                window::set_mode(id, window::Mode::Hidden)
            } else {
                iced::exit()
            }
        }

        // --- C3 privacy hub: DSAR / aliases / GPC / anchors ----------------
        Message::OpenPrivacy => {
            // Switch to the Privacy hub (default DSAR tab) and load the snapshot.
            app.state = AppState::Privacy {
                snapshot: None,
                tab: PrivacyTab::default(),
                busy: true,
            };
            Task::perform(bg::load_privacy(app.core.clone()), Message::PrivacyLoaded)
        }

        Message::ClosePrivacy => {
            // Return to Running via the Loading reload (Message::Loaded lands it),
            // so we never fabricate a placeholder Status here.
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::RefreshPrivacy => {
            let AppState::Privacy { busy, .. } = &mut app.state else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            *busy = true;
            Task::perform(bg::load_privacy(app.core.clone()), Message::PrivacyLoaded)
        }

        Message::PrivacyLoaded(result) => {
            let AppState::Privacy { snapshot, busy, .. } = &mut app.state else {
                // The user navigated away before the load resolved; drop it.
                return Task::none();
            };
            *busy = false;
            match result {
                Ok(loaded) => *snapshot = Some(loaded),
                Err(err) => app.error_banner = Some(format!("Privacy load failed: {err}")),
            }
            Task::none()
        }

        Message::SetPrivacyTab(new_tab) => {
            if let AppState::Privacy { tab, .. } = &mut app.state {
                *tab = new_tab;
            }
            Task::none()
        }

        // --- Settings screen (app + device prefs) --------------------------
        Message::OpenSettings => {
            // Seed the edit buffer from the live prefs. The port text buffer
            // shows the current port (blank for the core default).
            let port_text = app
                .prefs
                .sync_port
                .map(|p| p.to_string())
                .unwrap_or_default();
            app.state = AppState::Settings {
                draft: app.prefs.clone(),
                port_text,
                busy: false,
            };
            Task::none()
        }

        Message::CloseSettings => {
            // Discard unsaved draft edits and return to Running via the reload.
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::SettingsSetTheme(choice) => {
            if let AppState::Settings { draft, .. } = &mut app.state {
                draft.theme = choice;
            }
            Task::none()
        }

        Message::SettingsSetAutoRefresh(secs) => {
            if let AppState::Settings { draft, .. } = &mut app.state {
                draft.auto_refresh_secs = secs.clamp(
                    crate::prefs::MIN_REFRESH_SECS,
                    crate::prefs::MAX_REFRESH_SECS,
                );
            }
            Task::none()
        }

        Message::SettingsToggleCloseToTray(on) => {
            if let AppState::Settings { draft, .. } = &mut app.state {
                draft.close_to_tray = on;
            }
            Task::none()
        }

        Message::SettingsSetDeviceName(name) => {
            if let AppState::Settings { draft, .. } = &mut app.state {
                // Keep the raw text in the buffer; it is normalized (trimmed,
                // blank -> None) on Save.
                draft.device_name = Some(name);
            }
            Task::none()
        }

        Message::SettingsToggleLanSync(on) => {
            if let AppState::Settings { draft, .. } = &mut app.state {
                draft.lan_sync = on;
            }
            Task::none()
        }

        Message::SettingsSetSyncPort(text) => {
            if let AppState::Settings { port_text, .. } = &mut app.state {
                *port_text = text;
            }
            Task::none()
        }

        Message::SettingsSave => {
            let AppState::Settings {
                draft,
                port_text,
                busy,
            } = &mut app.state
            else {
                return Task::none();
            };
            if *busy {
                return Task::none();
            }
            // Parse the sync port: blank means the core default (None); a present
            // value must be a valid port, else the save is refused with a note.
            let trimmed = port_text.trim();
            let parsed_port = if trimmed.is_empty() {
                None
            } else {
                match trimmed.parse::<u16>() {
                    Ok(p) if p > 0 => Some(p),
                    _ => {
                        app.error_banner = Some(format!(
                            "Sync port must be a number between 1 and 65535 (got \"{trimmed}\")."
                        ));
                        return Task::none();
                    }
                }
            };
            // Normalize the draft, then apply it live (theme + tick cadence take
            // effect immediately; device/sync prefs apply at the next start).
            draft.sync_port = parsed_port;
            draft.device_name = draft.device_name_trimmed();
            draft.auto_refresh_secs = draft.auto_refresh_secs.clamp(
                crate::prefs::MIN_REFRESH_SECS,
                crate::prefs::MAX_REFRESH_SECS,
            );
            *busy = true;
            let to_save = draft.clone();
            app.prefs = to_save.clone();
            // Re-resolve the cached theme now (the System choice does an OS query
            // here, once, not per frame) so the window re-themes on Save.
            app.resolved_theme = app.prefs.theme.to_theme();
            Task::perform(bg::save_prefs(to_save), Message::SettingsSaved)
        }

        Message::SettingsSaved(result) => {
            if let AppState::Settings { busy, .. } = &mut app.state {
                *busy = false;
            }
            match result {
                Ok(()) => {
                    app.error_banner = Some(
                        "Settings saved. Device name, LAN sync, and port apply at the next start."
                            .to_string(),
                    )
                }
                Err(err) => app.error_banner = Some(format!("Could not save settings: {err}")),
            }
            Task::none()
        }

        // --- In-app Help / FAQ screen --------------------------------------
        Message::OpenFaq => {
            app.state = AppState::Faq;
            Task::none()
        }

        Message::CloseFaq => {
            app.state = AppState::Loading;
            Task::perform(bg::load(app.core.clone()), Message::Loaded)
        }

        Message::ErrorDismissed => {
            app.error_banner = None;
            Task::none()
        }
    }
}

/// Bring the (possibly hidden) main window back to the foreground. Resolves the
/// latest window id, then un-hides and focuses it.
fn show_window() -> Task<Message> {
    window::latest().and_then(|id| {
        Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::gain_focus(id),
        ])
    })
}

/// The seed the studio's week-simulator preview starts from. Fixed so the first
/// preview of a persona is deterministic; the Re-roll action advances it.
fn default_week_seed() -> u64 {
    0
}

/// The current studio selection `(persona_id, seed)` to carry across a reload,
/// or `(None, default seed)` when nothing is selected yet. Keeps the selected
/// persona and its preview seed stable across saves and settings changes.
fn studio_selection(snapshot: Option<&crate::message::StudioSnapshot>) -> (Option<String>, u64) {
    match snapshot.and_then(|s| s.detail.as_ref()) {
        Some(detail) => (Some(detail.persona.id.clone()), detail.seed),
        None => (None, default_week_seed()),
    }
}

/// The current broker-diff selection `(persona_id, broker_id)` to carry across
/// a reload, or `(None, None)` when nothing is selected yet.
fn broker_selection(
    snapshot: Option<&crate::message::BrokerDiffSnapshot>,
) -> (Option<String>, Option<String>) {
    match snapshot {
        Some(s) => (
            s.selected_persona.clone(),
            if s.selected_broker.is_empty() {
                None
            } else {
                Some(s.selected_broker.clone())
            },
        ),
        None => (None, None),
    }
}

/// The current network-panel persona selection to carry across a reload, or
/// `None` when nothing is selected yet.
fn network_selection(snapshot: Option<&crate::message::NetworkSnapshot>) -> Option<String> {
    snapshot.and_then(|s| s.selected_persona.clone())
}

/// Apply one freeform text edit to the in-memory persona edit buffer. The
/// desktop-only optional fields clear to `None` when emptied, so an emptied box
/// drops the JSON key rather than persisting an empty string.
fn apply_text_edit(
    persona: &mut fauxx_core::SyntheticPersona,
    field: PersonaTextField,
    value: String,
) {
    match field {
        PersonaTextField::Name => persona.name = value,
        PersonaTextField::HomeLocation => persona.home_location = non_empty(value),
        PersonaTextField::Schedule => persona.schedule = non_empty(value),
        PersonaTextField::BrowsingStyle => persona.browsing_style = non_empty(value),
    }
}

/// `Some(value)` when non-empty after trimming, else `None`.
fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Apply one enum-field edit (the chosen wire NAME) to the in-memory persona
/// edit buffer. The picker only ever offers valid enum names, so this just
/// stores the string verbatim (the wire model is string-typed by design).
fn apply_enum_edit(
    persona: &mut fauxx_core::SyntheticPersona,
    field: PersonaEnumField,
    name: String,
) {
    match field {
        PersonaEnumField::AgeRange => persona.age_range = name,
        PersonaEnumField::Profession => persona.profession = name,
        PersonaEnumField::Region => persona.region = name,
    }
}

/// Toggle membership of one interest (a `CategoryPool` name) in the persona's
/// interest set, enforcing the 3..=5 count rule. Removing below the minimum or
/// adding above the maximum is refused with a human-readable reason so the count
/// invariant holds before the save round-trips through the core.
fn toggle_interest(persona: &mut fauxx_core::SyntheticPersona, name: &str) -> Result<(), String> {
    if let Some(idx) = persona.interests.iter().position(|i| i == name) {
        if persona.interests.len() <= *INTEREST_COUNT.start() {
            return Err(format!(
                "A persona needs at least {} interests; add another before removing one.",
                INTEREST_COUNT.start()
            ));
        }
        persona.interests.remove(idx);
    } else {
        if persona.interests.len() >= *INTEREST_COUNT.end() {
            return Err(format!(
                "A persona carries at most {} interests; remove one before adding another.",
                INTEREST_COUNT.end()
            ));
        }
        persona.interests.push(name.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fauxx_core::Core;

    /// A storeless app in the default boot state; the reducer mutates it
    /// synchronously and we ignore the async `Task` it returns.
    fn app() -> App {
        App::new(Core::new(), None)
    }

    #[test]
    fn open_devices_navigates_to_a_busy_devices_screen() {
        let mut app = app();
        let _ = update(&mut app, Message::OpenDevices);
        assert!(
            matches!(
                app.state,
                AppState::Devices {
                    snapshot: None,
                    busy: true,
                    ..
                }
            ),
            "OpenDevices must switch to a loading Devices screen"
        );
    }

    #[test]
    fn devices_load_error_is_non_fatal_and_sets_the_banner() {
        let mut app = app();
        app.state = AppState::Devices {
            snapshot: None,
            busy: true,
            pair_back_input: String::new(),
            pair_back_note: None,
        };
        let _ = update(&mut app, Message::DevicesLoaded(Err("boom".to_string())));
        // The screen is kept (non-fatal); the busy flag clears; the error shows
        // in the dismissable banner, NOT the terminal Error state.
        match &app.state {
            AppState::Devices { busy, .. } => assert!(!busy, "busy must clear after a failed load"),
            other => panic!(
                "must stay on Devices, got {:?}",
                std::mem::discriminant(other)
            ),
        }
        let banner = app.error_banner.unwrap_or_default();
        assert!(
            banner.contains("boom"),
            "banner must surface the error: {banner}"
        );
    }

    #[test]
    fn error_dismissed_clears_the_banner() {
        let mut app = app();
        app.error_banner = Some("something went wrong".to_string());
        let _ = update(&mut app, Message::ErrorDismissed);
        assert!(app.error_banner.is_none());
    }

    #[test]
    fn boot_failure_lands_in_the_terminal_error_state() {
        let mut app = app();
        let _ = update(
            &mut app,
            Message::Booted(Err("store would not open".to_string())),
        );
        match &app.state {
            AppState::Error(msg) => assert!(msg.contains("store would not open")),
            _ => panic!("a boot failure must land in the terminal Error state"),
        }
    }

    #[test]
    fn open_privacy_navigates_to_a_busy_dsar_tab() {
        let mut app = app();
        let _ = update(&mut app, Message::OpenPrivacy);
        assert!(
            matches!(
                app.state,
                AppState::Privacy {
                    snapshot: None,
                    tab: PrivacyTab::Dsar,
                    busy: true
                }
            ),
            "OpenPrivacy must open the Privacy hub on the default DSAR tab, loading"
        );
    }

    #[test]
    fn privacy_loaded_sets_the_snapshot_and_clears_busy() {
        let mut app = app();
        app.state = AppState::Privacy {
            snapshot: None,
            tab: PrivacyTab::Dsar,
            busy: true,
        };
        let snapshot = Box::new(crate::message::PrivacySnapshot::default());
        let _ = update(&mut app, Message::PrivacyLoaded(Ok(snapshot)));
        match &app.state {
            AppState::Privacy { snapshot, busy, .. } => {
                assert!(snapshot.is_some(), "the loaded snapshot must be stored");
                assert!(!busy, "busy clears once the load resolves");
            }
            _ => panic!("must stay on the Privacy hub"),
        }
    }

    #[test]
    fn set_privacy_tab_switches_the_active_tab() {
        let mut app = app();
        app.state = AppState::Privacy {
            snapshot: None,
            tab: PrivacyTab::Dsar,
            busy: false,
        };
        let _ = update(&mut app, Message::SetPrivacyTab(PrivacyTab::Gpc));
        assert!(matches!(
            app.state,
            AppState::Privacy {
                tab: PrivacyTab::Gpc,
                ..
            }
        ));
    }

    #[test]
    fn privacy_load_error_is_non_fatal_and_keeps_the_tab() {
        let mut app = app();
        app.state = AppState::Privacy {
            snapshot: None,
            tab: PrivacyTab::Aliases,
            busy: true,
        };
        let _ = update(&mut app, Message::PrivacyLoaded(Err("nope".to_string())));
        match &app.state {
            AppState::Privacy { busy, tab, .. } => {
                assert!(!busy);
                assert_eq!(*tab, PrivacyTab::Aliases, "a failed load keeps the tab");
            }
            _ => panic!("a failed privacy load must keep the hub, not go terminal"),
        }
        assert!(app.error_banner.unwrap_or_default().contains("nope"));
    }

    #[test]
    fn studio_remove_pack_marks_busy_and_dispatches() {
        // #27: the library "remove" action is wired into the reducer (was missing
        // any user-facing surface). It marks the studio busy while the remove runs.
        let mut app = app();
        app.state = AppState::Studio {
            snapshot: None,
            busy: false,
        };
        let _ = update(&mut app, Message::StudioRemovePack("pack-1".to_string()));
        assert!(
            matches!(app.state, AppState::Studio { busy: true, .. }),
            "StudioRemovePack must mark the studio busy while the remove runs"
        );
    }

    #[test]
    fn dashboard_select_device_sets_the_filter_and_reloads() {
        // #20 per-device filter: picking a device records the selection and kicks
        // off a reload for that persona's drift.
        let mut app = app();
        app.state = AppState::Dashboard {
            snapshot: None,
            selected_platform: 0,
            selected_device: None,
            busy: false,
        };
        let _ = update(
            &mut app,
            Message::DashboardSelectDevice("persona-2".to_string()),
        );
        match &app.state {
            AppState::Dashboard {
                selected_device,
                busy,
                ..
            } => {
                assert_eq!(selected_device.as_deref(), Some("persona-2"));
                assert!(busy, "selecting a device starts a reload");
            }
            _ => panic!("must stay on the Dashboard"),
        }
    }

    /// A Devices screen with the given pair-back state, for the #42 symmetric-
    /// pairing reducer tests.
    fn devices_state(busy: bool, input: &str) -> AppState {
        AppState::Devices {
            snapshot: None,
            busy,
            pair_back_input: input.to_string(),
            pair_back_note: None,
        }
    }

    #[test]
    fn device_pair_back_changed_updates_the_input() {
        // #42: typing in the "Pair a device back" field records the payload.
        let mut app = app();
        app.state = devices_state(false, "");
        let _ = update(
            &mut app,
            Message::DevicePairBackChanged("code-123".to_string()),
        );
        match &app.state {
            AppState::Devices {
                pair_back_input, ..
            } => assert_eq!(pair_back_input, "code-123"),
            other => panic!(
                "must stay on Devices, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn device_pair_back_with_blank_input_sets_a_hint_and_stays_idle() {
        // #42: submitting an empty/whitespace payload must not start work; it
        // nudges the user to paste a code instead.
        let mut app = app();
        app.state = devices_state(false, "   ");
        let _ = update(&mut app, Message::DevicePairBack);
        match &app.state {
            AppState::Devices {
                busy,
                pair_back_note,
                ..
            } => {
                assert!(!busy, "a blank submit must not set busy");
                assert!(
                    pair_back_note
                        .as_deref()
                        .unwrap_or_default()
                        .contains("Paste"),
                    "note should prompt for a code, got {pair_back_note:?}"
                );
            }
            other => panic!(
                "must stay on Devices, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn device_paired_back_ok_clears_input_and_notes_success() {
        // #42: a successful pair-back clears the field and shows the summary; the
        // screen stays busy while it reloads to show the new peer.
        let mut app = app();
        app.state = devices_state(true, "some-code");
        let _ = update(
            &mut app,
            Message::DevicePairedBack(Ok("Paired Phone (ab:cd).".to_string())),
        );
        match &app.state {
            AppState::Devices {
                pair_back_input,
                pair_back_note,
                ..
            } => {
                assert!(pair_back_input.is_empty(), "input must clear on success");
                assert_eq!(pair_back_note.as_deref(), Some("Paired Phone (ab:cd)."));
            }
            other => panic!(
                "must stay on Devices, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn device_paired_back_err_sets_note_and_clears_busy() {
        // #42: a failed pair-back surfaces the reason inline and re-enables the
        // control (busy clears) so the user can correct the code.
        let mut app = app();
        app.state = devices_state(true, "bad-code");
        let _ = update(
            &mut app,
            Message::DevicePairedBack(Err("invalid pairing payload".to_string())),
        );
        match &app.state {
            AppState::Devices {
                busy,
                pair_back_note,
                ..
            } => {
                assert!(!busy, "busy must clear on failure");
                assert!(
                    pair_back_note
                        .as_deref()
                        .unwrap_or_default()
                        .contains("invalid pairing payload"),
                    "note should carry the failure reason, got {pair_back_note:?}"
                );
            }
            other => panic!(
                "must stay on Devices, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }
}
