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

//! Background bridge between the iced GUI thread and the `fauxx-core` async
//! API.
//!
//! Every function here is an `async fn` that wraps one or more core calls and
//! is driven from [`crate::update`] via `Task::perform(...)`. The core methods
//! are already async and yield at every `await`, so the GUI thread never blocks
//! on the core: it dispatches a task and gets a `Message` back when the future
//! resolves. No business logic lives here, only the translation of core results
//! into the owned, `Clone` view payloads the UI consumes.

use fauxx_core::{
    Baseline, Campaign, Comparator, Config, CoordinationMode, Core, DnsStrategy, Egress, Goal,
    IntensityLevel, PackProvenance, PersonaField, PersonaSettings, Platform, PlatformDrift,
    RotationSchedule, StaticReachability, SyntheticPersona, TargetMetric,
};

use crate::firstrun;
use crate::message::{
    AliasRow, AnchorRecommendationRow, AnchorRow, BootOutcome, BrokerDiffSnapshot,
    CampaignsSnapshot, DashboardSnapshot, DevicesSnapshot, DsarRow, GpcRow, NetworkSnapshot,
    PersonaDetail, PrivacySnapshot, Snapshot, StudioSnapshot,
};

/// Open the encrypted store with the default configuration, then load the
/// first snapshot.
///
/// Opening the store can fail (no key in the OS keystore yet, a locked
/// keystore, a missing data dir on first run, and so on). Rather than panic or
/// surface a half-open core, we fall back to the store-less handle we were
/// given and let the window come up in a usable, persona-empty state. A genuine
/// load failure after that is reported as `Err` so the UI can show
/// `AppState::Error`.
pub async fn open_and_load(fallback: Core) -> Result<BootOutcome, String> {
    // The GUI is the cross-device command center (the Devices view shows pairing
    // + discovered peers, and the first-run wizard imports a persona from the
    // phone), so it opens with live LAN sync enabled by default (C1 #7): mDNS
    // discovery fills the peer list and the inbound listener (started after boot
    // via `Core::spawn_background_lan_sync`) can receive sealed personas. The
    // Settings screen can override the LAN-sync toggle, device name, and port;
    // those are GUI-local prefs read here so they take effect at start.
    let prefs = crate::prefs::load();
    let mut config = Config::new().with_lan_sync(prefs.lan_sync);
    if let Some(name) = prefs.device_name_trimmed() {
        config = config.with_device_name(name);
    }
    if let Some(port) = prefs.sync_port {
        config = config.with_sync_port(port);
    }
    let core = match Core::open(config).await {
        Ok(opened) => opened,
        Err(err) => {
            // Not fatal: the window still works against a store-less core.
            // (Persona writes will report Unimplemented, which is correct.)
            tracing::warn!("store open failed, continuing store-less: {err}");
            fallback
        }
    };
    let snapshot = load(core.clone()).await?;
    // Decide first-run BEFORE the window settles so the wizard can show. Reading
    // the marker is cheap and infallible (a missing config dir reads as
    // not-first-run), so it never blocks or fails the boot.
    let first_run = firstrun::is_first_run();
    Ok(BootOutcome {
        core,
        snapshot,
        first_run,
    })
}

/// Load a fresh status + persona-list snapshot off the core. Used by the boot
/// task, the periodic tick, and the manual refresh.
pub async fn load(core: Core) -> Result<Snapshot, String> {
    let status = core.status().await.map_err(|e| e.to_string())?;
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    Ok(Snapshot { status, personas })
}

/// Persist the GUI-local desktop settings (the Settings screen Save action).
/// Wraps the synchronous [`crate::prefs::save`] so it rides the same
/// `Task::perform` channel as the core calls (the write is a tiny local file).
pub async fn save_prefs(settings: crate::prefs::DesktopSettings) -> Result<(), String> {
    crate::prefs::save(&settings)
}

/// Load the cross-device sync snapshot for the Devices view: the pairing QR and
/// fingerprint, the paired and discovered peer lists, and the active
/// coordination mode.
///
/// On a store-less core the sync engine reports `Unimplemented`, so the pairing
/// QR/fingerprint are best-effort (`None` on error) while the peer lists come
/// back empty and the mode falls back to the core default. Only a coordination
/// mode read failure (which would leave the view without a control state) is
/// surfaced as `Err`.
pub async fn load_devices(core: Core) -> Result<DevicesSnapshot, String> {
    let pairing_qr = core.pairing_qr().await.ok();
    let fingerprint = match &pairing_qr {
        Some(qr) => Some(qr.fingerprint.clone()),
        None => core.sync_fingerprint().ok(),
    };
    let paired = core.paired_peers().await.map_err(|e| e.to_string())?;
    let discovered = core.discovered_peers().await.map_err(|e| e.to_string())?;
    let mode = core.coordination_mode().await.map_err(|e| e.to_string())?;
    Ok(DevicesSnapshot {
        pairing_qr,
        fingerprint,
        paired,
        discovered,
        mode,
    })
}

/// Set the household coordination mode, returning the now-active mode so the
/// view can reflect it without a second round-trip.
pub async fn set_mode(core: Core, mode: CoordinationMode) -> Result<CoordinationMode, String> {
    core.set_coordination_mode(mode)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mode)
}

/// Revoke a paired peer by its base64url public key. The boolean "a record was
/// removed" result is collapsed to `()`: either way the Devices view reloads
/// and shows the current peer set.
pub async fn unpair(core: Core, public_key: String) -> Result<(), String> {
    core.unpair(&public_key).await.map_err(|e| e.to_string())?;
    Ok(())
}

// --- C4 #20 A1 efficacy dashboard ------------------------------------------

/// Load the efficacy-dashboard drift snapshot for the given inspected platform.
///
/// The dashboard is an ALL-DEVICES aggregate with a single-device fallback: it
/// reads every stored persona (each device's persona) and uses the first one's
/// intent as the baseline. With no persona it returns the well-formed no-data
/// snapshot (empty per-platform, empty combined) rather than erroring, so the
/// view renders an empty state instead of an error banner. A genuine store
/// failure is surfaced as `Err`.
pub async fn load_dashboard(
    core: Core,
    platform_index: usize,
    selected_device: Option<String>,
) -> Result<DashboardSnapshot, String> {
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    let devices: Vec<(String, String)> = personas
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    if personas.is_empty() {
        // No persona yet: a clean no-data dashboard (no panic, no error banner).
        return Ok(DashboardSnapshot {
            persona_id: None,
            device_count: 0,
            per_platform: Vec::new(),
            combined: PlatformDrift::empty(Platform::Google),
            devices,
        });
    }

    // The shown persona/device is the #20 selection when it still exists, else
    // the primary (first). Drift is computed for THAT persona.
    let shown = selected_device
        .as_deref()
        .and_then(|id| personas.iter().find(|p| p.id == id))
        .unwrap_or(&personas[0]);

    // The preferred baseline is the shown persona's declared intent; drift is
    // then distance from the configured goal (the core degrades an empty intent
    // to an all-zero series without panicking).
    let baseline = Baseline::from_persona(shown);
    let per_platform = core
        .all_platform_drift(&shown.id, &baseline)
        .await
        .map_err(|e| e.to_string())?;

    // The combined cross-device aggregate for the inspected platform. Every
    // persona id is one device; the core merges by timestamp and degrades to
    // single-device (one id) cleanly.
    let platform = builtin_platform(platform_index);
    let persona_ids: Vec<String> = personas.iter().map(|p| p.id.clone()).collect();
    let combined = core
        .combined_platform_drift(platform, &persona_ids, &baseline)
        .await
        .map_err(|e| e.to_string())?;

    Ok(DashboardSnapshot {
        persona_id: Some(shown.id.clone()),
        device_count: personas.len(),
        per_platform,
        combined,
        devices,
    })
}

/// The built-in platform at a display index (Google, Brokers, Meta), clamped to
/// the first when out of range so a stale index never panics.
fn builtin_platform(index: usize) -> Platform {
    let builtins = Platform::builtins();
    builtins.get(index).cloned().unwrap_or(Platform::Google)
}

// --- C5 persona studio ------------------------------------------------------

/// Load the full studio snapshot: every persona, the installed-pack library,
/// and (when one is selected) the selected persona's editor detail.
pub async fn load_studio(
    core: Core,
    selected_id: Option<String>,
    seed: u64,
) -> Result<Box<StudioSnapshot>, String> {
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    let installed_packs = core
        .list_installed_packs()
        .await
        .map_err(|e| e.to_string())?;

    // Resolve the selection: the explicitly-selected id if it still exists,
    // else the first persona, else none.
    let target_id = selected_id
        .filter(|id| personas.iter().any(|p| &p.id == id))
        .or_else(|| personas.first().map(|p| p.id.clone()));

    let detail = match target_id {
        Some(id) => Some(load_persona_detail(&core, &id, seed).await?),
        None => None,
    };

    // Boxed at the boundary so the `Message`/`AppState` payload stays compact.
    Ok(Box::new(StudioSnapshot {
        personas,
        installed_packs,
        detail,
    }))
}

/// Load one persona's editor detail: the persona, its settings, the linter
/// findings, and a simulated-week preview at `seed`.
async fn load_persona_detail(
    core: &Core,
    persona_id: &str,
    seed: u64,
) -> Result<PersonaDetail, String> {
    let persona = core
        .get_persona(persona_id)
        .await
        .map_err(|e| e.to_string())?;
    let settings = core
        .persona_settings(persona_id)
        .await
        .map_err(|e| e.to_string())?;
    // The linter and simulator are pure functions of the persona (no store).
    let findings = core.lint_persona(&persona);
    let week = core.simulate_week(&persona, IntensityLevel::Medium, seed);
    Ok(PersonaDetail {
        persona,
        settings,
        findings,
        week,
        seed,
    })
}

/// Persist an edited persona (the editor's #24 P1 write path).
pub async fn save_persona(core: Core, persona: SyntheticPersona) -> Result<(), String> {
    core.save_persona(&persona).await.map_err(|e| e.to_string())
}

/// Lock or unlock one persona field, returning the updated settings.
pub async fn set_field_locked(
    core: Core,
    persona_id: String,
    field: PersonaField,
    locked: bool,
) -> Result<PersonaSettings, String> {
    core.set_field_locked(&persona_id, field, locked)
        .await
        .map_err(|e| e.to_string())
}

/// Set the rotation schedule for a persona, returning the updated settings.
pub async fn set_rotation(
    core: Core,
    persona_id: String,
    rotation: RotationSchedule,
) -> Result<PersonaSettings, String> {
    core.set_rotation_schedule(&persona_id, rotation)
        .await
        .map_err(|e| e.to_string())
}

/// Import a persona pack chosen through a native file dialog (#27 P4). Returns a
/// short human summary, or `Err` when the user cancels or import fails. The
/// dialog and the file read run on the blocking pool so the GUI never stalls.
pub async fn import_pack(core: Core) -> Result<String, String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Fauxx persona pack", &["json", "fauxxpack"])
        .set_title("Import persona pack")
        .pick_file()
        .await
        .ok_or_else(|| "import cancelled".to_string())?;
    let bytes = file.read().await;
    let imported = core
        .import_persona_pack(&bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("Imported {} persona(s).", imported.len()))
}

/// Remove an installed pack from the library by id (#27 P4). Returns a short
/// human summary; the Studio reloads so the library list updates. Removing the
/// ledger entry does NOT delete the personas it brought in (they are managed
/// separately), only the library record.
pub async fn remove_pack(core: Core, pack_id: String) -> Result<String, String> {
    let removed = core
        .remove_installed_pack(&pack_id)
        .await
        .map_err(|e| e.to_string())?;
    if removed {
        Ok("Removed pack from the library.".to_string())
    } else {
        Err("That pack was not in the library.".to_string())
    }
}

/// Export one persona as a signed pack to a file chosen through a native save
/// dialog (#27 P4). Returns a short human summary, or `Err` on cancel/failure.
pub async fn export_pack(core: Core, persona_id: String) -> Result<String, String> {
    let provenance = PackProvenance::us(
        "desktop_export",
        persona_id.clone(),
        fauxx_core_now_millis(),
    );
    let bytes = core
        .export_persona_pack(&[persona_id], provenance)
        .await
        .map_err(|e| e.to_string())?;
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Fauxx persona pack", &["json"])
        .set_file_name("persona-pack.json")
        .set_title("Export persona pack")
        .save_file()
        .await
        .ok_or_else(|| "export cancelled".to_string())?;
    file.write(&bytes).await.map_err(|e| e.to_string())?;
    Ok("Exported signed persona pack.".to_string())
}

/// Export a SCRUBBED copy of the persisted debug logs for a bug report (opens a
/// save dialog). The redaction set is the fixed pattern policy plus the live
/// persona ids/names and the home directory, so a persona's display name and the
/// operator's username never reach a public issue. See `fauxx_core::logging`.
pub async fn export_logs(core: Core) -> Result<String, String> {
    let redactions = fauxx_core::logging::Redactions::new(core.redaction_literals().await)
        .map_err(|e| e.to_string())?;
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Debug log", &["txt", "log"])
        .set_file_name("fauxx-debug-log.txt")
        .set_title("Export debug log for a bug report")
        .save_file()
        .await
        .ok_or_else(|| "export cancelled".to_string())?;
    let summary = fauxx_core::logging::export(
        &redactions,
        &fauxx_core::logging::diagnostics_header(),
        file.path(),
    )
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "Exported scrubbed debug log ({} lines) to {}",
        summary.lines,
        summary.out_path.display()
    ))
}

/// A local wall-clock millis helper for stamping export provenance. Kept here
/// (not in the view) so the timestamp is captured off the GUI thread.
fn fauxx_core_now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// --- C4 #22 A3 broker-diff view ---------------------------------------------

/// Load the broker-diff snapshot for the given `(persona, broker)` selection.
///
/// Resolves the selection (the requested ids if still valid, else the first
/// persona and the first registry broker), then computes the timeline through
/// `core.broker_diff_timeline`. With no persona it returns the well-formed
/// no-persona snapshot (empty selectors, `None` timeline) rather than erroring.
pub async fn load_brokers(
    core: Core,
    selected_persona: Option<String>,
    selected_broker: Option<String>,
) -> Result<Box<BrokerDiffSnapshot>, String> {
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    let persona_choices: Vec<(String, String)> = personas
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    // The bundled broker registry (static; no store needed).
    let brokers: Vec<(String, String)> = core
        .broker_registry()
        .into_iter()
        .map(|(id, template)| (id.to_string(), template.display_name.clone()))
        .collect();

    // Resolve the persona selection: the requested id if it still exists, else
    // the first persona, else none.
    let persona_id = selected_persona
        .filter(|id| personas.iter().any(|p| &p.id == id))
        .or_else(|| personas.first().map(|p| p.id.clone()));

    // Resolve the broker selection: the requested id if it is a known broker,
    // else the first registry broker, else an empty id (the no-broker case).
    let broker_id = selected_broker
        .filter(|id| brokers.iter().any(|(b, _)| b == id))
        .or_else(|| brokers.first().map(|(b, _)| b.clone()))
        .unwrap_or_default();

    let timeline = match (&persona_id, broker_id.is_empty()) {
        (Some(pid), false) => Some(
            core.broker_diff_timeline(&broker_id, pid)
                .await
                .map_err(|e| e.to_string())?,
        ),
        _ => None,
    };

    Ok(Box::new(BrokerDiffSnapshot {
        personas: persona_choices,
        brokers,
        selected_persona: persona_id,
        selected_broker: broker_id,
        timeline,
    }))
}

// --- C8 #33 U2 campaign panel -----------------------------------------------

/// Load the campaign snapshot: every campaign plus the personas a new campaign
/// can target.
pub async fn load_campaigns(core: Core) -> Result<CampaignsSnapshot, String> {
    let campaigns = core.list_campaigns(None).await.map_err(|e| e.to_string())?;
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    let persona_choices = personas
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    Ok(CampaignsSnapshot {
        campaigns,
        personas: persona_choices,
    })
}

/// Create a goal-driven campaign from the draft fields (C8 #33 U2). Validates
/// the threshold and required selections, builds the [`Goal`] + [`Campaign`]
/// through the core types, and persists it via `core.save_campaign`.
pub async fn create_campaign(
    core: Core,
    label: String,
    persona_id: String,
    segment: String,
    comparator: Comparator,
    threshold: String,
) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("give the campaign a label first".to_string());
    }
    if persona_id.is_empty() {
        return Err("pick a persona for the campaign first".to_string());
    }
    if segment.is_empty() {
        return Err("pick a target segment for the campaign first".to_string());
    }
    let threshold: f64 = threshold
        .trim()
        .parse()
        .map_err(|_| "the goal threshold must be a number".to_string())?;
    // The only closed-loop metric today is the A1 segment drift.
    let goal =
        Goal::new(TargetMetric::SegmentDrift, comparator, threshold).map_err(|e| e.to_string())?;
    let now = fauxx_core_now_millis();
    let campaign = Campaign::new(uuid_v4(), label.to_string(), persona_id, segment, goal, now);
    core.save_campaign(&campaign)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("Created campaign \u{201c}{label}\u{201d}."))
}

/// Start (or resume) a campaign by id, returning a short summary.
pub async fn start_campaign(core: Core, id: String) -> Result<String, String> {
    let now = fauxx_core_now_millis();
    core.start_campaign(&id, now)
        .await
        .map_err(|e| e.to_string())?;
    Ok("Campaign started.".to_string())
}

/// Pause a campaign by id, returning a short summary.
pub async fn pause_campaign(core: Core, id: String) -> Result<String, String> {
    let now = fauxx_core_now_millis();
    core.pause_campaign(&id, now)
        .await
        .map_err(|e| e.to_string())?;
    Ok("Campaign paused.".to_string())
}

// --- C7 #30/#31 egress + DNS panel ------------------------------------------

/// Load the per-persona egress + DNS snapshot for the given selection.
///
/// The exit indicator is computed with the STATIC reachable seam
/// ([`StaticReachability::reachable`]) so the panel needs no live network call
/// (this host has none); it shows the configured exit label and never falsely
/// pauses a Direct egress. With no persona it returns the no-persona snapshot.
pub async fn load_network(
    core: Core,
    selected_persona: Option<String>,
) -> Result<NetworkSnapshot, String> {
    let personas = core.list_personas().await.map_err(|e| e.to_string())?;
    let persona_choices: Vec<(String, String)> = personas
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();

    let persona_id = selected_persona
        .filter(|id| personas.iter().any(|p| &p.id == id))
        .or_else(|| personas.first().map(|p| p.id.clone()));

    let Some(pid) = persona_id else {
        return Ok(NetworkSnapshot {
            personas: persona_choices,
            selected_persona: None,
            egress: Egress::Direct,
            dns: DnsStrategy::SystemDefault,
            exit: None,
            dns_note: DnsStrategy::SystemDefault.observer_note(),
        });
    };

    let egress = core
        .get_persona_egress(&pid)
        .await
        .map_err(|e| e.to_string())?;
    let dns = core
        .get_persona_dns(&pid)
        .await
        .map_err(|e| e.to_string())?;
    // The static-reachable seam keeps the indicator deterministic and avoids a
    // live TCP connect on a host with no network/display.
    let exit = core
        .persona_egress_exit(&pid, &StaticReachability::reachable())
        .await
        .map_err(|e| e.to_string())?;
    let dns_note = dns.observer_note();

    Ok(NetworkSnapshot {
        personas: persona_choices,
        selected_persona: Some(pid),
        egress,
        dns,
        exit: Some(exit),
        dns_note,
    })
}

/// Bind a persona's egress (N1), reporting success/failure for the banner.
pub async fn set_egress(core: Core, persona_id: String, egress: Egress) -> Result<(), String> {
    core.set_persona_egress(&persona_id, egress)
        .await
        .map_err(|e| e.to_string())
}

/// Bind a persona's DNS strategy (N2), reporting success/failure for the banner.
pub async fn set_dns(core: Core, persona_id: String, dns: DnsStrategy) -> Result<(), String> {
    core.set_persona_dns(&persona_id, dns)
        .await
        .map_err(|e| e.to_string())
}

/// A fresh UUID v4 string for new campaign ids. The `uuid` crate is already a
/// transitive dep of the core; the desktop generates ids off the GUI thread.
fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

// --- C8 #34 U3 first-run wizard + tray quick controls -----------------------

/// Import a persona from the phone via a scanned O1 pairing-payload string
/// (the wizard's key step). This completes pairing with the phone through the
/// core pairing API (which records the peer and persists the paired record);
/// the actual persona then arrives over the sealed sync channel. Returns a short
/// human summary, or `Err` on a malformed/failed payload.
pub async fn import_phone_persona(core: Core, payload: String) -> Result<String, String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Err("paste the pairing code from the phone first".to_string());
    }
    let peer = core
        .complete_pairing(trimmed)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Paired with {}. Personas will sync from the phone.",
        peer.name
    ))
}

/// Persist the first-run-completed marker (off the GUI thread). Infallible from
/// the caller's view: a write failure is logged inside, not surfaced.
pub async fn mark_first_run_complete() {
    firstrun::mark_complete();
}

/// Pause every running campaign (the tray "Pause" quick control, #34 U3).
/// Best-effort: the count of ticked campaigns is collapsed to `()`.
pub async fn pause_all(core: Core) -> Result<(), String> {
    let now = fauxx_core_now_millis();
    let running = core.list_campaigns(None).await.map_err(|e| e.to_string())?;
    for campaign in running {
        // Pause is idempotent in the planner; ignore per-campaign errors so one
        // bad campaign does not block the rest.
        let _ = core.pause_campaign(&campaign.id, now).await;
    }
    Ok(())
}

/// Resume (start) every paused campaign (the tray "Resume" quick control).
pub async fn resume_all(core: Core) -> Result<(), String> {
    let now = fauxx_core_now_millis();
    let campaigns = core.list_campaigns(None).await.map_err(|e| e.to_string())?;
    for campaign in campaigns {
        let _ = core.start_campaign(&campaign.id, now).await;
    }
    Ok(())
}

/// Window before the statutory DSAR deadline within which a request reads as
/// "due soon" (7 days).
const DSAR_DUE_SOON_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Load the C3 privacy-hub snapshot: DSAR requests + deadlines (#16), email
/// aliases (#17), per-site GPC honoring (#18), and account anchors (#19).
///
/// Each list is PRE-FORMATTED into display rows here (deadline state, enum
/// labels, linkage flags) so the view is a dumb renderer. All four reads are
/// over data the core already holds; no network call. An empty/store-less core
/// yields empty lists (not an error), so the hub renders its empty states.
pub async fn load_privacy(core: Core) -> Result<Box<PrivacySnapshot>, String> {
    let now = fauxx_core_now_millis();

    let dsar = core
        .list_dsar_requests(None)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| {
            let overdue = r.is_overdue(now);
            let deadline = if r.sent_at.is_none() {
                "not sent".to_string()
            } else if overdue {
                "overdue".to_string()
            } else if r.is_due_soon(now, DSAR_DUE_SOON_WINDOW_MS) {
                "due soon".to_string()
            } else {
                "on track".to_string()
            };
            DsarRow {
                controller: r.controller.name.clone(),
                kind: r.kind.label().to_string(),
                status: r.status.as_str().to_string(),
                deadline,
                overdue,
            }
        })
        .collect();

    let aliases = core
        .list_email_aliases(None)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|a| AliasRow {
            site: a.site.clone(),
            address: a.address.clone(),
            kind: a.kind.as_str().to_string(),
            status: a.status.as_str().to_string(),
        })
        .collect();

    let gpc = core
        .list_gpc_status()
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|g| GpcRow {
            origin: g.origin.clone(),
            honored: g.support.honored,
        })
        .collect();

    let anchors = core
        .list_account_anchors()
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|a| AnchorRow {
            label: a.label.clone(),
            site: a.site.clone(),
            signals: a.signals.len(),
            linked: a.shared_contact_key.is_some(),
        })
        .collect();

    // The prioritized partitioning recommendations (#19): surface the analysis,
    // not just the inventory. Each carries the linkage score that ranked it.
    let anchor_recommendations = core
        .account_anchor_recommendations()
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| AnchorRecommendationRow {
            label: r.label.clone(),
            action: r.kind.as_str().to_string(),
            score: r.score,
            rationale: r.rationale.clone(),
        })
        .collect();

    Ok(Box::new(PrivacySnapshot {
        dsar,
        aliases,
        gpc,
        anchors,
        anchor_recommendations,
    }))
}
