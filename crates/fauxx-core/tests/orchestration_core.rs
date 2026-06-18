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

//! C8 Orchestration Core integration tests (issues #32 U1, #33 U2, #36 U5).
//!
//! Hermetic: no real broker, no live measurement, no OS idle detection. Each
//! test uses a temp [`EncryptedFile`] store and the injected stubs.
//!
//! - U1: the [`RatePlanner`] gates scheduling across `Active` / `Idle` / `Locked`
//!   via the [`StubIdleSource`] (active pauses, idle past the threshold scales
//!   up, locked pauses).
//! - U2: the [`CampaignPlanner`] over a temp store drives the closed loop with a
//!   [`StubMetricSource`]: gap-to-intensity, back-off near the threshold, the
//!   Achieved/Paused transitions, a persistence round-trip, and survival across
//!   a store reopen (the schema migrated forward to include the campaigns table).
//! - U5: the [`MockMqtt`] bridge asserts the published HA-discovery sensor
//!   payloads, and a command-topic message routes start/pause/adjust into the U2
//!   campaign planner.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use fauxx_core::campaigns::{
    Campaign, CampaignPlanner, Comparator, Goal, StubMetricSource, TargetMetric,
};
use fauxx_core::error::Result;
use fauxx_core::idle::{
    ActiveBehavior, IdleScalingConfig, IdleState, RateDecision, RatePlanner, StubIdleSource,
};
use fauxx_core::mqtt::command::route as route_command;
use fauxx_core::mqtt::{
    publish_discovery, publish_status, CampaignCommand, DiscoveryConfig, EfficacySensor, MockMqtt,
    MqttBridge, MqttConfig, StatusPayload,
};
use fauxx_core::orchestration::IntensityLevel;
use fauxx_core::store::{EncryptedStore, KeySource, SCHEMA_VERSION};
use fauxx_core::{Config, Core};

fn passphrase_source(dir: &Path) -> KeySource {
    KeySource::EncryptedFile {
        path: dir.join("key.bin"),
        passphrase: "c8-orchestration-test".to_string(),
    }
}

fn open_store(dir: &Path) -> Result<Arc<Mutex<EncryptedStore>>> {
    let store = EncryptedStore::open_at(&dir.join("fauxx.db"), &passphrase_source(dir))?;
    Ok(Arc::new(Mutex::new(store)))
}

fn drift_goal(comparator: Comparator, threshold: f64) -> Result<Goal> {
    Goal::new(TargetMetric::SegmentDrift, comparator, threshold)
}

/// A `Core` [`Config`] over a temp dir with the headless passphrase-file key.
fn core_config(dir: &Path) -> Config {
    Config::new()
        .with_path(dir.join("fauxx.db"))
        .with_key_source(passphrase_source(dir))
}

/// A `Running` campaign with its closed-loop progress pre-seeded (so it drives
/// at a known intensity through the live `Core` directive path without needing a
/// live measurement source to feed the tick).
fn running_campaign(id: &str, persona: &str, segment: &str, last_gap: f64) -> Result<Campaign> {
    let mut campaign = Campaign::new(
        id,
        "Steer",
        persona,
        segment,
        drift_goal(Comparator::AtLeast, 1.0)?,
        0,
    );
    campaign.start(0)?;
    campaign.progress.last_metric = Some(1.0 - last_gap);
    campaign.progress.last_gap = Some(last_gap);
    Ok(campaign)
}

// --- U1 (#32): idle/lock-aware scheduling -----------------------------------

#[tokio::test]
async fn u1_idle_gating_across_all_three_states() -> Result<()> {
    // One planner, the injected stub flipped across Active / Idle(>threshold) /
    // Locked, mapping a base intensity to a scheduling decision.
    let stub = StubIdleSource::new(IdleState::Active);
    let config = IdleScalingConfig::new()
        .with_idle_threshold_secs(300)
        .with_steps_per_threshold(1);
    let planner = RatePlanner::new(Box::new(stub.clone()), config);

    // Active: the default behavior pauses decoy activity.
    assert_eq!(
        planner.plan(IntensityLevel::Medium).await,
        RateDecision::Paused,
        "an active session must pause decoy activity"
    );

    // Idle just under the threshold: runs at the base, no scale-up yet.
    stub.set(IdleState::idle_secs(299));
    assert_eq!(
        planner.plan(IntensityLevel::Medium).await,
        RateDecision::Run(IntensityLevel::Medium)
    );

    // Idle past the threshold: scales UP the intensity ladder.
    stub.set(IdleState::idle_secs(301));
    assert_eq!(
        planner.plan(IntensityLevel::Medium).await,
        RateDecision::Run(IntensityLevel::High),
        "idle past the threshold must scale intensity up"
    );

    // Locked: paused like active (conservative).
    stub.set(IdleState::Locked);
    assert_eq!(
        planner.plan(IntensityLevel::Medium).await,
        RateDecision::Paused,
        "a locked session must pause decoy activity"
    );

    Ok(())
}

#[tokio::test]
async fn u1_active_behavior_is_configurable() -> Result<()> {
    // The active-state behavior is configurable: throttle to a floor instead of
    // pausing outright.
    let stub = StubIdleSource::new(IdleState::Active);
    let config = IdleScalingConfig::new()
        .with_idle_threshold_secs(60)
        .with_active_behavior(ActiveBehavior::Throttle(IntensityLevel::Low));
    let planner = RatePlanner::new(Box::new(stub.clone()), config);

    // Active now throttles to the floor rather than pausing.
    assert_eq!(
        planner.plan(IntensityLevel::High).await,
        RateDecision::Run(IntensityLevel::Low)
    );
    // Locked also follows the configured active behavior (throttle).
    stub.set(IdleState::Locked);
    assert_eq!(
        planner.plan(IntensityLevel::High).await,
        RateDecision::Run(IntensityLevel::Low)
    );
    Ok(())
}

#[tokio::test]
async fn u1_idle_gating_is_consumed_by_the_live_campaign_directive() -> Result<()> {
    // #32 AC3: the rate planner is CONSUMED by the live core path. A campaign
    // driving at High (gap 0.9) is gated through an injected idle source: paused
    // while Active/Locked, scaled UP past the idle threshold.
    let dir = tempfile::tempdir()?;
    let stub = StubIdleSource::new(IdleState::Active);
    let scaling = IdleScalingConfig::new()
        .with_idle_threshold_secs(300)
        .with_steps_per_threshold(1);
    let planner = RatePlanner::new(Box::new(stub.clone()), scaling);
    let core = Core::open_with_idle_planner(core_config(dir.path()), planner).await?;

    core.save_campaign(&running_campaign(
        "camp-idle",
        "persona-1",
        "TECHNOLOGY",
        0.9,
    )?)
    .await?;

    // Active: the High campaign intensity is gated to paused (idle directive).
    stub.set(IdleState::Active);
    assert!(
        !core
            .campaign_directive_for_persona("persona-1")
            .await?
            .is_running(),
        "an active session must pause the campaign-driven activity"
    );

    // Idle past the threshold: the High base scales UP one step to Extreme.
    stub.set(IdleState::idle_secs(301));
    let scaled = core.campaign_directive_for_persona("persona-1").await?;
    assert_eq!(scaled.intensity, Some(IntensityLevel::Extreme));
    assert_eq!(scaled.target_segment, "TECHNOLOGY");

    // Locked: paused like Active.
    stub.set(IdleState::Locked);
    assert!(
        !core
            .campaign_directive_for_persona("persona-1")
            .await?
            .is_running(),
        "a locked session must pause the campaign-driven activity"
    );
    Ok(())
}

#[tokio::test]
async fn u1_campaign_directive_is_ungated_without_idle_gating() -> Result<()> {
    // The default (no idle gating) preserves the homelab path: a dedicated box
    // runs at the campaign's full intensity, never paused by a missing detector.
    let dir = tempfile::tempdir()?;
    let core = Core::open(core_config(dir.path())).await?;
    core.save_campaign(&running_campaign(
        "camp-plain",
        "persona-1",
        "TECHNOLOGY",
        0.9,
    )?)
    .await?;
    assert_eq!(
        core.idle_state_now().await,
        IdleState::Active,
        "ungated core reports the conservative display default"
    );
    let directive = core.campaign_directive_for_persona("persona-1").await?;
    assert_eq!(directive.intensity, Some(IntensityLevel::High));
    Ok(())
}

// --- U2 (#33): campaign planner closed loop + persistence -------------------

#[tokio::test]
async fn u2_closed_loop_gap_intensity_backoff_and_transitions() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    let metrics = StubMetricSource::new(None);
    let planner = CampaignPlanner::new(Arc::clone(&store), Arc::new(metrics.clone()));

    // Goal: drive the TECH segment's drift UP to at least 1.0, with a short
    // dwell so the test can confirm Achieved without long waits.
    let goal = drift_goal(Comparator::AtLeast, 1.0)?;
    let campaign = Campaign::new("camp-1", "Steer TECH", "persona-1", "TECHNOLOGY", goal, 0)
        .with_dwell_ms(1_000);
    planner.save(&campaign).await?;
    let started = planner.start("camp-1", 0).await?;
    assert_eq!(
        started.status,
        fauxx_core::campaigns::CampaignStatus::Running
    );

    // Far below the goal (drift 0.1, gap 0.9): drive HARD.
    metrics.set(Some(0.1)).await;
    let directive = planner.tick("camp-1", 10).await?;
    assert_eq!(directive.intensity, Some(IntensityLevel::High));
    assert_eq!(directive.target_segment, "TECHNOLOGY");
    let after = planner.get("camp-1").await?.expect_some()?;
    // Gap was computed and persisted.
    assert!((after.progress.gap().unwrap_or(f64::NAN) - 0.9).abs() < 1e-9);

    // Approaching the threshold (drift 0.95, gap 0.05 <= BACKOFF_GAP): back off.
    metrics.set(Some(0.95)).await;
    let directive = planner.tick("camp-1", 20).await?;
    assert_eq!(
        directive.intensity,
        Some(IntensityLevel::Low),
        "intensity must back off as the metric approaches the threshold"
    );

    // Goal satisfied (drift 1.1 >= 1.0): the dwell window opens but is not yet
    // elapsed, so it keeps running (and still backs off gently).
    metrics.set(Some(1.1)).await;
    let directive = planner.tick("camp-1", 30).await?;
    assert!(directive.is_running());
    let mid = planner.get("camp-1").await?.expect_some()?;
    assert_eq!(mid.status, fauxx_core::campaigns::CampaignStatus::Running);
    assert!(mid.progress.satisfied_since.is_some());

    // Still satisfied and the dwell (1000ms) has now elapsed: Achieved.
    let directive = planner.tick("camp-1", 1_030).await?;
    assert!(
        !directive.is_running(),
        "an achieved campaign stops driving"
    );
    let done = planner.get("camp-1").await?.expect_some()?;
    assert_eq!(done.status, fauxx_core::campaigns::CampaignStatus::Achieved);

    Ok(())
}

#[tokio::test]
async fn u2_pause_resets_dwell_and_resume_reruns() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    let metrics = StubMetricSource::new(Some(0.4));
    let planner = CampaignPlanner::new(Arc::clone(&store), Arc::new(metrics.clone()));

    let goal = drift_goal(Comparator::AtMost, 0.5)?; // cap drift at 0.5
    let campaign = Campaign::new("camp-2", "Cap TECH", "persona-1", "TECHNOLOGY", goal, 0)
        .with_dwell_ms(10_000);
    planner.save(&campaign).await?;
    planner.start("camp-2", 0).await?;

    // Satisfied: dwell window opens.
    planner.tick("camp-2", 0).await?;
    let running = planner.get("camp-2").await?.expect_some()?;
    assert!(running.progress.satisfied_since.is_some());

    // Pause on user request: clears the dwell, stops driving.
    let paused = planner.pause("camp-2", 100).await?;
    assert_eq!(paused.status, fauxx_core::campaigns::CampaignStatus::Paused);
    assert_eq!(paused.progress.satisfied_since, None);
    let directive = planner.tick("camp-2", 200).await?;
    assert!(
        !directive.is_running(),
        "a paused campaign does not drive activity"
    );

    // Resume returns to Running.
    let resumed = planner.start("camp-2", 300).await?;
    assert_eq!(
        resumed.status,
        fauxx_core::campaigns::CampaignStatus::Running
    );
    Ok(())
}

#[tokio::test]
async fn u2_adjust_threshold_resets_the_dwell_window() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    let metrics = StubMetricSource::new(Some(1.1));
    let planner = CampaignPlanner::new(Arc::clone(&store), Arc::new(metrics.clone()));

    // Goal met at drift >= 1.0; a long dwell so a single tick only OPENS the
    // window (does not elapse it).
    let goal = drift_goal(Comparator::AtLeast, 1.0)?;
    let campaign = Campaign::new("camp-adj", "Steer TECH", "persona-1", "TECHNOLOGY", goal, 0)
        .with_dwell_ms(10_000);
    planner.save(&campaign).await?;
    planner.start("camp-adj", 0).await?;

    // Satisfied (1.1 >= 1.0): the dwell window opens.
    planner.tick("camp-adj", 0).await?;
    let running = planner.get("camp-adj").await?.expect_some()?;
    assert!(
        running.progress.satisfied_since.is_some(),
        "the dwell window opens once the goal is satisfied"
    );

    // Harden the threshold (1.0 -> 2.0). The dwell earned against the easier
    // goal must NOT carry over, or the next tick could declare Achieved on it.
    let adjusted = planner.adjust_threshold("camp-adj", 2.0, 500).await?;
    assert_eq!(
        adjusted.progress.satisfied_since, None,
        "adjusting the threshold re-earns the dwell from scratch"
    );
    assert!((adjusted.goal.threshold - 2.0).abs() < 1e-12);
    assert_eq!(
        adjusted.status,
        fauxx_core::campaigns::CampaignStatus::Running,
        "adjust leaves the lifecycle status untouched"
    );

    // Now the metric (1.1) no longer meets the hardened goal (>= 2.0): the next
    // tick keeps running (it does not flip to Achieved on the stale dwell).
    let directive = planner.tick("camp-adj", 1_000).await?;
    assert!(
        directive.is_running(),
        "a hardened, unmet goal must not be Achieved on the prior dwell"
    );
    let after = planner.get("camp-adj").await?.expect_some()?;
    assert_eq!(after.status, fauxx_core::campaigns::CampaignStatus::Running);
    assert_eq!(after.progress.satisfied_since, None);

    Ok(())
}

#[tokio::test]
async fn u2_persistence_round_trip_survives_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;

    // First session: define, run, and tick a campaign so progress is persisted.
    {
        let store = open_store(dir.path())?;
        let metrics = StubMetricSource::new(Some(0.2));
        let planner = CampaignPlanner::new(store, Arc::new(metrics));
        let goal = drift_goal(Comparator::AtLeast, 1.0)?;
        let campaign = Campaign::new("camp-3", "Steer", "persona-1", "TECHNOLOGY", goal, 0);
        planner.save(&campaign).await?;
        planner.start("camp-3", 0).await?;
        planner.tick("camp-3", 50).await?;
    }

    // Second session: reopen the SAME store. The campaign + its progress survive.
    {
        let store = open_store(dir.path())?;
        // Confirm the schema-forward migration landed the campaigns table.
        assert_eq!(store.lock().await.list_campaigns(None)?.len(), 1);
        let metrics = StubMetricSource::new(Some(0.2));
        let planner = CampaignPlanner::new(store, Arc::new(metrics));
        let reloaded = planner.get("camp-3").await?.expect_some()?;
        assert_eq!(
            reloaded.status,
            fauxx_core::campaigns::CampaignStatus::Running
        );
        assert_eq!(reloaded.progress.last_metric, Some(0.2));
        assert!((reloaded.goal.threshold - 1.0).abs() < 1e-12);
        // It still ticks after restart.
        let directive = planner.tick("camp-3", 100).await?;
        assert!(directive.is_running());
    }
    Ok(())
}

#[tokio::test]
async fn u2_effective_directive_steers_plan_inputs() -> Result<()> {
    use fauxx_core::browser::{category_sites, sites_for_categories};
    use fauxx_core::persona::CategoryPool;

    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    let metrics = StubMetricSource::new(None);
    let planner = CampaignPlanner::new(Arc::clone(&store), Arc::new(metrics.clone()));

    // A persona whose LEAD interest is TECHNOLOGY, with a campaign that targets a
    // DIFFERENT segment (GAMING) so the bias is observable: a no-op bridge would
    // leave TECHNOLOGY leading.
    let persona_categories = [
        "TECHNOLOGY".to_string(),
        "SCIENCE".to_string(),
        "GAMING".to_string(),
    ];
    let goal = drift_goal(Comparator::AtLeast, 1.0)?;
    let campaign = Campaign::new("camp-steer", "Steer GAMING", "persona-1", "GAMING", goal, 0);
    planner.save(&campaign).await?;
    planner.start("camp-steer", 0).await?;

    // Before any tick (no last_gap): the persona is not yet being steered.
    assert!(!planner.effective_directive("persona-1").await?.is_running());

    // Tick far from the goal (drift 0.1, gap 0.9 -> High).
    metrics.set(Some(0.1)).await;
    planner.tick("camp-steer", 10).await?;

    // The effective directive now steers persona-1: High intensity, biased to
    // GAMING, derived from PERSISTED progress (no extra tick).
    let directive = planner.effective_directive("persona-1").await?;
    assert_eq!(directive.intensity, Some(IntensityLevel::High));
    assert_eq!(directive.target_segment, "GAMING");

    // Topic bias is MATERIAL: the biased category order leads with GAMING, and
    // resolving it to sites (exactly what `build_decoy_plan` does) puts a GAMING
    // page first, ahead of the persona's lead TECHNOLOGY interest.
    let biased = directive.bias_categories(&persona_categories);
    assert_eq!(biased.first().map(String::as_str), Some("GAMING"));
    let pools: Vec<CategoryPool> = biased
        .iter()
        .filter_map(|name| CategoryPool::from_name(name))
        .collect();
    let targets = sites_for_categories(&pools);
    let gaming_sites = category_sites(CategoryPool::GAMING)?;
    match targets.first() {
        Some(first) => assert!(
            gaming_sites.contains(first),
            "the targeted segment's sites must lead the resolved plan, got {first}"
        ),
        None => panic!("the biased categories must resolve to at least one site"),
    }

    // A persona with no running campaign is not steered (idle directive).
    assert!(!planner.effective_directive("other").await?.is_running());

    // Most-aggressive-wins: a second running campaign for the SAME persona with a
    // narrower gap (Medium) must not override the wider-gap GAMING/High directive.
    let goal2 = drift_goal(Comparator::AtLeast, 1.0)?;
    let c2 = Campaign::new(
        "camp-soft",
        "Soft SCIENCE",
        "persona-1",
        "SCIENCE",
        goal2,
        0,
    );
    planner.save(&c2).await?;
    planner.start("camp-soft", 0).await?;
    metrics.set(Some(0.7)).await; // gap 0.3 -> Medium
    planner.tick("camp-soft", 20).await?;
    let winner = planner.effective_directive("persona-1").await?;
    assert_eq!(winner.intensity, Some(IntensityLevel::High));
    assert_eq!(
        winner.target_segment, "GAMING",
        "the widest-gap (most aggressive) campaign steers the shared persona"
    );
    Ok(())
}

#[tokio::test]
async fn u2_schema_migrated_forward_to_current_version() -> Result<()> {
    // Opening a fresh temp store runs the forward-only migrations up to and
    // including the new campaigns table (schema v14 -> v15). SCHEMA_VERSION is
    // the public contract the migration chain stamps.
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    // The campaigns table exists and is empty on a fresh store: the forward
    // migration that introduced it (and bumped SCHEMA_VERSION) ran.
    assert!(store.lock().await.list_campaigns(None)?.is_empty());
    // The schema version this build expects includes the campaigns migration.
    let expected_at_least: i64 = 15;
    assert!(
        SCHEMA_VERSION >= expected_at_least,
        "SCHEMA_VERSION {SCHEMA_VERSION} must include the campaigns migration"
    );
    Ok(())
}

// --- U5 (#36): Home Assistant / MQTT hooks (MockMqtt) -----------------------

#[tokio::test]
async fn u5_mock_bridge_publishes_ha_discovery_sensors() -> Result<()> {
    let mock = MockMqtt::new();
    let cfg = MqttConfig::new("ha.lan");

    // Build the discovery set: a status sensor plus one efficacy sensor.
    let efficacy = EfficacySensor::new("persona-1", "Google", 0.42, 7);
    let object_ids = vec![efficacy.object_id()];
    let discovery = DiscoveryConfig::build(&cfg, &object_ids);

    // Publish every discovery config through the mock bridge.
    for (topic, payload) in &discovery.sensors {
        let json = serde_json::to_string(payload)?;
        mock.publish_discovery(topic, &json).await;
    }

    let published = mock.discovery_publishes();
    assert_eq!(published.len(), 2, "status + one efficacy sensor");

    // The status sensor's discovery payload points at the right state topic and
    // carries the HA-required keys.
    let (status_topic, status_json) = &published[0];
    assert_eq!(
        status_topic,
        "homeassistant/sensor/fauxx_desktop/status/config"
    );
    assert!(status_json.contains("\"state_topic\":\"fauxx/status/state\""));
    assert!(status_json.contains("\"unique_id\":\"fauxx_desktop_status\""));
    assert!(status_json.contains("\"value_template\""));

    // The efficacy sensor's discovery payload points at its namespaced state
    // topic and declares the KL unit.
    let (eff_topic, eff_json) = &published[1];
    assert_eq!(
        eff_topic,
        "homeassistant/sensor/fauxx_desktop/efficacy_persona_1_google/config"
    );
    assert!(eff_json.contains("\"state_topic\":\"fauxx/efficacy/persona_1_google/state\""));
    assert!(eff_json.contains("\"unit_of_measurement\":\"KL\""));

    // Now publish the matching STATE payloads and assert their content.
    let status_state = StatusPayload::new(
        true,
        IdleState::idle_secs(600),
        Some(IntensityLevel::High),
        1,
    );
    mock.publish_state(
        &cfg.status_state_topic(),
        &serde_json::to_string(&status_state)?,
    )
    .await;
    mock.publish_state(
        &cfg.efficacy_state_topic(&efficacy.object_id()),
        &serde_json::to_string(&efficacy)?,
    )
    .await;

    let states = mock.state_publishes();
    assert_eq!(states.len(), 2);
    assert_eq!(states[0].0, "fauxx/status/state");
    assert!(states[0].1.contains("\"summary\":\"running (high)\""));
    assert!(states[0].1.contains("\"running\":true"));
    assert_eq!(states[1].0, "fauxx/efficacy/persona_1_google/state");
    assert!(states[1].1.contains("\"drift\":0.42"));

    Ok(())
}

#[tokio::test]
async fn u5_command_topic_routes_into_campaign_planner() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = open_store(dir.path())?;
    let metrics = StubMetricSource::new(Some(0.2));
    let planner = CampaignPlanner::new(Arc::clone(&store), Arc::new(metrics));

    // A Planned campaign HA will control.
    let goal = drift_goal(Comparator::AtLeast, 0.8)?;
    let campaign = Campaign::new("camp-cmd", "HA-driven", "persona-1", "TECHNOLOGY", goal, 0);
    planner.save(&campaign).await?;

    // START command from HA on the command topic: routes to Running.
    let started = route_command(
        &planner,
        br#"{"action":"start","campaignId":"camp-cmd"}"#,
        100,
    )
    .await?;
    assert_eq!(
        started.status,
        fauxx_core::campaigns::CampaignStatus::Running
    );

    // ADJUST command: changes the goal threshold.
    let adjusted = route_command(
        &planner,
        br#"{"action":"adjust","campaignId":"camp-cmd","threshold":0.5}"#,
        200,
    )
    .await?;
    assert!((adjusted.goal.threshold - 0.5).abs() < 1e-12);
    assert_eq!(
        adjusted.status,
        fauxx_core::campaigns::CampaignStatus::Running
    );

    // PAUSE command: routes to Paused.
    let paused = route_command(
        &planner,
        br#"{"action":"pause","campaignId":"camp-cmd"}"#,
        300,
    )
    .await?;
    assert_eq!(paused.status, fauxx_core::campaigns::CampaignStatus::Paused);

    // A malformed command fails closed and changes nothing.
    assert!(route_command(&planner, b"garbage", 400).await.is_err());
    let unchanged = planner.get("camp-cmd").await?.expect_some()?;
    assert_eq!(
        unchanged.status,
        fauxx_core::campaigns::CampaignStatus::Paused
    );

    // The typed command parses identically (the bridge stays free of the
    // campaign types; the orchestration layer owns routing).
    let cmd = CampaignCommand::parse(br#"{"action":"start","campaignId":"camp-cmd"}"#)?;
    assert_eq!(cmd.campaign_id(), "camp-cmd");

    Ok(())
}

#[tokio::test]
async fn u5_status_snapshot_publishes_status_and_efficacy_via_mock() -> Result<()> {
    // #36 AC4: the live serve loop builds a status + efficacy snapshot and
    // publishes it. Here we exercise the exact core pieces serve calls
    // (mqtt_status_snapshot + publish_discovery + publish_status) through the
    // MockMqtt bridge and assert the HA topics/payloads.
    let dir = tempfile::tempdir()?;
    let core = Core::open(core_config(dir.path())).await?;
    // A Running campaign driving at High (default core => ungated).
    core.save_campaign(&running_campaign(
        "camp-mqtt",
        "persona-1",
        "TECHNOLOGY",
        0.9,
    )?)
    .await?;

    let (status, efficacy) = core.mqtt_status_snapshot().await?;
    assert_eq!(status.running_campaigns, 1);
    assert!(
        status.running,
        "an actively-driving campaign reads as running"
    );
    assert_eq!(status.intensity, Some(IntensityLevel::High));
    assert_eq!(efficacy.len(), 1);
    assert_eq!(efficacy[0].persona_id, "persona-1");
    assert!((efficacy[0].drift - 0.1).abs() < 1e-9);

    let mock = MockMqtt::new();
    let cfg = MqttConfig::new("ha.lan");
    let object_ids: Vec<String> = efficacy.iter().map(|e| e.object_id()).collect();
    publish_discovery(&mock, &DiscoveryConfig::build(&cfg, &object_ids)).await;
    publish_status(&mock, &cfg, &status, &efficacy).await;

    // Discovery announced the status sensor + one efficacy sensor.
    let disc = mock.discovery_publishes();
    assert_eq!(disc.len(), 2);
    assert!(disc.iter().any(|(t, _)| t.ends_with("/status/config")));
    // State carries the status payload + the efficacy reading (with a drift
    // field; the exact value is asserted on the snapshot above).
    let states = mock.state_publishes();
    assert!(states.iter().any(|(t, _)| *t == cfg.status_state_topic()));
    assert!(states
        .iter()
        .any(|(t, p)| t.contains("/efficacy/") && p.contains("\"drift\":")));
    Ok(())
}

#[tokio::test]
async fn u5_route_campaign_command_through_core() -> Result<()> {
    // #36 AC5: inbound HA commands route into the planner through the exact Core
    // method the serve loop drains its command channel into.
    let dir = tempfile::tempdir()?;
    let core = Core::open(core_config(dir.path())).await?;
    let campaign = Campaign::new(
        "camp-cmd",
        "Cap",
        "persona-1",
        "TECHNOLOGY",
        drift_goal(Comparator::AtMost, 0.5)?,
        0,
    );
    core.save_campaign(&campaign).await?;

    let started = core
        .route_campaign_command(br#"{"action":"start","campaignId":"camp-cmd"}"#, 10)
        .await?;
    assert_eq!(
        started.status,
        fauxx_core::campaigns::CampaignStatus::Running
    );

    let paused = core
        .route_campaign_command(br#"{"action":"pause","campaignId":"camp-cmd"}"#, 20)
        .await?;
    assert_eq!(paused.status, fauxx_core::campaigns::CampaignStatus::Paused);

    // Garbage fails closed (never crashes the loop) and changes nothing.
    assert!(core.route_campaign_command(b"garbage", 30).await.is_err());
    Ok(())
}

/// A tiny `?`-friendly helper to turn an `Option` into a `Result` without
/// `unwrap`/`expect` (the house rule), used only in these tests.
trait ExpectSome<T> {
    fn expect_some(self) -> Result<T>;
}

impl<T> ExpectSome<T> for Option<T> {
    fn expect_some(self) -> Result<T> {
        self.ok_or_else(|| fauxx_core::error::CoreError::Campaign("expected Some, got None".into()))
    }
}
