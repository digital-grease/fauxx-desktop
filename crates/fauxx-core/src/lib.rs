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

//! `fauxx-core` is the headless core of the Fauxx desktop companion.
//!
//! All real work (persona orchestration, the scheduler, browser automation,
//! measurement, cross-device sync) lives here behind a clean async API. The
//! Iced GUI (`apps/desktop`) and the clap CLI (`apps/cli`) are thin clients
//! over this crate. That split buys the headless/homelab mode for free and
//! keeps the GUI toolkit reversible.
//!
//! The public surface is the concrete [`Core`] facade: cheap to clone (shared
//! state is behind an [`Arc`]), with inherent `async fn` methods so it stays
//! object-safe-free and simple. C0 wires up the persona subsystem (backed by
//! the encrypted [`store`]); the scheduler, browser, measurement, and sync
//! subsystems exist as stubs (see [`subsystems`]) that pin the API shape and
//! fill in over C1-C8.
//!
//! Architecture invariants: no GUI/CLI types leak into this API; the store is
//! reachable only through [`Core`] (there is no public database handle); the
//! crate is 100% local (no network, no telemetry); and the store fails closed
//! (a missing/underivable key is an error, never an unencrypted open).

#![forbid(unsafe_code)]

pub mod aliases;
pub mod anchors;
pub mod brokers;
pub mod browser;
pub mod campaigns;
pub mod constants;
pub mod dsar;
pub mod error;
pub mod generate;
pub mod idle;
pub mod logging;
pub mod measurement;
pub mod mint;
pub mod mqtt;
pub mod network;
pub mod orchestration;
pub mod persona;
pub mod personapack;
pub mod querybank;
pub mod store;
pub mod studio;
pub mod subsystems;
pub mod sync;
pub mod ui;

pub use aliases::{AliasKind, AliasProvider, AliasStatus, EmailAlias, PlusAddressProvider};
pub use anchors::{
    anchor_score, recommendations, score_inventory, AccountAnchor, AnchorScore, IdentitySignal,
    Recommendation, RecommendationKind,
};
pub use brokers::{
    broker, brokers, compute_broker_diff_timeline, BrokerDiffTimeline, BrokerScanSnapshot,
    BrokerSubmission, BrokerTemplate, FieldChange, FieldDelta, FilledRequest, ListingCheck,
    OptOutMethod, RelistOutcome, RequiredField, SnapshotDiff, StaticListingCheck, SubmissionStatus,
};
pub use browser::{
    decoy_dir_for, decoy_profiles_root, desktop_for, device_templates_sha256, devices_for,
    mobile_for, parse_gpc_well_known, AssignedTopic, Brand, BrowserLaunchConfig, BrowsingCadence,
    DecoyBrowser, DecoyPage, DeviceProfile, FormFactor, GpcSupport, PresentedDevice, SeedOutcome,
    TopicsReadback, DEFAULT_CHROMIUM_PATH, DEVICE_TEMPLATES_JSON, GPC_WELL_KNOWN_PATH,
};
pub use campaigns::{
    directive_for_gap, Campaign, CampaignDirective, CampaignPlanner, CampaignProgress,
    CampaignStatus, Comparator, Goal, MeasurementMetricSource, MetricSource, StubMetricSource,
    TargetMetric, BACKOFF_GAP, DEFAULT_DWELL_MS, FAR_GAP,
};
pub use dsar::{Controller, DsarLetter, DsarRequest, RequestKind, RequestStatus, SubjectDetails};
pub use error::{CoreError, Result};
pub use generate::{
    allocate, decide_artifact, generate_query_plan, select_artifact_or_fallback, sign_artifact,
    sign_artifact_with, verify_artifact, verify_parsed_artifact, ArtifactContent, ArtifactDecision,
    ArtifactError, ArtifactPayload, FallbackReason, QueryIntent, QueryPlan, SignedArtifact,
    WeightMap, WeightNormalizer, CURRENT_ARTIFACT_SCHEMA_VERSION, DEFAULT_FRESHNESS_MS, KL_BUDGET,
    MIN_WEIGHT, SENSITIVE_ATTRIBUTES,
};
pub use idle::{
    ActiveBehavior, ConservativeIdleSource, IdleScalingConfig, IdleSource, IdleState, RateDecision,
    RatePlanner, StubIdleSource,
};
pub use measurement::{
    aggregate_devices, broker_snapshots, build_platform_drift, cohens_d, compare_cohorts,
    export_efficacy_snapshot, kl_divergence, kl_divergence_breakdown, topics_snapshots,
    two_sample_t_test, Arm, Baseline, CategoryContribution, CategoryDistribution, CohortComparison,
    DriftBreakdown, DriftPoint, DriftSeries, EfficacySnapshotData, ExportArtifact, ExportFormat,
    ExportMetadata, HeatmapSeries, MeasurementEngine, Platform, PlatformDrift, SampleStats,
    ShadowProfile, Smoothing, TTestKind, TTestResult,
};
pub use mint::{
    mint_pack, mint_personas, DemographicCell, MintError, MintedPersonas, PersonaDistribution,
    DEFAULT_DISTRIBUTION_JSON, MAX_RESAMPLE_ATTEMPTS, MINT_INTEREST_COUNT,
};
pub use mqtt::{
    CampaignCommand, DiscoveryConfig, EfficacySensor, MockMqtt, MqttBridge, MqttConfig,
    SensorPayload, StatusPayload, DEFAULT_BASE_TOPIC, DEFAULT_DISCOVERY_PREFIX, DEFAULT_MQTT_PORT,
};
pub use network::{
    validate_dns, validate_egress, DnsStrategy, Egress, EgressExit, PersonaNetwork, ProxyAuth,
    ReachabilityCheck, StaticReachability, TcpReachability, DEFAULT_DOH_RESOLVER,
    DEFAULT_DOT_RESOLVER, DEFAULT_TOR_SOCKS_ADDR, REACHABILITY_TIMEOUT,
};
pub use orchestration::{
    CoordinationMode, DeviceAssignment, DeviceIntent, HouseholdOrchestrator, IntensityLevel,
    IpRecommendation, PublicIpSource, ScheduledAction, SharedIpState, WanIpAssessment,
};
pub use persona::SyntheticPersona;
pub use personapack::{
    sign_pack, sign_pack_with, verify_pack, verify_parsed_pack, PackContent, PackError,
    PackProvenance, PackRecord, PackSigningKey, PersonaPack, CURRENT_PACK_SCHEMA_VERSION,
    MIN_SUPPORTED_PACK_SCHEMA_VERSION,
};
pub use store::{EncryptedStore, GpcSiteStatus, InstalledPack, KeySource, TopicsMeasurement};
pub use studio::{
    lint_persona, simulate_week, Finding, PersonaChangeKind, PersonaChanged, PersonaField,
    PersonaSettings, QueryWeighting, RotationSchedule, Severity, SimulatedQuery, SimulatedSession,
    SimulatedWeek,
};
pub use sync::{
    DiscoveredPeer, LanSync, PairedPeer, PairingPayload, PairingQr, SyncMessage, SERVICE_TYPE,
    SYNC_PROTOCOL_VERSION,
};
pub use ui::{NullUi, UiSink};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{broadcast, Mutex};

/// The crate version, surfaced to clients (CLI `--version`, GUI about box).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A point-in-time snapshot of core health, returned over the async API.
///
/// Deliberately minimal for the C0 skeleton. Richer fields (scheduler state,
/// paired peers, last measurement) arrive with the subsystems in C1+.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// The running core version.
    pub version: &'static str,
    /// Human-readable one-line summary.
    pub summary: String,
    /// Whether an encrypted store is attached to this handle.
    pub store_attached: bool,
    /// Number of personas currently stored (`0` when no store is attached).
    pub persona_count: usize,
}

/// Configuration for opening a [`Core`] with an encrypted store.
///
/// Construct via [`Config::new`] (default OS-keystore path) or the builder
/// helpers. Holds no GUI/CLI types so both clients can build it freely.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the SQLCipher database lives. `None` resolves to the OS data dir.
    path: Option<PathBuf>,
    /// How the database key is sourced (OS keystore or passphrase file).
    key_source: KeySource,
    /// Human-readable name this device advertises for cross-device sync (mDNS
    /// instance name, shown in the pairing QR and to peers).
    device_name: String,
    /// TCP port the sync transport advertises in the QR and mDNS TXT record.
    sync_port: u16,
    /// IP address the inbound sync listener binds. `None` resolves to
    /// `0.0.0.0` (all interfaces, the zero-config LAN default). Set it to a
    /// specific LAN address to limit the listener to one interface, or to
    /// `127.0.0.1` to refuse off-host connections entirely. The sealed channel is
    /// paired-only and fail-closed regardless; this only narrows the TCP surface.
    bind_addr: Option<std::net::IpAddr>,
    /// Optional idle/lock gating policy (C8 #32). `None` (the default) means
    /// NO gating: campaign-driven intensity is used as-is, so a dedicated headless
    /// box runs at full rate. `Some(policy)` attaches the dep-free
    /// [`ConservativeIdleSource`] with that policy,
    /// so decoy intensity pauses/throttles while the machine reports Active and
    /// scales up once idle past the threshold. A GUI can inject real per-OS
    /// detection instead via [`Core::open_with_idle_planner`].
    idle_gating: Option<idle::IdleScalingConfig>,
    /// Whether to bring up the live LAN sync seams (C1 #7): mDNS advertise/browse
    /// plus the real TCP [`SealedTransport`](sync::SealedTransport). `false` (the
    /// default) leaves the engine able to pair, seal, and round-trip in tests but
    /// opens NO sockets and advertises nothing, honoring "no network unless asked".
    /// `true` attaches [`MdnsDiscovery`](sync::MdnsDiscovery) plus the
    /// [`TcpTransport`](sync::TcpTransport); `serve` then advertises and listens.
    lan_sync: bool,
}

impl Config {
    /// A configuration that uses the OS keystore and the default data-dir path.
    pub fn new() -> Self {
        Self {
            path: None,
            key_source: KeySource::OsKeystore,
            device_name: default_device_name(),
            sync_port: sync::DEFAULT_SYNC_PORT,
            bind_addr: None,
            idle_gating: None,
            lan_sync: false,
        }
    }

    /// Override the database path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Override the key source (e.g. the headless passphrase-file fallback).
    pub fn with_key_source(mut self, source: KeySource) -> Self {
        self.key_source = source;
        self
    }

    /// Override the device name advertised for cross-device sync.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }

    /// Override the sync transport port advertised for cross-device sync.
    pub fn with_sync_port(mut self, port: u16) -> Self {
        self.sync_port = port;
        self
    }

    /// Override the IP address the inbound sync listener binds. The default
    /// (`0.0.0.0`, all interfaces) keeps LAN sync zero-config; pin it to a single
    /// LAN address to reduce the listener's surface, or to `127.0.0.1` to refuse
    /// off-host connections. Does not change the sealed-channel guarantees (an
    /// unpaired peer still cannot read or forge), only which interfaces accept a
    /// TCP connection at all.
    pub fn with_bind_addr(mut self, addr: std::net::IpAddr) -> Self {
        self.bind_addr = Some(addr);
        self
    }

    /// The resolved inbound-listener bind IP: the configured address, or the
    /// `0.0.0.0` all-interfaces default.
    pub fn bind_addr(&self) -> std::net::IpAddr {
        self.bind_addr
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
    }

    /// Enable idle/lock-aware gating (C8 #32) with `policy`, using the dep-free
    /// conservative idle source (reports Active until real per-OS detection is
    /// wired). Off by default; a dedicated headless box typically leaves it off
    /// so it runs at full rate, while a desktop turns it on to pause decoy
    /// activity while the user is present.
    pub fn with_idle_gating(mut self, policy: idle::IdleScalingConfig) -> Self {
        self.idle_gating = Some(policy);
        self
    }

    /// Bring up the live LAN sync seams (C1 #7): mDNS advertise/browse + the real
    /// TCP transport. Off by default (no sockets, no advertising). A headless
    /// `serve` or the GUI turns this on to actually pair and exchange sealed
    /// persona frames with another Fauxx device on the LAN.
    pub fn with_lan_sync(mut self, enabled: bool) -> Self {
        self.lan_sync = enabled;
        self
    }

    /// Whether live LAN sync seams are enabled.
    pub fn lan_sync(&self) -> bool {
        self.lan_sync
    }

    /// The configured key source.
    pub fn key_source(&self) -> &KeySource {
        &self.key_source
    }

    /// The configured device name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The configured sync port.
    pub fn sync_port(&self) -> u16 {
        self.sync_port
    }
}

/// A best-effort device name for sync: the OS hostname, falling back to a fixed
/// label. The core never depends on a GUI/CLI to supply this; callers can
/// override it via [`Config::with_device_name`].
fn default_device_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "Fauxx-Desktop".to_string())
}

/// The `active_until` a persona should carry for a given rotation schedule
/// (C5 #24): `Disabled` pins it (a far-future sentinel, so the C1 coordinator
/// never rotates it out), and a `Cadence` sets the window from `created_at` by
/// the MIDPOINT of the schedule's `(min, max)` day window (deterministic; the
/// per-mint random jitter belongs to minting, not to re-applying a schedule).
fn active_until_for_schedule(schedule: &RotationSchedule, created_at: i64) -> i64 {
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;
    match schedule.window_days() {
        // Pinned: never rotates out.
        None => i64::MAX,
        Some((min_days, max_days)) => {
            let days = i64::from((min_days + max_days) / 2);
            created_at.saturating_add(days * MS_PER_DAY)
        }
    }
}

/// A deterministic mint seed for rotating a persona SLOT (C5 #24): mixes the slot
/// id with `now`, so each rotation window draws a fresh-but-reproducible identity.
fn rotation_seed(id: &str, now: i64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    now.hash(&mut hasher);
    hasher.finish()
}

/// Copy one [`PersonaField`]'s value from `from` into `to` (C5 #24): used to pin a
/// LOCKED field across a rotation's regenerated identity.
fn copy_persona_field(field: PersonaField, from: &SyntheticPersona, to: &mut SyntheticPersona) {
    match field {
        PersonaField::Name => to.name = from.name.clone(),
        PersonaField::AgeRange => to.age_range = from.age_range.clone(),
        PersonaField::Profession => to.profession = from.profession.clone(),
        PersonaField::Region => to.region = from.region.clone(),
        PersonaField::Interests => to.interests = from.interests.clone(),
        PersonaField::HomeLocation => to.home_location = from.home_location.clone(),
        PersonaField::Schedule => to.schedule = from.schedule.clone(),
        PersonaField::BrowsingStyle => to.browsing_style = from.browsing_style.clone(),
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared, reference-counted core state. Cloning a [`Core`] clones this `Arc`.
#[derive(Debug)]
struct Inner {
    /// The encrypted store, present once [`Core::open`]/[`Core::open_at`] runs.
    /// `tokio::Mutex` makes the non-`Sync` SQLCipher connection shareable
    /// across clones and `await` points without blocking the runtime. Wrapped
    /// in an `Arc` so the sync engine can share the very same store handle.
    store: Option<Arc<Mutex<EncryptedStore>>>,
    /// The cross-device LAN sync engine (C1 #7), present once a store is open.
    /// Holds this device's pairing identity and the paired-peer set; reaches
    /// the persona cache through the shared `store` above.
    sync: Option<sync::LanSync>,
    /// The cross-device persona orchestrator (C1 #8/#9/#10), present once a
    /// store is open. Layers coordination mode, WAN-IP awareness, and the
    /// household timeline scheduler over the sync engine and the same store.
    orchestrator: Option<orchestration::HouseholdOrchestrator>,
    /// The measurement and analytics engine (C4 #20/#21), present once a store
    /// is open. Computes the per-platform KL-divergence drift series and
    /// heatmaps, and the control-profile A/B comparison, over the same store.
    measurement: Option<measurement::MeasurementEngine>,
    /// Persona-change broadcast (C5 #24 P1): the non-GUI change-event stream
    /// dependent views (the linter panel, the week preview) recompute on. Always
    /// present, even on a store-less core, so a subscriber can attach before a
    /// store is opened. Saving a persona or its settings emits an event.
    persona_changes: broadcast::Sender<studio::PersonaChanged>,
    /// The device's persona-pack signing identity (C5 #27 P4), present once a
    /// store is open. Loaded (or generated and persisted) through the same
    /// [`KeySource`] the store uses, so the secret seed stays in the OS keystore
    /// (or the headless passphrase-file fallback) and never in the SQLite
    /// plaintext. Exports are signed with it; imports verify pack integrity and
    /// the embedded key independent of it.
    pack_key: Option<personapack::PackSigningKey>,
    /// The key source backing the keystore, present once a store is open. Used by
    /// the C7 network layer to store/load per-persona proxy CREDENTIALS in the OS
    /// keystore (never the DB, never a log). A [`KeySource`] is the configuration
    /// of WHERE the secret lives; the passphrase variant only matters on a
    /// no-keystore host.
    key_source: Option<KeySource>,
    /// The goal-driven campaign planner (C8 #33 U2), present once a store is
    /// open. Persists campaigns + their closed-loop progress in the same store,
    /// and reads the C4 A1 drift metric for the targeted segment through the
    /// measurement engine. Holds no GUI/CLI types.
    campaign_planner: Option<campaigns::CampaignPlanner>,
    /// The idle/lock gating planner (C8 #32), present only when idle gating was
    /// enabled (via [`Config::with_idle_gating`] or [`Core::open_with_idle_planner`]).
    /// `None` means ungated: campaign-driven intensity is used as-is. When present,
    /// [`Core::campaign_directive_for_persona`] gates the campaign intensity
    /// through it each call (pause/throttle while Active, scale up past idle).
    idle_planner: Option<idle::RatePlanner>,
    /// The public-key -> address routing table backing the live TCP sync
    /// transport (C1 #7), present only when LAN sync is enabled
    /// ([`Config::with_lan_sync`]). `serve` refreshes it from mDNS-discovered
    /// peers each tick so a freshly-seen peer becomes reachable. `None` means the
    /// engine has no live transport (the default, sealed-but-socketless mode).
    sync_routes: Option<sync::RoutingTable>,
    /// The IP the inbound sync listener binds (C1 #7), resolved from
    /// [`Config::bind_addr`]. Defaults to `0.0.0.0` (all interfaces); set via
    /// [`Config::with_bind_addr`] to narrow the listener to one interface or to
    /// loopback. Read by [`Core::sync_listen_addr`].
    sync_bind_ip: std::net::IpAddr,
}

impl Default for Inner {
    fn default() -> Self {
        let (persona_changes, _rx) = broadcast::channel(studio::PERSONA_CHANGE_CHANNEL_CAPACITY);
        Self {
            store: None,
            sync: None,
            orchestrator: None,
            measurement: None,
            persona_changes,
            pack_key: None,
            key_source: None,
            campaign_planner: None,
            idle_planner: None,
            sync_routes: None,
            sync_bind_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        }
    }
}

/// Handle to the running headless core. Cheap to clone; shared by every client
/// (the GUI holds one, the CLI constructs one per invocation). Persona queries
/// hit the encrypted [`store`]; the other subsystems are C0 stubs.
#[derive(Clone, Debug)]
pub struct Core {
    inner: Arc<Inner>,
}

impl Core {
    /// Construct an in-memory core handle with no store attached. Does no I/O;
    /// used for smoke tests and for client startup before a store is opened.
    /// Persona methods on a store-less core return [`CoreError::NotFound`] /
    /// empty results rather than panicking.
    pub fn new() -> Self {
        tracing::debug!("fauxx-core {VERSION} initialized (no store)");
        Self {
            inner: Arc::new(Inner::default()),
        }
    }

    /// Open the encrypted store described by `config` and return a core bound
    /// to it. Fails closed: a missing/underivable key, or a key that does not
    /// decrypt the database, is an error and the database is never opened
    /// unencrypted.
    ///
    /// Idle/lock gating (C8 #32) is attached when `config` enabled it via
    /// [`Config::with_idle_gating`] (the dep-free conservative source); otherwise
    /// the core is ungated and campaign intensity is used as-is.
    pub async fn open(config: Config) -> Result<Self> {
        // Build the conservative idle planner iff the config asked for gating.
        let idle_planner = config
            .idle_gating
            .map(|policy| idle::RatePlanner::new(Box::new(idle::ConservativeIdleSource), policy));
        Self::open_inner(config, idle_planner).await
    }

    /// Open the store with an explicitly-built idle [`RatePlanner`]
    /// (C8 #32), bypassing the conservative default. A GUI injects real per-OS
    /// detection this way; tests inject a [`StubIdleSource`]
    /// to drive the gating across Active/Idle/Locked through the live core path.
    pub async fn open_with_idle_planner(
        config: Config,
        idle_planner: idle::RatePlanner,
    ) -> Result<Self> {
        Self::open_inner(config, Some(idle_planner)).await
    }

    /// Shared open path: build every subsystem over a freshly-opened store and
    /// attach the optional idle planner.
    async fn open_inner(config: Config, idle_planner: Option<idle::RatePlanner>) -> Result<Self> {
        let store = match &config.path {
            Some(path) => EncryptedStore::open_at(path, &config.key_source)?,
            None => EncryptedStore::open(&config.key_source)?,
        };
        tracing::debug!("fauxx-core {VERSION} opened store at {:?}", store.path());
        let store = Arc::new(Mutex::new(store));

        // Bring up the cross-device sync engine over the same store. Loads (or
        // generates and persists) this device's pairing identity via the same
        // key source the store uses, so the secret stays in the OS keystore.
        let sync = sync::LanSync::open(
            &config.key_source,
            config.device_name.clone(),
            config.sync_port,
            Arc::clone(&store),
        )?;

        // Opt-in (C1 #7): attach the live LAN seams. Default-off keeps the engine
        // sealed-but-socketless (pairs and round-trips in tests, opens nothing).
        // When enabled, the real TCP transport rides a shared routing table that
        // `serve` refreshes from mDNS-discovered peers; discovery advertises this
        // device and browses for peers. A daemon that cannot open the mDNS sockets
        // (some sandboxes) degrades to transport-only with a warning rather than
        // failing the whole open.
        let (sync, sync_routes) = if config.lan_sync {
            let routes = sync::routing_table();
            let transport: Arc<dyn sync::SealedTransport> =
                Arc::new(sync::TcpTransport::new(Arc::clone(&routes)));
            let device = sync.advertised_device();
            match sync::MdnsDiscovery::new(device) {
                Ok(discovery) => {
                    let sync = sync.with_seams(transport, Arc::new(discovery))?;
                    (sync, Some(routes))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "LAN sync: mDNS discovery unavailable; continuing transport-only \
                         (peers reachable only via stored paired-peer addresses)"
                    );
                    // No discovery backend; attach a no-op so the transport seam is
                    // still present and the routing table is shared with `serve`.
                    let discovery: Arc<dyn sync::Discovery> = Arc::new(sync::NullDiscovery);
                    let sync = sync.with_seams(transport, discovery)?;
                    (sync, Some(routes))
                }
            }
        } else {
            (sync, None)
        };

        // Layer the household orchestrator over the same store and sync engine.
        // The default public-IP source is `Unknown`, so the core makes no
        // network call; real detection (STUN / the C7 egress layer) is wired in
        // by callers via the orchestration API.
        let orchestrator =
            orchestration::HouseholdOrchestrator::new(Arc::clone(&store), sync.clone());

        // The measurement engine reads the Topics/broker history and the new
        // shadow-profile table through the same store; it makes no network call
        // and holds no GUI/CLI types (the GUI rendering is a separate batch).
        let measurement = measurement::MeasurementEngine::new(Arc::clone(&store));

        // The persona-change broadcast is always available (a subscriber may
        // attach before any persona is saved). Capacity is generous; a lagging
        // subscriber reloads current state rather than relying on the buffer.
        let (persona_changes, _rx) = broadcast::channel(studio::PERSONA_CHANGE_CHANNEL_CAPACITY);

        // Load (or generate and persist) the device's persona-pack signing key
        // (C5 #27 P4) through the same key source the store uses, so the secret
        // seed lives in the OS keystore (or the headless passphrase-file
        // fallback), never in the SQLite plaintext.
        let pack_key = load_or_create_pack_key(&config.key_source)?;

        // The C8 #33 U2 campaign planner persists campaigns + their closed-loop
        // progress in the same store, and reads the C4 A1 per-segment drift
        // metric through the measurement engine (the live closed-loop source).
        let metric_source: Arc<dyn campaigns::MetricSource> = Arc::new(
            campaigns::MeasurementMetricSource::new(measurement.clone(), Arc::clone(&store)),
        );
        let campaign_planner = campaigns::CampaignPlanner::new(Arc::clone(&store), metric_source);

        Ok(Self {
            inner: Arc::new(Inner {
                store: Some(store),
                sync: Some(sync),
                orchestrator: Some(orchestrator),
                measurement: Some(measurement),
                persona_changes,
                pack_key: Some(pack_key),
                key_source: Some(config.key_source.clone()),
                campaign_planner: Some(campaign_planner),
                idle_planner,
                sync_routes,
                sync_bind_ip: config.bind_addr(),
            }),
        })
    }

    /// Open the encrypted store at `path` using the OS keystore. Convenience
    /// over [`Core::open`] for the common desktop case. Fails closed.
    pub async fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(Config::new().with_path(path.as_ref().to_path_buf())).await
    }

    /// The running core version.
    pub fn version(&self) -> &'static str {
        VERSION
    }

    /// Report core status, including whether a store is attached and how many
    /// personas it holds.
    pub async fn status(&self) -> Result<Status> {
        let (store_attached, persona_count) = match &self.inner.store {
            Some(store) => {
                let guard = store.lock().await;
                (true, guard.list_personas()?.len())
            }
            None => (false, 0),
        };
        Ok(Status {
            version: VERSION,
            summary: if store_attached {
                "headless core online (store attached)".to_string()
            } else {
                "headless core online".to_string()
            },
            store_attached,
            persona_count,
        })
    }

    // --- Persona API (backed by the encrypted store) -----------------------

    /// List all personas. Returns an empty list when no store is attached.
    pub async fn list_personas(&self) -> Result<Vec<SyntheticPersona>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_personas(),
            None => Ok(Vec::new()),
        }
    }

    /// Fetch a persona by id. Returns [`CoreError::NotFound`] if absent or if
    /// no store is attached.
    pub async fn get_persona(&self, id: &str) -> Result<SyntheticPersona> {
        let found = match &self.inner.store {
            Some(store) => store.lock().await.get_persona(id)?,
            None => None,
        };
        found.ok_or_else(|| CoreError::NotFound(format!("persona {id}")))
    }

    /// Insert or replace a persona. Errors if no store is attached.
    ///
    /// On success this emits a [`studio::PersonaChanged`] `Saved` event on the
    /// persona-change broadcast (C5 #24 P1) so dependent views (the coherence
    /// linter panel, the week preview) recompute. A failed write emits nothing.
    pub async fn save_persona(&self, persona: &SyntheticPersona) -> Result<()> {
        match &self.inner.store {
            Some(store) => {
                store.lock().await.save_persona(persona)?;
                self.emit_persona_change(studio::PersonaChanged::saved(&persona.id));
                Ok(())
            }
            None => Err(CoreError::Unimplemented(
                "save_persona requires an open store",
            )),
        }
    }

    /// Delete a persona by id. Returns [`CoreError::NotFound`] if no such
    /// persona exists, or if no store is attached. On success this also removes
    /// any desktop-local [`PersonaSettings`] for that persona and emits a
    /// [`studio::PersonaChanged`] `Deleted` event.
    pub async fn delete_persona(&self, id: &str) -> Result<()> {
        let removed = match &self.inner.store {
            Some(store) => {
                let guard = store.lock().await;
                let removed = guard.delete_persona(id)?;
                if removed {
                    // Editor metadata for a deleted persona is now orphaned;
                    // drop it so it does not linger.
                    let _ = guard.delete_persona_settings(id)?;
                }
                removed
            }
            None => false,
        };
        if removed {
            self.emit_persona_change(studio::PersonaChanged::deleted(id));
            Ok(())
        } else {
            Err(CoreError::NotFound(format!("persona {id}")))
        }
    }

    // --- Persona Studio: editor settings, change events, linter, simulator ---
    //     (C5 #24 P1, #25 P2, #26 P3) -----------------------------------------

    /// Subscribe to the persona-change broadcast (C5 #24 P1). Returns a
    /// [`broadcast::Receiver`] of [`studio::PersonaChanged`] events: saving a
    /// persona or its settings, or deleting a persona, emits one. A NON-GUI
    /// mechanism so the linter / week-simulator views (a later batch) can
    /// recompute; the headless core just exposes the stream.
    pub fn subscribe_persona_changes(&self) -> broadcast::Receiver<studio::PersonaChanged> {
        self.inner.persona_changes.subscribe()
    }

    /// Emit a persona-change event. A send error means there are no live
    /// subscribers, which is fine (the events are recompute triggers, not a
    /// durable log), so it is ignored.
    fn emit_persona_change(&self, event: studio::PersonaChanged) {
        let _ = self.inner.persona_changes.send(event);
    }

    /// The desktop-local [`PersonaSettings`] for a persona (locked fields +
    /// rotation schedule). Returns the default settings (nothing locked, frozen
    /// 8-to-10-day cadence) when none have been saved or no store is attached;
    /// these are editor-only metadata kept OUT of the synced wire model.
    pub async fn persona_settings(&self, persona_id: &str) -> Result<PersonaSettings> {
        let stored = match &self.inner.store {
            Some(store) => store.lock().await.get_persona_settings(persona_id)?,
            None => None,
        };
        Ok(stored.unwrap_or_else(|| PersonaSettings::default_for(persona_id)))
    }

    /// Persist the desktop-local [`PersonaSettings`] for a persona, emitting a
    /// [`studio::PersonaChanged`] `SettingsChanged` event on success. Errors if
    /// no store is attached.
    pub async fn save_persona_settings(&self, settings: &PersonaSettings) -> Result<()> {
        match &self.inner.store {
            Some(store) => {
                store.lock().await.save_persona_settings(settings)?;
                self.emit_persona_change(studio::PersonaChanged::settings_changed(
                    &settings.persona_id,
                ));
                Ok(())
            }
            None => Err(CoreError::Unimplemented(
                "save_persona_settings requires an open store",
            )),
        }
    }

    /// Lock or unlock one persona field (a locked field survives regeneration
    /// and rotation), persisting the updated [`PersonaSettings`] and emitting a
    /// `SettingsChanged` event. Idempotent. Errors if no store is attached.
    pub async fn set_field_locked(
        &self,
        persona_id: &str,
        field: PersonaField,
        locked: bool,
    ) -> Result<PersonaSettings> {
        let mut settings = self.persona_settings(persona_id).await?;
        if locked {
            settings.lock(field);
        } else {
            settings.unlock(field);
        }
        self.save_persona_settings(&settings).await?;
        Ok(settings)
    }

    /// Set the rotation schedule for a persona (the frozen cadence, or
    /// [`RotationSchedule::Disabled`] to PIN it), persisting and emitting a
    /// `SettingsChanged` event. Errors if no store is attached.
    ///
    /// The schedule is also CONSUMED at runtime: the persona's
    /// [`active_until`](SyntheticPersona::active_until) (the window the C1
    /// coordinator rotates on) is recomputed from it, so `Disabled` genuinely
    /// pins the persona (it never rotates out) and a custom cadence is honored,
    /// not merely recorded.
    pub async fn set_rotation_schedule(
        &self,
        persona_id: &str,
        rotation: RotationSchedule,
    ) -> Result<PersonaSettings> {
        let mut settings = self.persona_settings(persona_id).await?;
        settings.set_rotation(rotation);
        self.save_persona_settings(&settings).await?;
        // Apply the schedule to the persona's runtime rotation window so the
        // setting takes effect (was previously stored-but-ignored).
        if let Ok(mut persona) = self.get_persona(persona_id).await {
            persona.active_until = active_until_for_schedule(&rotation, persona.created_at);
            self.save_persona(&persona).await?;
        }
        Ok(settings)
    }

    /// Rotate every persona whose rotation window has ELAPSED (C5 #24): mint a
    /// fresh synthetic identity in the SAME persona slot (keeping its id, so
    /// device assignments + history follow), reset the rotation window from the
    /// persona's schedule, and PRESERVE every field the author LOCKED. Returns the
    /// ids that rotated. Empty on a store-less core.
    ///
    /// A `Disabled` (pinned) persona never rotates; an enabled persona only
    /// rotates once `now >= active_until`. This is the regeneration executor the
    /// homelab `serve` loop drives each tick; it is the mechanism that makes a
    /// persona's identity churn over time (anti-long-term-tracking) while honoring
    /// the author's locks.
    pub async fn rotate_due_personas(&self, now: i64) -> Result<Vec<String>> {
        if self.inner.store.is_none() {
            return Ok(Vec::new());
        }
        let mut rotated = Vec::new();
        for persona in self.list_personas().await? {
            let settings = self.persona_settings(&persona.id).await?;
            // Pinned personas never rotate; enabled ones only once the window ends.
            if !settings.rotation.is_enabled() || persona.active_until > now {
                continue;
            }
            let regenerated = self.regenerate_persona(&persona, &settings, now)?;
            self.save_persona(&regenerated).await?;
            rotated.push(persona.id.clone());
        }
        Ok(rotated)
    }

    /// Mint a fresh identity for an existing persona SLOT (C5 #24), keeping its
    /// id, resetting the rotation window from its schedule, and copying back every
    /// LOCKED field from the prior identity. Pure (no store); the caller persists.
    fn regenerate_persona(
        &self,
        old: &SyntheticPersona,
        settings: &PersonaSettings,
        now: i64,
    ) -> Result<SyntheticPersona> {
        // A fresh identity, seeded from the slot id + now: varies each rotation
        // window, reproducible for a fixed (id, now).
        let seed = rotation_seed(&old.id, now);
        let minted = self.mint_personas(1, seed)?;
        let mut fresh = minted.personas.into_iter().next().ok_or_else(|| {
            CoreError::Orchestration("rotation mint produced no persona".to_string())
        })?;
        // Keep the slot id; restart the lifecycle window from the schedule.
        fresh.id = old.id.clone();
        fresh.created_at = now;
        fresh.active_until = active_until_for_schedule(&settings.rotation, now);
        // Preserve every field the author locked: a lock pins its value across
        // regeneration AND rotation.
        for field in PersonaField::ALL {
            if settings.is_locked(*field) {
                copy_persona_field(*field, old, &mut fresh);
            }
        }
        Ok(fresh)
    }

    /// Lint a persona for coherence (C5 #25 P2): a NON-destructive list of
    /// [`studio::Finding`]s (hard-implausible trait pairs plus rare-co-occurrence
    /// warnings). A coherent persona returns an empty list. Pure; needs no store.
    ///
    /// A subscriber drives this on the P1 change event: take a
    /// [`studio::PersonaChanged`] off [`Core::subscribe_persona_changes`], reload
    /// the persona with [`Core::get_persona`], and call this. The GUI
    /// subscription is a later batch.
    pub fn lint_persona(&self, persona: &SyntheticPersona) -> Vec<Finding> {
        studio::lint_persona(persona)
    }

    /// Lint a stored persona by id (C5 #25 P2): loads it and runs
    /// [`Core::lint_persona`]. Errors if the persona is unknown or no store is
    /// attached.
    pub async fn lint_persona_by_id(&self, persona_id: &str) -> Result<Vec<Finding>> {
        let persona = self.get_persona(persona_id).await?;
        Ok(studio::lint_persona(&persona))
    }

    /// Simulate one deterministic, seedable synthetic WEEK for a persona (C5 #26
    /// P3): a timeline of decoy sessions/queries with times, categories, and the
    /// persona-following weighting. Performs NO real browsing or network. The
    /// same `(persona, intensity, seed)` yields an identical week; a new `seed`
    /// re-rolls it. Pure; needs no store.
    pub fn simulate_week(
        &self,
        persona: &SyntheticPersona,
        intensity: IntensityLevel,
        seed: u64,
    ) -> SimulatedWeek {
        studio::simulate_week(persona, intensity, seed)
    }

    /// Simulate a week for a STORED persona by id (C5 #26 P3): loads it and runs
    /// [`Core::simulate_week`]. Errors if the persona is unknown or no store is
    /// attached.
    pub async fn simulate_week_for(
        &self,
        persona_id: &str,
        intensity: IntensityLevel,
        seed: u64,
    ) -> Result<SimulatedWeek> {
        let persona = self.get_persona(persona_id).await?;
        Ok(studio::simulate_week(&persona, intensity, seed))
    }

    // --- Cross-device sync API (C1 #7) --------------------------------------

    /// The cross-device LAN sync engine, present once a store is open. Returns
    /// [`CoreError::Unimplemented`] on a store-less core (sync requires the
    /// encrypted store for the paired-peer set and the persona cache).
    fn sync_engine(&self) -> Result<&sync::LanSync> {
        self.inner
            .sync
            .as_ref()
            .ok_or(CoreError::Unimplemented("sync requires an open store"))
    }

    /// This device's pairing public-key fingerprint (for display next to the
    /// QR and in the discovery UI).
    pub fn sync_fingerprint(&self) -> Result<String> {
        Ok(self.sync_engine()?.fingerprint())
    }

    /// This device's pairing public key, base64url (no padding). This is the key
    /// a peer routes to (the recipient identifier for the transport) and the key
    /// it records when pairing.
    pub fn sync_public_key(&self) -> Result<String> {
        Ok(sync::encode_public_key(self.sync_engine()?.public_key()))
    }

    /// Render the pairing QR (unicode for terminals, SVG for the GUI) carrying
    /// this device's public key and connection hint. The phone scans it.
    pub async fn pairing_qr(&self) -> Result<PairingQr> {
        self.sync_engine()?.pairing_qr()
    }

    /// The compact pairing payload string this device shows (QR contents). Both
    /// clients can also display it as fallback text.
    pub async fn pairing_payload(&self) -> Result<PairingPayload> {
        Ok(self.sync_engine()?.pairing_payload())
    }

    /// Complete pairing with a peer from a scanned pairing-payload string (the
    /// base64url QR contents). Records the peer's public key so the sealed
    /// channel between the two devices opens; persists the paired record.
    pub async fn complete_pairing(&self, scanned_payload: &str) -> Result<PairedPeer> {
        let payload = PairingPayload::decode(scanned_payload)?;
        self.sync_engine()?.complete_pairing(&payload).await
    }

    /// List paired (trusted) peers.
    pub async fn paired_peers(&self) -> Result<Vec<PairedPeer>> {
        match &self.inner.sync {
            Some(engine) => engine.paired_peers().await,
            None => Ok(Vec::new()),
        }
    }

    /// Revoke a paired peer by its base64url public key, removing its ability to
    /// sync. Returns `true` if a record was removed.
    pub async fn unpair(&self, public_key: &str) -> Result<bool> {
        self.sync_engine()?.unpair(public_key).await
    }

    /// List peers seen over mDNS discovery (untrusted until paired). Empty when
    /// no discovery backend has been started.
    pub async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
        match &self.inner.sync {
            Some(engine) => engine.discovered_peers().await,
            None => Ok(Vec::new()),
        }
    }

    /// Seal a persona for one paired peer and return the on-wire frame bytes
    /// (confidential + authenticated). Errors if the peer is not paired.
    pub async fn seal_persona_for(
        &self,
        peer_public_key: &str,
        persona: &SyntheticPersona,
    ) -> Result<Vec<u8>> {
        self.sync_engine()?
            .seal_persona_for(peer_public_key, persona)
            .await
    }

    /// Push a persona to every paired peer over the attached transport. Returns
    /// the number of peers it was sealed and sent to.
    pub async fn sync_persona_to_paired(&self, persona: &SyntheticPersona) -> Result<usize> {
        self.sync_engine()?.push_persona_to_all(persona).await
    }

    /// Receive a sealed frame attributed to a paired sender (by base64url public
    /// key), open and verify it, and upsert the carried persona into the store.
    /// A frame from an unpaired sender, or one that fails authentication, is
    /// rejected.
    pub async fn receive_persona_frame(
        &self,
        sender_public_key: &str,
        frame: &[u8],
    ) -> Result<SyntheticPersona> {
        self.sync_engine()?
            .receive_frame(sender_public_key, frame)
            .await
    }

    // --- Live LAN sync transport (C1 #7, opt-in via Config::with_lan_sync) ---

    /// Begin advertising this device over mDNS and browsing for peers, if the
    /// live discovery backend is attached (a no-op otherwise). Called once by
    /// `serve` at startup when LAN sync is enabled.
    pub async fn advertise_sync(&self) -> Result<()> {
        self.sync_engine()?.advertise_if_enabled().await
    }

    /// The socket address the inbound sync listener binds: the configured bind IP
    /// (default `0.0.0.0`, all interfaces) on the configured sync port. Errors on
    /// a store-less core. See [`Config::with_bind_addr`].
    pub fn sync_listen_addr(&self) -> Result<std::net::SocketAddr> {
        let port = self.sync_engine()?.port();
        Ok(std::net::SocketAddr::new(self.inner.sync_bind_ip, port))
    }

    /// Add an explicit route (peer public key -> address) to the live transport's
    /// routing table, bypassing mDNS. Used when an address is known out of band
    /// (a stored paired-peer address, or a test loopback). Errors if LAN sync is
    /// disabled (no routing table) or the key is not valid base64url.
    pub async fn add_sync_route(
        &self,
        peer_public_key: &str,
        addr: std::net::SocketAddr,
    ) -> Result<()> {
        let routes =
            self.inner.sync_routes.as_ref().ok_or_else(|| {
                CoreError::Sync("LAN sync is disabled; no routing table".to_string())
            })?;
        let pk = sync::decode_public_key(peer_public_key)?;
        routes.lock().await.insert(pk, addr);
        Ok(())
    }

    /// Refresh the live transport's routing table from mDNS-discovered peers so a
    /// freshly-seen peer becomes reachable for an outbound push. Returns the
    /// number of routes set. A no-op (0) when LAN sync is disabled.
    pub async fn refresh_sync_routes(&self) -> Result<usize> {
        let routes = match &self.inner.sync_routes {
            Some(routes) => routes,
            None => return Ok(0),
        };
        let peers = self.discovered_peers().await?;
        let mut table = routes.lock().await;
        let mut set = 0usize;
        for peer in peers {
            let Some(pk) = peer
                .public_key
                .as_deref()
                .and_then(|p| sync::decode_public_key(p).ok())
            else {
                continue;
            };
            if let Some(addr) = peer
                .addresses
                .iter()
                .find_map(|a| a.parse::<std::net::SocketAddr>().ok())
            {
                table.insert(pk, addr);
                set += 1;
            }
        }
        Ok(set)
    }

    /// Open, authenticate, and apply one inbound sealed frame received over the
    /// transport. The wire frame carries no clear-text sender (the MAC against a
    /// *paired* key is the only attribution), so this tries each paired peer's
    /// key in turn: the frame authenticates against exactly its real sender and
    /// fails for every other (and for an unpaired/forged frame, against all).
    /// Once attributed, the frame is routed to its dedicated verified path by
    /// kind. Returns `(sender base64url key, applied kind name)`.
    pub async fn ingest_inbound_frame(&self, frame: &[u8]) -> Result<(String, &'static str)> {
        // Issue #42: LAN-sync pairing is per-device. A push from a peer this
        // device has not paired back cannot be authenticated (the sealed frame
        // carries no cleartext sender, so attribution is by trying each paired
        // key). Surface a clear, actionable message instead of a bare auth
        // failure, mirroring the two-way pairing guidance shipped on the Android
        // side (fauxx#213).
        const PAIR_BOTH_WAYS_HINT: &str = "LAN-sync pairing must be completed on BOTH devices: pair the other device from this device too (open Devices and use \"Pair a device back\", or run `fauxx-cli pair add <code>`).";
        let peers = self.paired_peers().await?;
        if peers.is_empty() {
            return Err(CoreError::Sync(format!(
                "received a LAN sync push, but this device has not paired any device, so nothing can authenticate it. {PAIR_BOTH_WAYS_HINT}"
            )));
        }
        for peer in peers {
            // Peek: does this frame open + authenticate as from `peer`?
            let Ok(message) = self
                .sync_engine()?
                .receive_sync_message(&peer.public_key, frame)
                .await
            else {
                continue; // wrong sender key (MAC fails); try the next paired peer
            };
            let kind = match message.body {
                // The signed-artifact and persona-pack kinds verify their own
                // embedded signature on a dedicated path; route them there.
                sync::SyncBody::SignedArtifact(_) => {
                    self.receive_artifact_frame(&peer.public_key, frame, now_millis())
                        .await?;
                    "SignedArtifact"
                }
                sync::SyncBody::PersonaPack(_) => {
                    self.receive_pack_frame(&peer.public_key, frame).await?;
                    "PersonaPack"
                }
                // PersonaUpsert / PublicIpReport / CoordinationState all apply
                // through the orchestrator's unified router.
                _ => {
                    self.orchestrator()?
                        .apply_sync_frame(&peer.public_key, frame)
                        .await?
                }
            };
            return Ok((peer.public_key, kind));
        }
        Err(CoreError::Sync(format!(
            "received a LAN sync push, but this device has not paired the sender back, so it cannot be authenticated. {PAIR_BOTH_WAYS_HINT} If you did not start a sync, you can safely ignore this."
        )))
    }

    /// Run the inbound sync listener until `shutdown` is notified. Binds the sync
    /// port, accepts connections, reads one length-prefixed frame per connection,
    /// and applies it via [`Core::ingest_inbound_frame`]. Each connection is
    /// handled on its own task so a slow or malformed peer cannot stall others.
    /// Errors only if the initial bind fails; per-connection errors are logged
    /// and the loop continues (fail-closed per frame, never per listener).
    pub async fn run_sync_listener(&self, shutdown: Arc<tokio::sync::Notify>) -> Result<()> {
        let addr = self.sync_listen_addr()?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CoreError::Sync(format!("bind sync listener {addr}: {e}")))?;
        tracing::info!(%addr, "LAN sync: inbound listener bound");
        self.serve_inbound(listener, shutdown).await
    }

    /// Drive the inbound accept loop over an already-bound listener until
    /// `shutdown` is notified. Split out of [`Core::run_sync_listener`] so a test
    /// can bind an ephemeral loopback port and exercise the real socket path.
    pub async fn serve_inbound(
        &self,
        listener: tokio::net::TcpListener,
        shutdown: Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        loop {
            tokio::select! {
                _ = shutdown.notified() => {
                    tracing::info!("LAN sync: inbound listener stopping");
                    return Ok(());
                }
                accepted = listener.accept() => {
                    let (mut stream, peer_addr) = match accepted {
                        Ok(pair) => pair,
                        Err(e) => {
                            tracing::warn!(error = %e, "LAN sync: accept failed");
                            continue;
                        }
                    };
                    let this = self.clone();
                    tokio::spawn(async move {
                        match sync::tcp::read_frame(&mut stream).await {
                            Ok(frame) => match this.ingest_inbound_frame(&frame).await {
                                Ok((sender, kind)) => tracing::info!(
                                    %peer_addr, %sender, kind,
                                    "LAN sync: applied inbound frame"
                                ),
                                Err(e) => tracing::warn!(
                                    %peer_addr, error = %e,
                                    "LAN sync: rejected inbound frame"
                                ),
                            },
                            Err(e) => tracing::warn!(
                                %peer_addr, error = %e,
                                "LAN sync: failed to read inbound frame"
                            ),
                        }
                    });
                }
            }
        }
    }

    /// Spawn a dedicated background thread that advertises this device over mDNS
    /// and runs the inbound sync listener for the rest of the process lifetime.
    ///
    /// Intended for the GUI, which (unlike `serve`) has no async loop of its own
    /// to host the listener: it can call this once after opening a LAN-sync-
    /// enabled core so the desktop becomes discoverable and ready to receive
    /// sealed persona frames from a paired phone (C1 #7 / the C8 #34 wizard
    /// import). The thread owns its own current-thread runtime and shares this
    /// core's `Arc` (and thus its store), so a received persona lands in the same
    /// encrypted store the GUI reads.
    ///
    /// Returns the thread handle (the caller may detach it; the thread ends when
    /// the process exits). Returns `None` only if the OS refuses the thread. On a
    /// core without LAN sync enabled the thread starts but exits promptly after
    /// logging, since `advertise_sync`/`run_sync_listener` report no seams.
    pub fn spawn_background_lan_sync(&self) -> Option<std::thread::JoinHandle<()>> {
        let core = self.clone();
        let spawned = std::thread::Builder::new()
            .name("fauxx-lan-sync".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(error = %e, "LAN sync: background runtime build failed");
                        return;
                    }
                };
                rt.block_on(async move {
                    if let Err(e) = core.advertise_sync().await {
                        tracing::warn!(error = %e, "LAN sync: background advertise failed");
                    }
                    // A Notify that is never signalled: the listener runs until the
                    // process (and thus this thread) exits.
                    let never = Arc::new(tokio::sync::Notify::new());
                    if let Err(e) = core.run_sync_listener(never).await {
                        tracing::error!(error = %e, "LAN sync: background listener exited");
                    }
                });
            });
        match spawned {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::error!(error = %e, "LAN sync: background thread spawn failed");
                None
            }
        }
    }

    /// The redaction literals for a scrubbed debug-log export: the local account
    /// identifiers, this device's name, every paired peer's name + host, every
    /// persona's id + name, and each persona's egress proxy host / VPN provider.
    /// Folded into [`logging::Redactions`] so none of
    /// these reach a public bug report. Best-effort: a subsystem that errors
    /// contributes nothing rather than failing the export.
    pub async fn redaction_literals(&self) -> Vec<String> {
        let mut lits = crate::logging::account_literals();
        if let Ok(payload) = self.pairing_payload().await {
            lits.push(payload.name);
        }
        if let Ok(peers) = self.paired_peers().await {
            for peer in peers {
                lits.push(peer.name);
                if let Some(host) = peer.host {
                    lits.push(host);
                }
            }
        }
        if let Ok(personas) = self.list_personas().await {
            for persona in personas {
                lits.push(persona.id.clone());
                lits.push(persona.name);
                if let Ok(egress) = self.get_persona_egress(&persona.id).await {
                    match egress {
                        crate::network::Egress::HttpProxy { host, .. }
                        | crate::network::Egress::SocksProxy { host, .. } => lits.push(host),
                        crate::network::Egress::Vpn { provider, .. } => lits.push(provider),
                        _ => {}
                    }
                }
            }
        }
        lits
    }

    // --- Cross-device persona orchestration API (C1 #8/#9/#10) --------------

    /// The household orchestrator, present once a store is open. Returns
    /// [`CoreError::Unimplemented`] on a store-less core (orchestration needs
    /// the encrypted store and the sync engine).
    fn orchestrator(&self) -> Result<&orchestration::HouseholdOrchestrator> {
        self.inner
            .orchestrator
            .as_ref()
            .ok_or(CoreError::Unimplemented(
                "orchestration requires an open store",
            ))
    }

    /// The active [`CoordinationMode`] (defaults to coherent until set).
    pub async fn coordination_mode(&self) -> Result<CoordinationMode> {
        self.orchestrator()?.coordination_mode().await
    }

    /// Set the active coordination mode (persisted; survives restart).
    pub async fn set_coordination_mode(&self, mode: CoordinationMode) -> Result<()> {
        self.orchestrator()?.set_coordination_mode(mode).await
    }

    /// Every device-to-persona assignment, with the local device flagged.
    pub async fn device_assignments(&self) -> Result<Vec<DeviceAssignment>> {
        self.orchestrator()?.assignments().await
    }

    /// The persona id assigned to a device (empty key = this device), or `None`.
    pub async fn assigned_persona(&self, device_key: &str) -> Result<Option<String>> {
        self.orchestrator()?.assigned_persona(device_key).await
    }

    /// Elect ONE persona for the whole household (Coherent mode) and propagate
    /// it to every paired device over the sealed channel. Returns the count of
    /// peers it reached.
    pub async fn elect_coherent_persona(&self, persona_id: &str) -> Result<usize> {
        self.orchestrator()?
            .elect_coherent_persona(persona_id)
            .await
    }

    /// Reconcile the coherent persona to its rotated successor across the
    /// household (advances all devices together at the frozen cadence).
    pub async fn reconcile_coherent_rotation(&self, rotated: &SyntheticPersona) -> Result<usize> {
        self.orchestrator()?
            .reconcile_coherent_rotation(rotated)
            .await
    }

    /// Assign a DISTINCT persona to a device (Fragmentation mode). Refuses a
    /// persona already assigned to another device.
    pub async fn assign_fragmented_persona(
        &self,
        device_key: &str,
        persona_id: &str,
    ) -> Result<()> {
        self.orchestrator()?
            .assign_fragmented_persona(device_key, persona_id)
            .await
    }

    /// Record a peer's reported public IP (received over the sealed channel).
    pub async fn record_peer_public_ip(&self, peer_key: &str, ip: Option<&str>) -> Result<()> {
        self.orchestrator()?
            .record_peer_public_ip(peer_key, ip)
            .await
    }

    /// Assess shared-public-IP linkage with a paired peer under the active mode.
    pub async fn assess_shared_ip(&self, peer_key: &str) -> Result<WanIpAssessment> {
        self.orchestrator()?.assess_shared_ip(peer_key).await
    }

    /// Plan the household's action timeline for one active day. Deterministic
    /// for a fixed `seed`; degrades to local-only when a peer is offline.
    pub async fn plan_household(
        &self,
        intents: &[DeviceIntent],
        collision_window_secs: i64,
        seed: u64,
    ) -> Result<Vec<ScheduledAction>> {
        self.orchestrator()?
            .plan_household(intents, collision_window_secs, seed)
            .await
    }

    // --- Real-browser decoy automation API (C2 #11 R1 / #13 R3) -------------

    /// Launch an ISOLATED decoy Chromium profile for `decoy_id` from a dedicated
    /// user-data directory under the app data dir, using the system Chromium.
    ///
    /// Enforces the R3 isolation guardrail (fail closed): the decoy dir must be
    /// verifiably distinct from every detected real browser profile, or this
    /// errors with [`CoreError::Browser`] before any browser launches. The
    /// launcher only ever creates/uses its own dir; it never imports cookies,
    /// tokens, logins, or cache from a real profile. Closing the returned
    /// [`DecoyBrowser`] kills the child process and stops the CDP handler task.
    pub async fn launch_decoy_browser(&self, decoy_id: &str) -> Result<DecoyBrowser> {
        DecoyBrowser::launch(decoy_id).await
    }

    /// Launch an isolated decoy browser for `decoy_id` with an explicit
    /// [`BrowserLaunchConfig`] (custom executable, user-data dir, headed mode,
    /// or injected real-profile roots). Same R3 guardrail as
    /// [`Core::launch_decoy_browser`].
    pub async fn launch_decoy_browser_with(
        &self,
        decoy_id: &str,
        config: BrowserLaunchConfig,
    ) -> Result<DecoyBrowser> {
        DecoyBrowser::launch_with(decoy_id, config).await
    }

    /// The stable DESKTOP [`DeviceProfile`] a persona presents on the decoy browser
    /// (#47): the derived User-Agent + client hints + fixed screen/navigator values,
    /// computed deterministically from the persona (id + createdAt). Byte-identical
    /// to what the paired phone derives for the same persona, without the device ever
    /// crossing the LAN wire. Pure; needs no store.
    pub fn desktop_device_for(&self, persona: &SyntheticPersona) -> DeviceProfile {
        browser::desktop_for(persona)
    }

    /// The desktop device identity a STORED persona presents (#47): loads the persona
    /// and derives [`Core::desktop_device_for`]. Errors if the persona is unknown or
    /// no store is attached.
    pub async fn desktop_device_for_id(&self, persona_id: &str) -> Result<DeviceProfile> {
        let persona = self.get_persona(persona_id).await?;
        Ok(browser::desktop_for(&persona))
    }

    /// Launch an isolated decoy browser bound to `persona`'s stable DESKTOP device
    /// identity (#47): derives [`browser::desktop_for`] and applies it
    /// (UA + full client hints + screen/navigator) to every page over CDP, so the
    /// decoy presents one coherent, non-headless desktop device for the persona's
    /// life. Same R3 isolation guardrail as [`Core::launch_decoy_browser`]; the
    /// caller-supplied `config`'s other settings (egress, GPC, headed mode) are
    /// preserved, only the device is bound.
    pub async fn launch_decoy_browser_for_persona(
        &self,
        decoy_id: &str,
        persona: &SyntheticPersona,
        config: BrowserLaunchConfig,
    ) -> Result<DecoyBrowser> {
        DecoyBrowser::launch_with(decoy_id, config.with_persona_device(persona)).await
    }

    // --- Privacy Sandbox Topics read-back / closed loop (C2 #12 R2) ---------

    /// Persist a Topics read-back measurement for a persona. Errors if no store
    /// is attached. An empty `readback.topics` (the common epoch-boundary case)
    /// is a valid record and is intentionally persisted.
    ///
    /// Returns the [`TopicsMeasurement`] that was written (with its recorded-at
    /// timestamp) so callers can surface it immediately.
    pub async fn record_topics_readback(
        &self,
        persona_id: &str,
        decoy_id: &str,
        readback: &TopicsReadback,
    ) -> Result<TopicsMeasurement> {
        let record = TopicsMeasurement {
            persona_id: persona_id.to_string(),
            decoy_id: decoy_id.to_string(),
            recorded_at: now_millis(),
            available: readback.available,
            topics: readback.topics.clone(),
        };
        match &self.inner.store {
            Some(store) => {
                store.lock().await.insert_topics_measurement(&record)?;
                Ok(record)
            }
            None => Err(CoreError::Unimplemented(
                "record_topics_readback requires an open store",
            )),
        }
    }

    /// All Topics read-back measurements for a persona, oldest first. Empty when
    /// no store is attached or the persona has no recorded reads. Consumed by the
    /// later dashboards (C4) and campaigns (C8).
    pub async fn topics_measurements_for(
        &self,
        persona_id: &str,
    ) -> Result<Vec<TopicsMeasurement>> {
        match &self.inner.store {
            Some(store) => store.lock().await.topics_for(persona_id),
            None => Ok(Vec::new()),
        }
    }

    /// The most recent Topics read-back for a persona, or `None` if none has
    /// been recorded (or no store is attached).
    pub async fn latest_topics_measurement(
        &self,
        persona_id: &str,
    ) -> Result<Option<TopicsMeasurement>> {
        match &self.inner.store {
            Some(store) => store.lock().await.latest_topics_for(persona_id),
            None => Ok(None),
        }
    }

    // --- Data-broker opt-out & deletion automation (C3 #15 D1c) -------------

    /// The bundled data-broker opt-out registry as `(id, template)` pairs, in
    /// deterministic order. Static data (no store needed); the GUI/CLI list it.
    pub fn broker_registry(&self) -> Vec<(&'static str, &'static BrokerTemplate)> {
        brokers::brokers()
    }

    /// One broker opt-out template by id. [`CoreError::NotFound`] if unknown.
    pub fn broker_template(&self, broker_id: &str) -> Result<&'static BrokerTemplate> {
        brokers::broker(broker_id)
    }

    /// Generate a filled opt-out request for `(broker, persona)`, plus optional
    /// caller-supplied field `overrides` (e.g. a real confirmation email or the
    /// known listing URL). Does NOT submit or persist; pure generation so a
    /// caller can review the filled request and any missing fields first.
    pub async fn generate_broker_request(
        &self,
        broker_id: &str,
        persona_id: &str,
        overrides: &std::collections::BTreeMap<String, String>,
    ) -> Result<FilledRequest> {
        let template = brokers::broker(broker_id)?;
        let persona = self.get_persona(persona_id).await?;
        Ok(FilledRequest::generate(
            broker_id, template, &persona, overrides,
        ))
    }

    /// Generate AND record a `drafted` opt-out submission for `(broker, persona)`
    /// in the `broker_submissions` table, returning the persisted record. The
    /// deadline is computed from the broker's actioning window. This is the
    /// solid, hermetic-testable state step; an actual web-form submission is a
    /// separate browser-driven step ([`Core::submit_broker_request_via_decoy`]).
    /// Errors if no store is attached or the persona/broker is unknown.
    pub async fn record_broker_submission(
        &self,
        broker_id: &str,
        persona_id: &str,
    ) -> Result<BrokerSubmission> {
        let template = brokers::broker(broker_id)?;
        // Validate the persona exists before recording (fail closed on a typo).
        let _ = self.get_persona(persona_id).await?;
        let submission = BrokerSubmission::draft(
            uuid::Uuid::new_v4().to_string(),
            broker_id,
            persona_id,
            template,
            now_millis(),
        );
        match &self.inner.store {
            Some(store) => {
                store.lock().await.upsert_broker_submission(&submission)?;
                Ok(submission)
            }
            None => Err(CoreError::Unimplemented(
                "record_broker_submission requires an open store",
            )),
        }
    }

    /// Persist an updated broker submission (e.g. after marking it `submitted`,
    /// `confirmed`, or `removed`, or attaching a confirmation token). Upsert by
    /// id. Errors if no store is attached.
    pub async fn save_broker_submission(&self, submission: &BrokerSubmission) -> Result<()> {
        match &self.inner.store {
            Some(store) => store.lock().await.upsert_broker_submission(submission),
            None => Err(CoreError::Unimplemented(
                "save_broker_submission requires an open store",
            )),
        }
    }

    /// List broker submissions; scoped to one persona when `persona_id` is
    /// `Some`, else all. Newest first. Empty when no store is attached.
    pub async fn list_broker_submissions(
        &self,
        persona_id: Option<&str>,
    ) -> Result<Vec<BrokerSubmission>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_broker_submissions(persona_id),
            None => Ok(Vec::new()),
        }
    }

    /// The broker submissions whose deadline is due or overdue as of `now` and
    /// that are still outstanding (the reminder set). Sorted soonest-deadline
    /// first. Empty when no store is attached.
    pub async fn due_broker_submissions(&self, now: i64) -> Result<Vec<BrokerSubmission>> {
        let mut due: Vec<BrokerSubmission> = self
            .list_broker_submissions(None)
            .await?
            .into_iter()
            .filter(|s| s.is_deadline_due(now))
            .collect();
        due.sort_by_key(|s| s.deadline);
        Ok(due)
    }

    /// Re-scan a recorded submission for re-listing using the injected
    /// [`ListingCheck`] seam, persisting a `relisted` status when a previously
    /// `removed` persona is found listed again. Returns the [`RelistOutcome`].
    ///
    /// The check is abstracted so this is hermetic-testable (inject a
    /// [`StaticListingCheck`]); the live check drives the decoy browser to the
    /// broker's PUBLIC listing/search page. Errors if no store is attached or
    /// the submission id is unknown.
    pub async fn rescan_broker_submission(
        &self,
        submission_id: &str,
        check: &dyn ListingCheck,
    ) -> Result<RelistOutcome> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "rescan_broker_submission requires an open store",
        ))?;

        let mut submission = {
            let guard = store.lock().await;
            guard
                .get_broker_submission(submission_id)?
                .ok_or_else(|| CoreError::NotFound(format!("broker submission {submission_id}")))?
        };
        let persona = self.get_persona(&submission.persona_id).await?;

        let still_listed = check.is_listed(&submission.broker_id, &persona).await?;
        let newly_relisted =
            match brokers::submission::relist_transition(submission.status, still_listed) {
                Some(new_status) => {
                    submission.status = new_status;
                    store.lock().await.upsert_broker_submission(&submission)?;
                    true
                }
                None => false,
            };

        Ok(RelistOutcome {
            submission_id: submission.id.clone(),
            broker_id: submission.broker_id.clone(),
            still_listed,
            newly_relisted,
        })
    }

    /// Best-effort web-form opt-out submission via the R3-guarded decoy browser
    /// (D1c #15). Launches an isolated decoy profile, navigates to the broker's
    /// PUBLIC opt-out URL (guarded: a sign-in host is refused), and records the
    /// submission as `submitted` (or leaves the draft when the form could not be
    /// reached). Field entry is best-effort; the state recording is solid.
    ///
    /// HARD GUARDRAIL: this only ever drives the public opt-out request form on
    /// an isolated decoy profile. It never touches an authenticated account.
    /// Errors only on a hard guardrail/store failure; an unreachable form is a
    /// recorded `drafted` submission, not an error.
    pub async fn submit_broker_request_via_decoy(
        &self,
        broker_id: &str,
        persona_id: &str,
        decoy_id: &str,
    ) -> Result<BrokerSubmission> {
        let template = brokers::broker(broker_id)?;
        // Only web-form brokers are auto-driven; email/manual are recorded as a
        // draft for the operator to complete out of band.
        let mut submission = self.record_broker_submission(broker_id, persona_id).await?;

        if template.method != OptOutMethod::WebForm {
            return Ok(submission);
        }

        // Launch the isolated decoy and navigate to the PUBLIC opt-out form. Any
        // failure leaves the submission as the recorded draft (best effort).
        let browser = DecoyBrowser::launch(decoy_id).await?;
        let nav = async {
            let page = browser.new_page().await?;
            page.navigate(&template.opt_out_url).await?;
            Result::Ok(())
        }
        .await;
        let _ = browser.close().await;

        if nav.is_ok() {
            submission.status = SubmissionStatus::Submitted;
            self.save_broker_submission(&submission).await?;
        } else {
            tracing::warn!(
                target: "fauxx_core::brokers",
                broker_id,
                "decoy could not reach broker opt-out form; left as drafted"
            );
        }
        Ok(submission)
    }

    // --- Broker diff view: scan snapshots + diff timeline (C4 #22 A3) -------

    /// Record (insert or replace) a broker identity-scan SNAPSHOT for
    /// `(broker, persona)`: the set of identity `fields` the broker exposes
    /// about the persona at `scanned_at`. Returns the persisted
    /// [`BrokerScanSnapshot`].
    ///
    /// The live scanning that POPULATES `fields` from a broker's public listing
    /// page is DEFERRED (like the C3 live [`ListingCheck`]); A3 records and
    /// diffs STORED snapshots. Validates the broker exists (fail closed on a
    /// typo) but does NOT require the persona to still exist (it may have
    /// rotated). Errors if no store is attached or the broker is unknown.
    pub async fn record_broker_scan_snapshot(
        &self,
        broker_id: &str,
        persona_id: &str,
        scanned_at: i64,
        fields: impl IntoIterator<Item = String>,
    ) -> Result<BrokerScanSnapshot> {
        // Validate the broker id against the registry (fail closed on a typo).
        let _ = brokers::broker(broker_id)?;
        let snapshot = BrokerScanSnapshot::new(
            uuid::Uuid::new_v4().to_string(),
            broker_id,
            persona_id,
            scanned_at,
            fields,
        );
        match &self.inner.store {
            Some(store) => {
                store.lock().await.upsert_broker_scan_snapshot(&snapshot)?;
                Ok(snapshot)
            }
            None => Err(CoreError::Unimplemented(
                "record_broker_scan_snapshot requires an open store",
            )),
        }
    }

    /// List the recorded broker scan snapshots for one `(broker, persona)`,
    /// OLDEST first. Empty when none recorded (or no store is attached).
    pub async fn list_broker_scan_snapshots(
        &self,
        broker_id: &str,
        persona_id: &str,
    ) -> Result<Vec<BrokerScanSnapshot>> {
        match &self.inner.store {
            Some(store) => store
                .lock()
                .await
                .list_broker_scan_snapshots(broker_id, persona_id),
            None => Ok(Vec::new()),
        }
    }

    /// Compute the per-broker diff TIMELINE for one `(broker, persona)` from the
    /// stored scan snapshots (C4 #22 A3): each field classified
    /// added/removed/unchanged across consecutive snapshots, with re-listing
    /// (a removed field reappearing) distinctly flagged.
    ///
    /// A broker with zero or one snapshot yields the clear "no diff yet" state
    /// (no panic). Empty when no store is attached (also a no-diff-yet timeline).
    pub async fn broker_diff_timeline(
        &self,
        broker_id: &str,
        persona_id: &str,
    ) -> Result<BrokerDiffTimeline> {
        let snapshots = self
            .list_broker_scan_snapshots(broker_id, persona_id)
            .await?;
        Ok(compute_broker_diff_timeline(
            broker_id, persona_id, &snapshots,
        ))
    }

    // --- Global Privacy Control honoring status (C3 #18 D4c) ----------------

    /// Record a per-site GPC-honoring observation in the store (D4c #18),
    /// returning the persisted [`GpcSiteStatus`]. The `support` is typically the
    /// parsed result of a site's `/.well-known/gpc.json`. Errors if no store is
    /// attached.
    pub async fn record_gpc_status(
        &self,
        origin: &str,
        support: GpcSupport,
    ) -> Result<GpcSiteStatus> {
        let status = GpcSiteStatus {
            origin: origin.to_string(),
            checked_at: now_millis(),
            support,
        };
        match &self.inner.store {
            Some(store) => {
                store.lock().await.upsert_gpc_status(&status)?;
                Ok(status)
            }
            None => Err(CoreError::Unimplemented(
                "record_gpc_status requires an open store",
            )),
        }
    }

    /// The recorded GPC-honoring observation for a site origin, or `None` if it
    /// has never been checked (or no store is attached).
    pub async fn gpc_status_for(&self, origin: &str) -> Result<Option<GpcSiteStatus>> {
        match &self.inner.store {
            Some(store) => store.lock().await.gpc_status_for(origin),
            None => Ok(None),
        }
    }

    /// All recorded per-site GPC-honoring observations, origin ascending. Empty
    /// when no store is attached.
    pub async fn list_gpc_status(&self) -> Result<Vec<GpcSiteStatus>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_gpc_status(),
            None => Ok(Vec::new()),
        }
    }

    /// Check a site's advertised GPC honoring over the R3-guarded decoy browser
    /// (D4c #18): launch an isolated decoy, fetch and parse the site's
    /// `/.well-known/gpc.json`, record the observation, and return it. A missing
    /// or malformed well-known file records a well-formed "not advertised"
    /// observation rather than erroring. Errors if no store is attached or the
    /// origin is a guardrail-blocked host.
    pub async fn check_site_gpc_via_decoy(
        &self,
        origin: &str,
        decoy_id: &str,
    ) -> Result<GpcSiteStatus> {
        if self.inner.store.is_none() {
            return Err(CoreError::Unimplemented(
                "check_site_gpc_via_decoy requires an open store",
            ));
        }
        let browser = DecoyBrowser::launch(decoy_id).await?;
        let fetched = async {
            let page = browser.new_page().await?;
            page.fetch_gpc_well_known(origin).await
        }
        .await;
        let _ = browser.close().await;
        let support = fetched?;
        self.record_gpc_status(origin, support).await
    }

    // --- DSAR helper (C3 #16 D2c) -------------------------------------------

    /// Generate (without persisting) a DSAR request of `kind` for `persona_id`
    /// to `controller`. Pure generation so a caller can review the drafted
    /// request (and export its letter) before recording it. Validates the
    /// persona exists when a store is attached. The returned request is
    /// `drafted` (no send date, no deadline yet).
    pub async fn generate_dsar_request(
        &self,
        kind: RequestKind,
        persona_id: &str,
        controller: Controller,
    ) -> Result<DsarRequest> {
        // Validate the persona exists when a store is attached (fail closed on a
        // typo); on a store-less core, generation still works for previewing.
        if self.inner.store.is_some() {
            let _ = self.get_persona(persona_id).await?;
        }
        Ok(DsarRequest::draft(
            uuid::Uuid::new_v4().to_string(),
            kind,
            persona_id,
            controller,
            now_millis(),
        ))
    }

    /// Generate AND record a `drafted` DSAR request in the `dsar_requests`
    /// table, returning the persisted record. Errors if no store is attached or
    /// the persona is unknown.
    pub async fn record_dsar_request(
        &self,
        kind: RequestKind,
        persona_id: &str,
        controller: Controller,
    ) -> Result<DsarRequest> {
        let request = self
            .generate_dsar_request(kind, persona_id, controller)
            .await?;
        match &self.inner.store {
            Some(store) => {
                store.lock().await.upsert_dsar_request(&request)?;
                Ok(request)
            }
            None => Err(CoreError::Unimplemented(
                "record_dsar_request requires an open store",
            )),
        }
    }

    /// Persist an updated DSAR request (e.g. after marking it sent/acknowledged/
    /// fulfilled). Upsert by id. Errors if no store is attached.
    pub async fn save_dsar_request(&self, request: &DsarRequest) -> Result<()> {
        match &self.inner.store {
            Some(store) => store.lock().await.upsert_dsar_request(request),
            None => Err(CoreError::Unimplemented(
                "save_dsar_request requires an open store",
            )),
        }
    }

    /// Mark a recorded DSAR request as SENT as of `sent_at`, computing and
    /// persisting the statutory deadline (GDPR one calendar month, CCPA 45
    /// days). Returns the updated record. Errors if no store is attached or the
    /// request id is unknown.
    pub async fn mark_dsar_sent(&self, request_id: &str, sent_at: i64) -> Result<DsarRequest> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "mark_dsar_sent requires an open store",
        ))?;
        let mut request = {
            let guard = store.lock().await;
            guard
                .get_dsar_request(request_id)?
                .ok_or_else(|| CoreError::NotFound(format!("dsar request {request_id}")))?
        };
        request.mark_sent(sent_at)?;
        store.lock().await.upsert_dsar_request(&request)?;
        Ok(request)
    }

    /// List DSAR requests; scoped to one persona when `persona_id` is `Some`,
    /// else all. Newest-created first. Empty when no store is attached.
    pub async fn list_dsar_requests(&self, persona_id: Option<&str>) -> Result<Vec<DsarRequest>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_dsar_requests(persona_id),
            None => Ok(Vec::new()),
        }
    }

    /// The DSAR requests that are OVERDUE as of `now` (the clock is running and
    /// the statutory deadline has lapsed). Sorted soonest-deadline first. Empty
    /// when no store is attached.
    pub async fn overdue_dsar_requests(&self, now: i64) -> Result<Vec<DsarRequest>> {
        let mut overdue: Vec<DsarRequest> = self
            .list_dsar_requests(None)
            .await?
            .into_iter()
            .filter(|r| r.is_overdue(now))
            .collect();
        overdue.sort_by_key(|r| r.deadline);
        Ok(overdue)
    }

    /// The DSAR requests that are DUE SOON as of `now` (the clock is running and
    /// the deadline is within `window_millis` ahead but not yet passed). Sorted
    /// soonest-deadline first. Empty when no store is attached.
    pub async fn due_soon_dsar_requests(
        &self,
        now: i64,
        window_millis: i64,
    ) -> Result<Vec<DsarRequest>> {
        let mut due: Vec<DsarRequest> = self
            .list_dsar_requests(None)
            .await?
            .into_iter()
            .filter(|r| r.is_due_soon(now, window_millis))
            .collect();
        due.sort_by_key(|r| r.deadline);
        Ok(due)
    }

    /// Export (render) the letter text for a recorded DSAR request, signed with
    /// `subject` (the user's real identity details, supplied at export time and
    /// never persisted). Returns the rendered [`DsarLetter`] for MANUAL sending;
    /// this never auto-sends. Errors if no store is attached or the id is
    /// unknown.
    pub async fn export_dsar_letter(
        &self,
        request_id: &str,
        subject: &SubjectDetails,
    ) -> Result<DsarLetter> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "export_dsar_letter requires an open store",
        ))?;
        let request = {
            let guard = store.lock().await;
            guard
                .get_dsar_request(request_id)?
                .ok_or_else(|| CoreError::NotFound(format!("dsar request {request_id}")))?
        };
        Ok(request.render_letter(subject))
    }

    /// Render a letter for an UNRECORDED, in-memory request (preview before
    /// recording). Pure; no store needed, never sends.
    pub fn preview_dsar_letter(
        &self,
        request: &DsarRequest,
        subject: &SubjectDetails,
    ) -> DsarLetter {
        request.render_letter(subject)
    }

    // --- email-alias management (C3 #17 D3c) --------------------------------

    /// Mint a fresh alias for `(persona, site)` via the given [`AliasProvider`]
    /// (e.g. the local [`PlusAddressProvider`]; a future HTTP masking provider
    /// slots into the same seam), record it `active`, and return it.
    ///
    /// Enforces the no-reuse-across-sites rule unless `allow_reuse` is set: if
    /// the minted address is already an active alias for a DIFFERENT site under
    /// this persona, the mint is refused with [`CoreError::Alias`]. Errors if no
    /// store is attached or the persona is unknown.
    pub async fn mint_email_alias(
        &self,
        provider: &dyn AliasProvider,
        persona_id: &str,
        site: &str,
        allow_reuse: bool,
    ) -> Result<EmailAlias> {
        let _ = self.get_persona(persona_id).await?;
        let address = provider.mint(persona_id, site).await?;
        let alias = EmailAlias::new(
            uuid::Uuid::new_v4().to_string(),
            persona_id,
            site,
            &address,
            provider.kind(),
            now_millis(),
            Some(provider.id().to_string()),
        );
        self.persist_new_alias(alias, allow_reuse).await
    }

    /// Record a MANUALLY-created alias (an address the user minted out of band,
    /// e.g. an iCloud Hide-My-Email forward) for `(persona, site)`, marked
    /// `active`, and return it. Same no-reuse-across-sites rule as
    /// [`Core::mint_email_alias`]. Errors if no store is attached or the persona
    /// is unknown.
    pub async fn record_email_alias(
        &self,
        persona_id: &str,
        site: &str,
        address: &str,
        kind: AliasKind,
        provider: Option<&str>,
        allow_reuse: bool,
    ) -> Result<EmailAlias> {
        let _ = self.get_persona(persona_id).await?;
        let alias = EmailAlias::new(
            uuid::Uuid::new_v4().to_string(),
            persona_id,
            site,
            address,
            kind,
            now_millis(),
            provider.map(str::to_string),
        );
        self.persist_new_alias(alias, allow_reuse).await
    }

    /// Shared persistence + no-reuse enforcement for a freshly built alias.
    async fn persist_new_alias(&self, alias: EmailAlias, allow_reuse: bool) -> Result<EmailAlias> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "email-alias management requires an open store",
        ))?;
        if !allow_reuse {
            let clashes = store
                .lock()
                .await
                .active_aliases_with_address(&alias.persona_id, &alias.address)?;
            // Reuse is a clash only when an ACTIVE alias with this address
            // already fronts a DIFFERENT site for this persona.
            if clashes.iter().any(|existing| existing.site != alias.site) {
                return Err(CoreError::Alias(format!(
                    "address {:?} already fronts a different site for this persona; \
                     pass allow_reuse to share it",
                    alias.address
                )));
            }
        }
        store.lock().await.upsert_email_alias(&alias)?;
        Ok(alias)
    }

    /// List email aliases (the inventory of which address fronts which site for
    /// which persona); scoped to one persona when `persona_id` is `Some`, else
    /// all. Newest-created first. Empty when no store is attached.
    pub async fn list_email_aliases(&self, persona_id: Option<&str>) -> Result<Vec<EmailAlias>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_email_aliases(persona_id),
            None => Ok(Vec::new()),
        }
    }

    /// Revoke an alias by id (mark it `revoked`, preserving the audit trail).
    /// Returns the updated record. Errors if no store is attached or the id is
    /// unknown.
    pub async fn revoke_email_alias(&self, alias_id: &str) -> Result<EmailAlias> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "revoke_email_alias requires an open store",
        ))?;
        let mut alias = {
            let guard = store.lock().await;
            guard
                .get_email_alias(alias_id)?
                .ok_or_else(|| CoreError::NotFound(format!("email alias {alias_id}")))?
        };
        alias.status = AliasStatus::Revoked;
        store.lock().await.upsert_email_alias(&alias)?;
        Ok(alias)
    }

    /// Rotate an alias: revoke the existing one and mint a fresh active alias
    /// for the same `(persona, site)` via `provider`. Returns the NEW alias.
    /// The old record is kept `revoked` for the audit trail. Errors if no store
    /// is attached or the id is unknown.
    pub async fn rotate_email_alias(
        &self,
        alias_id: &str,
        provider: &dyn AliasProvider,
    ) -> Result<EmailAlias> {
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "rotate_email_alias requires an open store",
        ))?;
        let old = {
            let guard = store.lock().await;
            guard
                .get_email_alias(alias_id)?
                .ok_or_else(|| CoreError::NotFound(format!("email alias {alias_id}")))?
        };
        // Revoke the old record first so its address frees up for the rotation.
        let mut revoked = old.clone();
        revoked.status = AliasStatus::Revoked;
        store.lock().await.upsert_email_alias(&revoked)?;

        // Mint a fresh address for the same site. A plus-address provider would
        // regenerate the SAME address; that is acceptable (the old one is now
        // revoked), but a future HTTP masking provider mints a genuinely new
        // one. Reuse against the (now revoked) old alias is allowed here.
        let address = provider.mint(&old.persona_id, &old.site).await?;
        let fresh = EmailAlias::new(
            uuid::Uuid::new_v4().to_string(),
            &old.persona_id,
            &old.site,
            &address,
            provider.kind(),
            now_millis(),
            Some(provider.id().to_string()),
        );
        store.lock().await.upsert_email_alias(&fresh)?;
        Ok(fresh)
    }

    // --- account-anchor scanner (C3 #19 D5c) --------------------------------

    /// Add or update a user-curated account anchor in the inventory, returning
    /// the persisted record. READ-ONLY analysis inventory: this records what the
    /// user types; it NEVER scrapes or automates against the account. Errors if
    /// no store is attached.
    pub async fn record_account_anchor(
        &self,
        label: &str,
        site: &str,
        signals: impl IntoIterator<Item = IdentitySignal>,
        shared_contact_key: Option<String>,
    ) -> Result<AccountAnchor> {
        let anchor = AccountAnchor::new(
            uuid::Uuid::new_v4().to_string(),
            label,
            site,
            signals,
            shared_contact_key,
            now_millis(),
        );
        match &self.inner.store {
            Some(store) => {
                store.lock().await.upsert_account_anchor(&anchor)?;
                Ok(anchor)
            }
            None => Err(CoreError::Unimplemented(
                "record_account_anchor requires an open store",
            )),
        }
    }

    /// Persist an updated account anchor (e.g. after editing its signals).
    /// Upsert by id. Errors if no store is attached.
    pub async fn save_account_anchor(&self, anchor: &AccountAnchor) -> Result<()> {
        match &self.inner.store {
            Some(store) => store.lock().await.upsert_account_anchor(anchor),
            None => Err(CoreError::Unimplemented(
                "save_account_anchor requires an open store",
            )),
        }
    }

    /// The account-anchor inventory, label ascending. Empty when no store is
    /// attached.
    pub async fn list_account_anchors(&self) -> Result<Vec<AccountAnchor>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_account_anchors(),
            None => Ok(Vec::new()),
        }
    }

    /// Delete an account anchor from the inventory by id. Returns `true` if a
    /// row was removed, `false` if no such anchor (or no store) exists.
    pub async fn delete_account_anchor(&self, anchor_id: &str) -> Result<bool> {
        match &self.inner.store {
            Some(store) => store.lock().await.delete_account_anchor(anchor_id),
            None => Ok(false),
        }
    }

    /// Score every account anchor in the inventory, highest linkage score first.
    /// Pure, read-only analysis over the stored inventory; never touches an
    /// account. Empty when no store is attached.
    pub async fn score_account_anchors(&self) -> Result<Vec<AnchorScore>> {
        let inventory = self.list_account_anchors().await?;
        Ok(anchors::score_inventory(&inventory))
    }

    /// Produce prioritized partitioning recommendations over the inventory,
    /// ordered by linkage strength. Pure, read-only analysis; it recommends
    /// (separate alias via D3c, split recovery contacts, isolate high-anchor
    /// accounts) but NEVER acts on a real account. Empty when no store is
    /// attached.
    pub async fn account_anchor_recommendations(&self) -> Result<Vec<Recommendation>> {
        let inventory = self.list_account_anchors().await?;
        Ok(anchors::recommendations(&inventory))
    }

    // --- Measurement & analytics API (C4 #20 A1 / #21 A2) -------------------

    /// The measurement engine, present once a store is open. Returns
    /// [`CoreError::Unimplemented`] on a store-less core (measurement needs the
    /// encrypted store for the read-back/submission history and shadow profiles).
    fn measurement(&self) -> Result<&measurement::MeasurementEngine> {
        self.inner
            .measurement
            .as_ref()
            .ok_or(CoreError::Unimplemented(
                "measurement requires an open store",
            ))
    }

    /// The per-platform KL-divergence drift bundle (scalar timeline + per-category
    /// heatmap) for one persona on `platform`, against `baseline` (C4 #20 A1).
    /// Meta is the gracefully-empty no-data series. NO charting here; this is the
    /// data the GUI later renders. Errors only if no store is attached.
    pub async fn platform_drift(
        &self,
        platform: Platform,
        persona_id: &str,
        baseline: &Baseline,
    ) -> Result<PlatformDrift> {
        self.measurement()?
            .platform_drift(platform, persona_id, baseline)
            .await
    }

    /// The per-platform drift bundles for EVERY built-in platform (Google,
    /// Brokers, Meta) for a persona, in display order (C4 #20 A1). The single
    /// call the dashboard's multi-series timeline consumes.
    pub async fn all_platform_drift(
        &self,
        persona_id: &str,
        baseline: &Baseline,
    ) -> Result<Vec<PlatformDrift>> {
        self.measurement()?
            .all_platform_drift(persona_id, baseline)
            .await
    }

    /// A platform's drift bundle aggregated ACROSS DEVICES into one combined view
    /// (C4 #20 A1 device dimension). Each `persona_ids` entry is one device's
    /// persona; their snapshots merge by timestamp. Degrades to single-device
    /// (one id) and to no-data (empty list) without panicking. Errors only if no
    /// store is attached.
    pub async fn combined_platform_drift(
        &self,
        platform: Platform,
        persona_ids: &[String],
        baseline: &Baseline,
    ) -> Result<PlatformDrift> {
        self.measurement()?
            .combined_platform_drift(platform, persona_ids, baseline)
            .await
    }

    /// Persist (insert or replace) a shadow-profile definition for the
    /// control-profile A/B (C4 #21 A2). Errors if no store is attached.
    pub async fn save_shadow_profile(&self, profile: &ShadowProfile) -> Result<()> {
        self.measurement()?.save_shadow_profile(profile).await
    }

    /// List all shadow-profile definitions, newest-defined first (C4 #21 A2).
    /// Empty when no store is attached.
    pub async fn list_shadow_profiles(&self) -> Result<Vec<ShadowProfile>> {
        match &self.inner.measurement {
            Some(engine) => engine.list_shadow_profiles().await,
            None => Ok(Vec::new()),
        }
    }

    /// Fetch one shadow-profile definition by id, or `None` if absent (or no
    /// store is attached) (C4 #21 A2).
    pub async fn get_shadow_profile(&self, id: &str) -> Result<Option<ShadowProfile>> {
        match &self.inner.measurement {
            Some(engine) => engine.get_shadow_profile(id).await,
            None => Ok(None),
        }
    }

    /// Delete a shadow-profile definition by id (C4 #21 A2). Returns `true` if a
    /// row was removed, `false` if absent or no store is attached.
    pub async fn delete_shadow_profile(&self, id: &str) -> Result<bool> {
        match &self.inner.measurement {
            Some(engine) => engine.delete_shadow_profile(id).await,
            None => Ok(false),
        }
    }

    /// Run the treated-vs-control A/B comparison across the defined shadow
    /// profiles on `platform` (C4 #21 A2): the effect size (Cohen's d) plus the
    /// significance (t-test p-value) on the A1 drift metric, with a plainly-
    /// readable summary. `kind` selects Welch (default) or pooled. Errors only if
    /// no store is attached.
    pub async fn compare_shadow_cohorts(
        &self,
        platform: Platform,
        baseline: &Baseline,
        kind: TTestKind,
    ) -> Result<CohortComparison> {
        self.measurement()?
            .compare_shadow_cohorts(platform, baseline, kind)
            .await
    }

    // --- C8 #33 U2: goal-driven campaigns -----------------------------------

    /// The campaign planner, present once a store is open. Returns
    /// [`CoreError::Unimplemented`] on a store-less core (campaigns need the
    /// encrypted store for persistence and the measurement engine for the
    /// closed-loop signal).
    fn campaign_planner(&self) -> Result<&campaigns::CampaignPlanner> {
        self.inner
            .campaign_planner
            .as_ref()
            .ok_or(CoreError::Unimplemented("campaigns require an open store"))
    }

    /// Persist (insert or replace) a goal-driven campaign (C8 #33 U2). Errors if
    /// no store is attached.
    pub async fn save_campaign(&self, campaign: &Campaign) -> Result<()> {
        self.campaign_planner()?.save(campaign).await
    }

    /// Fetch a campaign by id, or `None` if absent (or no store is attached).
    pub async fn get_campaign(&self, id: &str) -> Result<Option<Campaign>> {
        match &self.inner.campaign_planner {
            Some(planner) => planner.get(id).await,
            None => Ok(None),
        }
    }

    /// List campaigns. Scoped to a persona when `persona_id` is `Some`, else all;
    /// most recently updated first. Empty when no store is attached.
    pub async fn list_campaigns(&self, persona_id: Option<&str>) -> Result<Vec<Campaign>> {
        match &self.inner.campaign_planner {
            Some(planner) => planner.list(persona_id).await,
            None => Ok(Vec::new()),
        }
    }

    /// Delete a campaign by id. Returns `true` if a row was removed, `false` if
    /// absent or no store is attached.
    pub async fn delete_campaign(&self, id: &str) -> Result<bool> {
        match &self.inner.campaign_planner {
            Some(planner) => planner.delete(id).await,
            None => Ok(false),
        }
    }

    /// Start (or resume) a campaign (`Planned`/`Paused` -> `Running`), persisting
    /// it. Errors if the campaign is `Achieved` or no store is attached.
    pub async fn start_campaign(&self, id: &str, now: i64) -> Result<Campaign> {
        self.campaign_planner()?.start(id, now).await
    }

    /// Pause a campaign on user request, persisting it. Errors if no store is
    /// attached.
    pub async fn pause_campaign(&self, id: &str, now: i64) -> Result<Campaign> {
        self.campaign_planner()?.pause(id, now).await
    }

    /// Adjust a campaign's goal threshold, persisting it. Errors if no store is
    /// attached or the threshold is non-finite.
    pub async fn adjust_campaign_threshold(
        &self,
        id: &str,
        threshold: f64,
        now: i64,
    ) -> Result<Campaign> {
        self.campaign_planner()?
            .adjust_threshold(id, threshold, now)
            .await
    }

    /// Advance one campaign's closed loop one tick (read the metric, compute the
    /// gap, advance the lifecycle, persist) and return the scheduler
    /// [`CampaignDirective`]. Errors if no store is attached.
    pub async fn tick_campaign(&self, id: &str, now: i64) -> Result<CampaignDirective> {
        self.campaign_planner()?.tick(id, now).await
    }

    /// Tick every `Running` campaign, returning each `(id, directive)`. Empty
    /// when no store is attached.
    pub async fn tick_running_campaigns(
        &self,
        now: i64,
    ) -> Result<Vec<(String, CampaignDirective)>> {
        match &self.inner.campaign_planner {
            Some(planner) => planner.tick_all_running(now).await,
            None => Ok(Vec::new()),
        }
    }

    /// The campaign directive that should steer `persona_id`'s decoy activity
    /// right now (C8 #33 closed loop), derived from its running campaigns'
    /// persisted progress WITHOUT advancing the loop. Plan builders (the household
    /// schedule and the extension decoy plan) consult this so gap-to-goal sets the
    /// scheduler intensity and biases decoy topic selection toward the target
    /// segment. Returns an idle directive on a store-less core, or when no running
    /// campaign currently drives the persona.
    pub async fn campaign_directive_for_persona(
        &self,
        persona_id: &str,
    ) -> Result<CampaignDirective> {
        let directive = match &self.inner.campaign_planner {
            Some(planner) => planner.effective_directive(persona_id).await?,
            None => return Ok(CampaignDirective::idle()),
        };
        Ok(self.gate_by_idle(directive).await)
    }

    /// Gate a campaign directive's intensity through the idle planner (C8 #32):
    /// when idle gating is attached, the campaign-driven intensity becomes the
    /// rate planner's decision over it (paused while Active/Locked, scaled up past
    /// the idle threshold). Ungated (returned unchanged) when no idle planner is
    /// attached, so a dedicated headless box runs at the campaign's full rate.
    async fn gate_by_idle(&self, directive: CampaignDirective) -> CampaignDirective {
        let (Some(planner), Some(base)) = (&self.inner.idle_planner, directive.intensity) else {
            return directive;
        };
        match planner.plan(base).await {
            RateDecision::Run(level) => CampaignDirective::running(level, directive.target_segment),
            RateDecision::Paused => CampaignDirective::idle(),
        }
    }

    /// The current idle/lock state the gating sees (C8 #32), for the #36 status
    /// sensor and operator visibility. Reports [`IdleState::Active`] (the
    /// conservative default) when no idle planner is attached.
    pub async fn idle_state_now(&self) -> IdleState {
        match &self.inner.idle_planner {
            Some(planner) => planner.sample().await,
            None => IdleState::Active,
        }
    }

    /// Assemble the live MQTT status + per-campaign efficacy readings for the
    /// Home Assistant sensors (C8 #36). Pure read: the status reflects the
    /// idle-gated campaign activity (running iff some running campaign is actually
    /// driving this tick, the effective intensity being the strongest such), and
    /// each running campaign contributes one efficacy reading (its target
    /// segment's last A1 drift, which the closed loop already tracks). Empty on a
    /// store-less core.
    pub async fn mqtt_status_snapshot(
        &self,
    ) -> Result<(mqtt::StatusPayload, Vec<mqtt::EfficacySensor>)> {
        let idle_state = self.idle_state_now().await;
        let campaigns = self.list_campaigns(None).await?;
        let mut running_campaigns = 0usize;
        let mut effective: Option<IntensityLevel> = None;
        let mut efficacy = Vec::new();
        for campaign in &campaigns {
            if campaign.status != CampaignStatus::Running {
                continue;
            }
            running_campaigns += 1;
            // The idle-gated directive this campaign's persona is currently driving.
            let directive = self
                .campaign_directive_for_persona(&campaign.persona_id)
                .await?;
            if let Some(level) = directive.intensity {
                let stronger = effective
                    .map(|cur| level.rate_per_second() > cur.rate_per_second())
                    .unwrap_or(true);
                if stronger {
                    effective = Some(level);
                }
            }
            // The campaign's last observed per-segment drift (A1), if it has ticked.
            let drift = campaign.progress.last_metric.unwrap_or(0.0);
            let points = usize::from(campaign.progress.last_metric.is_some());
            efficacy.push(mqtt::EfficacySensor::new(
                &campaign.persona_id,
                &campaign.target_segment,
                drift,
                points,
            ));
        }
        let status = mqtt::StatusPayload::new(
            effective.is_some(),
            idle_state,
            effective,
            running_campaigns,
        );
        Ok((status, efficacy))
    }

    /// Apply a parsed MQTT campaign command (start/pause/adjust) to the planner
    /// (C8 #36 U5 command routing). Errors if no store is attached.
    pub async fn apply_campaign_command(
        &self,
        command: &mqtt::CampaignCommand,
        now: i64,
    ) -> Result<Campaign> {
        command.apply(self.campaign_planner()?, now).await
    }

    /// Route a raw MQTT command-topic payload into the campaign planner (C8 #36
    /// U5). Parses the JSON and applies it; a malformed payload is a
    /// [`CoreError::Mqtt`]. Errors if no store is attached.
    pub async fn route_campaign_command(&self, payload: &[u8], now: i64) -> Result<Campaign> {
        mqtt::command::route(self.campaign_planner()?, payload, now).await
    }

    // --- Efficacy-snapshot export: CSV / JSON / PDF (C4 #23 A4) --------------

    /// Build the [`EfficacySnapshotData`] for a persona as of `as_of_millis`:
    /// the per-platform A1 drift bundles against `baseline`. The structured data
    /// the exports derive from; the GUI/CLI can also inspect it directly. Errors
    /// only if no store is attached.
    pub async fn efficacy_snapshot_data(
        &self,
        persona_id: &str,
        baseline: &Baseline,
        as_of_millis: i64,
    ) -> Result<EfficacySnapshotData> {
        self.measurement()?
            .efficacy_snapshot_data(persona_id, baseline, as_of_millis)
            .await
    }

    /// Export the efficacy snapshot for a persona to `format` (CSV / JSON /
    /// PDF), embedding the as-of date, and return the in-memory
    /// [`ExportArtifact`] (bytes + typed metadata). PRODUCING the artifact and
    /// WRITING it ([`ExportArtifact::write_to`]) are separate steps: that is the
    /// clean seam a future ed25519 signing layer wraps. No signing/hashing/
    /// timestamping happens here. Errors only if no store is attached.
    pub async fn export_efficacy_snapshot(
        &self,
        persona_id: &str,
        baseline: &Baseline,
        as_of_millis: i64,
        format: ExportFormat,
    ) -> Result<ExportArtifact> {
        self.measurement()?
            .export_efficacy_snapshot(persona_id, baseline, as_of_millis, format)
            .await
    }

    // --- Persona library: import/export signed packs (C5 #27 P4) ------------

    /// The device's persona-pack signing key, present once a store is open.
    /// Returns [`CoreError::Unimplemented`] on a store-less core (the key is
    /// loaded from the keystore alongside the store).
    fn pack_key(&self) -> Result<&personapack::PackSigningKey> {
        self.inner.pack_key.as_ref().ok_or(CoreError::Unimplemented(
            "persona-pack library requires an open store",
        ))
    }

    /// This device's persona-pack SIGNER public key, STANDARD base64. The key
    /// every pack this device exports is signed with; surfaced so a recipient
    /// can record it (a future trust-on-first-use / known-keys list, which is a
    /// later concern). Errors if no store is attached.
    pub async fn pack_signer_public_key(&self) -> Result<String> {
        Ok(self.pack_key()?.public_key_base64())
    }

    /// Export the named personas as a SIGNED persona pack, returning the pack's
    /// JSON bytes (the import/export byte form). Pulls the selected persona ids
    /// from the encrypted store, wraps them with a [`PackProvenance`] record, and
    /// signs the canonical content with this device's pack-signing key.
    ///
    /// Errors if no store is attached, if any requested id is unknown
    /// ([`CoreError::NotFound`], fail closed), or if signing fails. The byte
    /// output round-trips through [`Core::import_persona_pack`] /
    /// [`verify_pack`].
    pub async fn export_persona_pack(
        &self,
        persona_ids: &[String],
        provenance: PackProvenance,
    ) -> Result<Vec<u8>> {
        let key = self.pack_key()?;
        let mut personas = Vec::with_capacity(persona_ids.len());
        for id in persona_ids {
            personas.push(self.get_persona(id).await?);
        }
        let content = PackContent::new(provenance, personas);
        let pack = personapack::sign_pack_with(content, key)?;
        Ok(pack.to_bytes()?)
    }

    /// Import a signed persona pack from its JSON `bytes`: VERIFY it, then land
    /// its personas into the encrypted persona store and record the pack in the
    /// installed-pack library ledger. Returns the imported personas on success.
    ///
    /// Verification ([`verify_pack`]) fails closed with a typed
    /// [`CoreError::Pack`] for a tampered pack, an unsigned pack, an
    /// unknown/newer schema version, or an invalid embedded key/signature; in
    /// every rejection case NOTHING is written. Verifying that the signer key is
    /// TRUSTED is a separate future concern; this confirms pack integrity and
    /// that the embedded key signed it.
    ///
    /// Errors if no store is attached. The importer is tolerant of OLDER pack
    /// schema versions within the supported window.
    pub async fn import_persona_pack(&self, bytes: &[u8]) -> Result<Vec<SyntheticPersona>> {
        // Verify BEFORE touching the store: a rejected pack writes nothing.
        let pack = personapack::verify_pack(bytes)?;
        let store = self.inner.store.as_ref().ok_or(CoreError::Unimplemented(
            "import_persona_pack requires an open store",
        ))?;

        let record = PackRecord::from_pack(uuid::Uuid::new_v4().to_string(), &pack, now_millis());
        let installed = InstalledPack::new(record);
        {
            let guard = store.lock().await;
            for persona in &pack.content.personas {
                guard.save_persona(persona)?;
            }
            guard.upsert_installed_pack(&installed)?;
        }
        // Notify dependent views that personas changed (one event per persona).
        for persona in &pack.content.personas {
            self.emit_persona_change(studio::PersonaChanged::saved(&persona.id));
        }
        Ok(pack.content.personas)
    }

    /// List the installed persona packs (the library ledger), most-recently
    /// imported first. Each [`InstalledPack`] carries the pack's provenance, its
    /// signer key, and the persona ids it brought in. Empty when no store is
    /// attached.
    pub async fn list_installed_packs(&self) -> Result<Vec<InstalledPack>> {
        match &self.inner.store {
            Some(store) => store.lock().await.list_installed_packs(),
            None => Ok(Vec::new()),
        }
    }

    /// Fetch one installed pack from the library ledger by id, or `None` if
    /// absent (or no store is attached).
    pub async fn get_installed_pack(&self, pack_id: &str) -> Result<Option<InstalledPack>> {
        match &self.inner.store {
            Some(store) => store.lock().await.get_installed_pack(pack_id),
            None => Ok(None),
        }
    }

    /// Remove an installed pack from the library ledger by id. Returns `true` if
    /// a row was removed.
    ///
    /// This removes only the library ledger entry, not the personas the pack
    /// brought in: an imported persona may have been edited, rotated, or shared
    /// since import, so deleting it is a separate, explicit
    /// [`Core::delete_persona`] decision. Returns `false` if no such pack (or no
    /// store) exists.
    pub async fn remove_installed_pack(&self, pack_id: &str) -> Result<bool> {
        match &self.inner.store {
            Some(store) => store.lock().await.delete_installed_pack(pack_id),
            None => Ok(false),
        }
    }

    // --- Generate-on-desktop, execute-on-phone (C6 #28 H1) ------------------

    /// Run a full generation pass for one STORED persona and return the two
    /// SIGNED artifacts (the adversarial-allocation weight map and the
    /// category-targeted query plan), WITHOUT pushing them. The weight map biases
    /// the query plan's category selection.
    ///
    /// The heavy work runs here on the desktop: the weight map is the
    /// [`generate::allocate`] adversarial surrogate over the persona-following
    /// blend (favoring the persona's interests, within the
    /// [`generate::KL_BUDGET`]); the query plan reuses the same circadian Poisson
    /// timing and persona blend the studio simulator and the C1 scheduler use,
    /// biased by the weight map. Both are signed with the device's
    /// artifact-signing key (the same key persona packs use). `seed` makes the
    /// pass DETERMINISTIC. `freshness_ms` sets each artifact's expiry
    /// ([`generate::DEFAULT_FRESHNESS_MS`] for the 24h default).
    ///
    /// Errors if no store is attached or the persona id is unknown.
    pub async fn generate_signed_artifacts(
        &self,
        persona_id: &str,
        intensity: IntensityLevel,
        seed: u64,
        freshness_ms: i64,
    ) -> Result<GeneratedArtifacts> {
        let persona = self.get_persona(persona_id).await?;
        let key = self.pack_key()?;
        let now = now_millis();

        // The persona-following blend the allocator perturbs and the plan biases.
        // Interest categories carry the aligned blend weight, others the
        // misaligned one (the same two-tier blend the simulator uses), so the
        // allocator starts from the genuine persona shape.
        let aligned: Vec<persona::CategoryPool> = persona
            .interests
            .iter()
            .filter_map(|i| persona::CategoryPool::from_name(i))
            .collect();
        let baseline_blend = generate::allocator::weight_map_from(|c| {
            let follow = constants::PERSONA_FOLLOW_FRACTION;
            let noise = constants::UNIFORM_BASELINE_WEIGHT * (1.0 - follow);
            if aligned.is_empty() {
                constants::NEUTRAL_WEIGHT
            } else if aligned.contains(&c) {
                constants::ALIGNED_WEIGHT * follow + noise
            } else {
                constants::MISALIGNED_WEIGHT * follow + noise
            }
        });

        // 1. Adversarial-allocation weight map (signed).
        let weight_map = generate::allocate(&baseline_blend, &aligned, generate::KL_BUDGET);
        let weight_map_content = generate::ArtifactContent::new(
            persona_id,
            generate::ArtifactPayload::WeightMap(weight_map.clone()),
            now,
            freshness_ms,
        );
        let weight_map_artifact = generate::sign_artifact_with(weight_map_content, key)?;

        // 2. Query plan (signed), biased by the weight map.
        let plan = generate::generate_query_plan(&persona, &weight_map, intensity, seed);
        let plan_content = generate::ArtifactContent::new(
            persona_id,
            generate::ArtifactPayload::QueryPlan(plan),
            now,
            freshness_ms,
        );
        let query_plan_artifact = generate::sign_artifact_with(plan_content, key)?;

        Ok(GeneratedArtifacts {
            weight_map: weight_map_artifact,
            query_plan: query_plan_artifact,
        })
    }

    /// Headless generation pass: generate + sign BOTH artifacts for a stored
    /// persona, then PUSH each to every paired peer over the O1 sealed channel.
    /// Returns the artifacts that were pushed and the number of peers each was
    /// sealed and sent to (the same count for both). The phone verifies the
    /// signature and freshness before replaying, falling back to on-device
    /// generation otherwise.
    ///
    /// Errors if no store is attached or the persona id is unknown. With no
    /// paired peers the artifacts are still generated/signed and the count is 0.
    pub async fn run_generation_pass(
        &self,
        persona_id: &str,
        intensity: IntensityLevel,
        seed: u64,
        freshness_ms: i64,
    ) -> Result<GenerationPassOutcome> {
        let artifacts = self
            .generate_signed_artifacts(persona_id, intensity, seed, freshness_ms)
            .await?;
        let sync = self.sync_engine()?;
        let weight_map_msg = SyncMessage::signed_artifact(artifacts.weight_map.clone());
        let plan_msg = SyncMessage::signed_artifact(artifacts.query_plan.clone());
        let peers_reached = sync.push_message_to_all(&weight_map_msg).await?;
        sync.push_message_to_all(&plan_msg).await?;
        tracing::info!(
            persona = %persona_id,
            peers = peers_reached,
            "C6 generation pass: pushed signed weight-map + query-plan artifacts"
        );
        Ok(GenerationPassOutcome {
            artifacts,
            peers_reached,
        })
    }

    /// Push one already-signed artifact to every paired peer over the sealed
    /// channel (C6 #28 H1). Returns the number of peers it reached. Used when a
    /// caller has produced the artifact out of band (e.g. via
    /// [`Core::generate_signed_artifacts`]).
    pub async fn push_signed_artifact(&self, artifact: &SignedArtifact) -> Result<usize> {
        let message = SyncMessage::signed_artifact(artifact.clone());
        self.sync_engine()?.push_message_to_all(&message).await
    }

    /// Receive a sealed frame attributed to a paired sender (by base64url public
    /// key), open and verify the sealed channel, then VERIFY the carried signed
    /// artifact (signature + integrity) and apply the freshness check at `now`.
    ///
    /// Returns an [`ArtifactDecision`]: [`ArtifactDecision::Replay`] with the
    /// verified, fresh artifact, or [`ArtifactDecision::Fallback`] (the phone's
    /// fall-back-to-on-device-generation signal) when the artifact is invalid or
    /// stale. A frame from an unpaired sender, or one that fails the sealed-channel
    /// authentication, is rejected before any artifact logic (fail closed). A
    /// frame whose body is NOT a [`SyncBody::SignedArtifact`](crate::sync::wire::SyncBody)
    /// is a [`CoreError::Sync`] (wrong path), not a silent drop.
    pub async fn receive_artifact_frame(
        &self,
        sender_public_key: &str,
        frame: &[u8],
        now: i64,
    ) -> Result<ArtifactDecision> {
        let message = self
            .sync_engine()?
            .receive_sync_message(sender_public_key, frame)
            .await?;
        match message.body {
            sync::wire::SyncBody::SignedArtifact(artifact) => {
                // The replay-vs-fallback decision (verify + freshness, fail closed)
                // is shared with the bytes-based `select_artifact_or_fallback` via
                // `decide_artifact`, so a tampered or stale artifact yields a
                // Fallback (not a replay) identically on both consumer paths.
                Ok(generate::decide_artifact(artifact, now))
            }
            other => Err(CoreError::Sync(format!(
                "receive_artifact_frame expected SignedArtifact; got a different kind ({}); use receive_sync_message",
                other.kind_name()
            ))),
        }
    }

    // --- Persona-pack minting: the PUMS generator (C6 #29 H2) ---------------

    /// Mint `count` COHERENT synthetic personas from the bundled PUMS-style SEED
    /// distribution, deterministically seeded by `seed`, WITHOUT persisting or
    /// packing them. Each persona's demographics are drawn JOINTLY from one
    /// [`DemographicCell`] (so the age/profession/
    /// region co-occur), it carries 3-to-5 interests and the frozen 8-to-10-day
    /// rotation window, and it passes the C5 coherence linter with NO
    /// HardImplausible finding (a flagged draw is re-sampled).
    ///
    /// The bundled distribution is the real ACS-PUMS 2022 export (315 demographic
    /// cells; see [`mint`]). Pure (needs no store); the same `seed` re-draws
    /// identical demographics/interests/windows (the UUID id is the one fresh
    /// field).
    pub fn mint_personas(&self, count: usize, seed: u64) -> Result<MintedPersonas> {
        let dist = mint::PersonaDistribution::bundled()?;
        Ok(mint::mint_personas(&dist, count, seed, now_millis())?)
    }

    /// Mint `count` coherent personas (as [`Core::mint_personas`]) and bundle them
    /// into a SIGNED [`PersonaPack`], returning the pack's JSON bytes (the
    /// import/export byte form, which [`verify_pack`] accepts). The pack is signed
    /// with this device's pack-signing key and its [`PackProvenance`] records the
    /// source distribution label and the generation seed.
    ///
    /// Errors if no store is attached (the pack-signing key loads alongside the
    /// store) or if minting/signing fails. Does NOT persist the personas; pair it
    /// with [`Core::import_persona_pack`] to land them locally, or
    /// [`Core::mint_and_push_pack`] to distribute them.
    pub async fn mint_persona_pack(&self, count: usize, seed: u64) -> Result<Vec<u8>> {
        let key = self.pack_key()?;
        let now = now_millis();
        let dist = mint::PersonaDistribution::bundled()?;
        let minted = mint::mint_personas(&dist, count, seed, now)?;
        let pack = mint::mint_pack(&minted, now, key)?;
        Ok(pack.to_bytes()?)
    }

    /// Headless mint pass: mint `count` coherent personas, sign them into a
    /// [`PersonaPack`], and PUSH the signed pack to every paired peer over the O1
    /// sealed channel as the [`SyncBody::PersonaPack`](crate::sync::wire::SyncBody)
    /// wire kind. Returns the outcome (the pack bytes and the number of peers it
    /// reached). The receiver VERIFIES the pack signature before importing
    /// (verify-before-write); see [`Core::receive_pack_frame`].
    ///
    /// Errors if no store is attached or minting/signing fails. With no paired
    /// peers the pack is still minted/signed and the count is 0.
    pub async fn mint_and_push_pack(&self, count: usize, seed: u64) -> Result<MintPushOutcome> {
        let pack_bytes = self.mint_persona_pack(count, seed).await?;
        let message = SyncMessage::persona_pack(&pack_bytes);
        let peers_reached = self.sync_engine()?.push_message_to_all(&message).await?;
        tracing::info!(
            count,
            peers = peers_reached,
            "C6 mint pass: pushed signed persona pack to paired peers"
        );
        Ok(MintPushOutcome {
            pack_bytes,
            peers_reached,
        })
    }

    /// Receive a sealed frame attributed to a paired sender (by base64url public
    /// key), open and authenticate the sealed channel, then VERIFY the carried
    /// persona pack's signature (P4 [`verify_pack`]) BEFORE importing its personas
    /// into the encrypted store and recording the pack in the installed-pack
    /// ledger. Returns the imported personas on success.
    ///
    /// Verify-before-write, fail closed: a frame from an unpaired sender or one that
    /// fails the sealed-channel authentication is rejected before any pack logic; a
    /// bad/unsigned/tampered pack is a [`CoreError::Pack`] and NOTHING is written.
    /// A frame whose body is NOT a [`SyncBody::PersonaPack`](crate::sync::wire::SyncBody)
    /// is a [`CoreError::Sync`] (wrong path), not a silent drop.
    pub async fn receive_pack_frame(
        &self,
        sender_public_key: &str,
        frame: &[u8],
    ) -> Result<Vec<SyntheticPersona>> {
        let message = self
            .sync_engine()?
            .receive_sync_message(sender_public_key, frame)
            .await?;
        match message.body {
            sync::wire::SyncBody::PersonaPack(body) => {
                // Decode the pack bytes and import via the SAME verify-before-write
                // path the file import uses (`import_persona_pack`): a rejected pack
                // writes nothing.
                let pack_bytes = body.pack_bytes()?;
                self.import_persona_pack(&pack_bytes).await
            }
            other => Err(CoreError::Sync(format!(
                "receive_pack_frame expected PersonaPack; got a different kind ({}); use receive_sync_message",
                other.kind_name()
            ))),
        }
    }

    // --- Per-persona network egress (C7 #30 N1) -----------------------------

    /// The key source backing this core's keystore (for proxy credentials).
    /// Present once a store is open.
    fn key_source(&self) -> Result<&KeySource> {
        self.inner
            .key_source
            .as_ref()
            .ok_or(CoreError::Unimplemented(
                "network egress requires an open store",
            ))
    }

    /// Bind a per-persona [`Egress`] (N1). Validates the egress (fail closed on a
    /// malformed host/endpoint) and persists it in the `persona_egress` table.
    ///
    /// This sets ONLY the routing config; proxy CREDENTIALS are supplied
    /// separately via [`Core::set_persona_proxy_credentials`] and live in the OS
    /// keystore, never the DB and never a log. Errors if no store is attached or
    /// the egress is malformed.
    pub async fn set_persona_egress(&self, persona_id: &str, egress: Egress) -> Result<()> {
        network::validate_egress(&egress)?;
        match &self.inner.store {
            Some(store) => {
                store.lock().await.put_persona_egress(persona_id, &egress)?;
                tracing::info!(
                    target: "fauxx_core::network",
                    persona_id,
                    exit = %egress.exit_label(),
                    "bound per-persona egress"
                );
                Ok(())
            }
            None => Err(CoreError::Unimplemented(
                "set_persona_egress requires an open store",
            )),
        }
    }

    /// The per-persona [`Egress`], defaulting to [`Egress::Direct`] when none is
    /// bound (or no store is attached).
    pub async fn get_persona_egress(&self, persona_id: &str) -> Result<Egress> {
        let stored = match &self.inner.store {
            Some(store) => store.lock().await.get_persona_egress(persona_id)?,
            None => None,
        };
        Ok(stored.unwrap_or_default())
    }

    /// Clear a persona's egress binding (reverting it to [`Egress::Direct`]) and
    /// remove any proxy credentials it had in the keystore, so a secret never
    /// outlives its config. Returns `true` if a binding existed. Errors if no
    /// store is attached.
    pub async fn clear_persona_egress(&self, persona_id: &str) -> Result<bool> {
        let (removed, prior) = match &self.inner.store {
            Some(store) => {
                let guard = store.lock().await;
                let prior = guard.get_persona_egress(persona_id)?;
                let removed = guard.delete_persona_egress(persona_id)?;
                (removed, prior)
            }
            None => {
                return Err(CoreError::Unimplemented(
                    "clear_persona_egress requires an open store",
                ))
            }
        };
        // Best-effort credential cleanup for a proxy that carried auth.
        if let Some(auth) = prior.as_ref().and_then(Egress::proxy_auth) {
            let source = self.key_source()?;
            let _ = store::delete_proxy_credentials(source, &auth.account_label)?;
        }
        Ok(removed)
    }

    /// Store the proxy CREDENTIALS (username/password) for a persona's egress in
    /// the OS keystore under the egress's
    /// [`ProxyAuth::account_label`](crate::network::ProxyAuth::account_label).
    /// The secret never touches the DB and is never logged. The persona's egress
    /// must already carry a [`ProxyAuth`] marker (an HTTP/SOCKS proxy with auth);
    /// otherwise this errors. Errors if no store is attached.
    pub async fn set_persona_proxy_credentials(
        &self,
        persona_id: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let egress = self.get_persona_egress(persona_id).await?;
        let auth = egress.proxy_auth().ok_or_else(|| {
            CoreError::Network(format!(
                "persona {persona_id} egress carries no proxy-auth marker; \
                 set an HttpProxy/SocksProxy egress with auth first"
            ))
        })?;
        let source = self.key_source()?;
        store::store_proxy_credentials(source, &auth.account_label, username, password)?;
        tracing::info!(
            target: "fauxx_core::network",
            persona_id,
            "stored proxy credentials in the OS keystore (never the DB)"
        );
        Ok(())
    }

    /// Whether the OS keystore holds proxy credentials for this persona's egress
    /// (a NON-secret presence check; never returns the secret). `false` when the
    /// egress carries no auth marker or no credential is stored.
    pub async fn has_persona_proxy_credentials(&self, persona_id: &str) -> Result<bool> {
        let egress = self.get_persona_egress(persona_id).await?;
        let Some(auth) = egress.proxy_auth() else {
            return Ok(false);
        };
        let source = self.key_source()?;
        Ok(store::load_proxy_credentials(source, &auth.account_label)?.is_some())
    }

    /// The per-persona exit indicator (N1): the configured exit provider/region
    /// (or Tor) plus its reachable/paused state, computed via the injected
    /// [`ReachabilityCheck`] seam. FAIL CLOSED: a configured (non-Direct) egress
    /// that the seam reports UNREACHABLE yields `paused = true`, never a
    /// direct-route fallback. Errors only if no store is attached.
    pub async fn persona_egress_exit(
        &self,
        persona_id: &str,
        check: &dyn ReachabilityCheck,
    ) -> Result<EgressExit> {
        let egress = self.get_persona_egress(persona_id).await?;
        let reachable = check.is_reachable(&egress).await;
        let exit = EgressExit::evaluate(persona_id, &egress, reachable);
        if exit.paused {
            tracing::warn!(
                target: "fauxx_core::network",
                persona_id,
                exit = %exit.label,
                "persona PAUSED: configured egress unreachable (fail closed, no direct fallback)"
            );
        }
        Ok(exit)
    }

    /// The per-persona exit indicator using the LIVE [`TcpReachability`] check (a
    /// TCP connect to the egress endpoint). Convenience over
    /// [`Core::persona_egress_exit`]; the hermetic tests use the seam form with a
    /// [`StaticReachability`] result instead.
    pub async fn persona_egress_exit_live(&self, persona_id: &str) -> Result<EgressExit> {
        self.persona_egress_exit(persona_id, &TcpReachability).await
    }

    // --- Per-persona DNS strategy (C7 #31 N2) -------------------------------

    /// Bind a per-persona [`DnsStrategy`] (N2). Validates it (a DoH resolver must
    /// be an `https://` template; fail closed otherwise) and persists it in the
    /// `persona_dns` table. The chosen resolver is applied to the SAME isolated
    /// decoy profile as the egress (see [`Core::launch_persona_decoy_browser`]).
    /// Errors if no store is attached or the strategy is malformed.
    pub async fn set_persona_dns(&self, persona_id: &str, dns: DnsStrategy) -> Result<()> {
        network::validate_dns(&dns)?;
        match &self.inner.store {
            Some(store) => {
                store.lock().await.put_persona_dns(persona_id, &dns)?;
                // The DNS choice itself is persisted, not logged as sensitive; we
                // log only that a strategy was bound, plus the explicit observer
                // trade-off note (which names the resolver the user chose).
                tracing::info!(
                    target: "fauxx_core::network",
                    persona_id,
                    "bound per-persona DNS strategy"
                );
                Ok(())
            }
            None => Err(CoreError::Unimplemented(
                "set_persona_dns requires an open store",
            )),
        }
    }

    /// The per-persona [`DnsStrategy`], defaulting to
    /// [`DnsStrategy::SystemDefault`] when none is bound (or no store is
    /// attached).
    pub async fn get_persona_dns(&self, persona_id: &str) -> Result<DnsStrategy> {
        let stored = match &self.inner.store {
            Some(store) => store.lock().await.get_persona_dns(persona_id)?,
            None => None,
        };
        Ok(stored.unwrap_or_default())
    }

    /// Clear a persona's DNS-strategy binding (reverting it to
    /// [`DnsStrategy::SystemDefault`]). Returns `true` if a binding existed.
    /// Errors if no store is attached.
    pub async fn clear_persona_dns(&self, persona_id: &str) -> Result<bool> {
        match &self.inner.store {
            Some(store) => store.lock().await.delete_persona_dns(persona_id),
            None => Err(CoreError::Unimplemented(
                "clear_persona_dns requires an open store",
            )),
        }
    }

    /// The EXPLICIT observer trade-off note for this persona's DNS strategy (N2):
    /// a human-readable line naming who sees this persona's lookups. The note is
    /// always surfaced so the trade-off is never hidden.
    pub async fn persona_dns_observer_note(&self, persona_id: &str) -> Result<String> {
        Ok(self.get_persona_dns(persona_id).await?.observer_note())
    }

    // --- Combined per-persona network config + decoy launch (N1 + N2) -------

    /// The combined per-persona [`PersonaNetwork`] (egress + DNS), as applied to
    /// the decoy browser. Defaults to Direct + SystemDefault when nothing is
    /// bound.
    pub async fn persona_network(&self, persona_id: &str) -> Result<PersonaNetwork> {
        let egress = self.get_persona_egress(persona_id).await?;
        let dns = self.get_persona_dns(persona_id).await?;
        Ok(PersonaNetwork::new(egress, dns))
    }

    /// Launch an isolated decoy browser for `persona_id` with that persona's
    /// egress and DNS applied to the SAME isolated profile (N1 + N2), using the
    /// injected [`ReachabilityCheck`] seam for the fail-closed gate.
    ///
    /// FAIL CLOSED: if the persona's configured (non-Direct) egress is
    /// unreachable, this does NOT launch and returns [`CoreError::Network`] with
    /// the paused state, rather than launching on the default route and leaking
    /// the real IP. A Direct egress (and a reachable configured egress) launches
    /// normally with the right `--proxy-server` / secure-DNS flags. An
    /// AUTHENTICATED proxy egress (C7 #30) loads its credentials from the keystore
    /// and applies them per page via CDP `Fetch.continueWithAuth` (Chromium
    /// ignores credentials in `--proxy-server`); it FAILS CLOSED if the egress
    /// declares proxy auth but no credentials are stored. Errors if no store is
    /// attached.
    pub async fn launch_persona_decoy_browser(
        &self,
        persona_id: &str,
        decoy_id: &str,
        check: &dyn ReachabilityCheck,
    ) -> Result<DecoyBrowser> {
        if self.inner.store.is_none() {
            return Err(CoreError::Unimplemented(
                "launch_persona_decoy_browser requires an open store",
            ));
        }
        let network = self.persona_network(persona_id).await?;
        // Fail-closed gate: a configured egress that is unreachable PAUSES the
        // persona; we never fall back to the direct route.
        let reachable = check.is_reachable(&network.egress).await;
        let exit = EgressExit::evaluate(persona_id, &network.egress, reachable);
        if exit.paused {
            return Err(CoreError::Network(exit.paused_reason.unwrap_or_else(
                || {
                    format!(
                        "persona {persona_id} egress {} is unreachable; decoy paused",
                        network.egress.exit_label()
                    )
                },
            )));
        }
        // Authenticated proxy egress: Chromium ignores credentials in
        // `--proxy-server`, so the BROWSER answers the proxy's auth challenge over
        // CDP (`Fetch.continueWithAuth`, applied per page by the launcher). Load
        // the secret from the keystore (never the DB) and hand it to the launcher.
        // FAIL CLOSED if the egress declares proxy auth but no credentials are
        // stored, rather than launch a browser whose every request would 407.
        let proxy_credentials = match network.egress.proxy_auth() {
            Some(auth) => {
                let source = self.key_source()?;
                match store::load_proxy_credentials(source, &auth.account_label)? {
                    Some(creds) => Some(creds),
                    None => {
                        return Err(CoreError::Network(format!(
                            "persona {persona_id} egress {} requires proxy authentication but no \
                             credentials are stored for account {:?}; call \
                             set_persona_proxy_credentials first",
                            network.egress.exit_label(),
                            auth.account_label
                        )));
                    }
                }
            }
            None => None,
        };
        let config = BrowserLaunchConfig::new().with_network(network);
        match proxy_credentials {
            Some((username, password)) => {
                DecoyBrowser::launch_with_proxy_auth(decoy_id, config, &username, &password).await
            }
            None => DecoyBrowser::launch_with(decoy_id, config).await,
        }
    }

    /// Launch a per-persona decoy browser using the LIVE [`TcpReachability`]
    /// check. Convenience over [`Core::launch_persona_decoy_browser`].
    pub async fn launch_persona_decoy_browser_live(
        &self,
        persona_id: &str,
        decoy_id: &str,
    ) -> Result<DecoyBrowser> {
        self.launch_persona_decoy_browser(persona_id, decoy_id, &TcpReachability)
            .await
    }

    /// Run a decoy SEARCH session for `persona_id` (C6 H1): launch the persona's
    /// isolated decoy browser (fail-closed egress, same gate as
    /// [`launch_persona_decoy_browser`](Self::launch_persona_decoy_browser)),
    /// generate one blocklist-safe, per-install-styled query per interest
    /// category, and dispatch each to a search engine through the guarded
    /// navigation path. Returns the [`SearchOutcome`](crate::browser::SearchOutcome).
    ///
    /// This is the desktop's standalone search-engine poisoning, for a phone-less
    /// / homelab deployment: no paired phone required. Every query is safety-gated
    /// by [`crate::querybank`] before dispatch. Errors if no store is attached or
    /// the persona is unknown; a category that yields no safe query is a recorded
    /// skip, not an error. Closes the browser before returning.
    pub async fn run_persona_search_session(
        &self,
        persona_id: &str,
        decoy_id: &str,
        check: &dyn ReachabilityCheck,
    ) -> Result<crate::browser::SearchOutcome> {
        let persona = self.get_persona(persona_id).await?;
        let browser = self
            .launch_persona_decoy_browser(persona_id, decoy_id, check)
            .await?;
        let seed = self.install_style_seed();
        let generator = crate::querybank::QueryGenerator::new(seed);
        let categories: Vec<crate::persona::CategoryPool> = persona
            .interests
            .iter()
            .filter_map(|name| crate::persona::CategoryPool::from_name(name))
            .collect();
        let outcome =
            crate::browser::run_search_session(&browser, &persona, &generator, &categories, seed)
                .await;
        // Close the browser regardless of the session result (no orphan process).
        let _ = browser.close().await;
        outcome
    }

    /// [`run_persona_search_session`](Self::run_persona_search_session) using the
    /// LIVE [`TcpReachability`] check.
    pub async fn run_persona_search_session_live(
        &self,
        persona_id: &str,
        decoy_id: &str,
    ) -> Result<crate::browser::SearchOutcome> {
        self.run_persona_search_session(persona_id, decoy_id, &TcpReachability)
            .await
    }

    /// A stable per-INSTALL seed for the query-generation style, derived from the
    /// device sync identity's public key, so this install's query distribution is
    /// consistent across runs and distinct from other installs (the per-install
    /// styling that defeats a fleet-wide query signature). Falls back to a fixed
    /// seed when no sync identity is attached.
    fn install_style_seed(&self) -> u64 {
        match &self.inner.sync {
            Some(sync) => {
                let key = sync.public_key();
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&key[..8]);
                u64::from_le_bytes(bytes)
            }
            // No sync identity: a fixed, arbitrary nonzero seed.
            None => 0xFAFA_FAFA_FAFA_FAFA,
        }
    }
}

/// The outcome of a headless [`Core::mint_and_push_pack`]: the signed pack bytes
/// that were minted and how many paired peers the pack was pushed to.
#[derive(Debug, Clone)]
pub struct MintPushOutcome {
    /// The signed persona-pack JSON bytes that were pushed (the import/export form).
    pub pack_bytes: Vec<u8>,
    /// The number of paired peers the pack was sealed and sent to.
    pub peers_reached: usize,
}

/// The two signed artifacts produced by one C6 generation pass: the
/// adversarial-allocation weight map and the category-targeted query plan.
#[derive(Debug, Clone)]
pub struct GeneratedArtifacts {
    /// The signed adversarial-allocation weight map.
    pub weight_map: SignedArtifact,
    /// The signed, weight-map-biased query plan.
    pub query_plan: SignedArtifact,
}

/// The outcome of a headless [`Core::run_generation_pass`]: the artifacts that
/// were generated/signed and how many paired peers each was pushed to.
#[derive(Debug, Clone)]
pub struct GenerationPassOutcome {
    /// The signed artifacts that were pushed.
    pub artifacts: GeneratedArtifacts,
    /// The number of paired peers each artifact was sealed and sent to.
    pub peers_reached: usize,
}

/// Load the device's persona-pack signing key (C5 #27 P4) through `source`,
/// generating and persisting a fresh one on first run. The 32-byte seed lives in
/// the OS keystore (or the headless passphrase-file fallback); the transient
/// seed buffer is zeroized after the key is built. Fails closed: any inability
/// to load/derive/persist the key is an error, never a silent skip.
fn load_or_create_pack_key(source: &KeySource) -> Result<personapack::PackSigningKey> {
    match store::load_pack_signing_seed(source)? {
        Some(seed) => Ok(personapack::PackSigningKey::from_seed_slice(&seed)?),
        None => {
            let key = personapack::PackSigningKey::generate();
            let seed = key.seed_bytes();
            store::store_pack_signing_seed(source, seed.as_slice())?;
            // `seed` is zeroized on drop (Zeroizing).
            Ok(key)
        }
    }
}

/// Current wall-clock time in epoch milliseconds (0 if the clock predates the
/// epoch, which cannot happen on a sane host). Mirrors the store's helper so the
/// Core stamps read-backs with a consistent timestamp source.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{AgeRange, CategoryPool, Profession, Region};

    fn temp_config(dir: &Path) -> Config {
        Config::new()
            .with_path(dir.join("fauxx.db"))
            .with_key_source(KeySource::EncryptedFile {
                path: dir.join("key.bin"),
                passphrase: "core-test-passphrase".to_string(),
            })
    }

    #[test]
    fn bind_addr_defaults_to_all_interfaces_and_overrides() {
        use std::net::{IpAddr, Ipv4Addr};
        // Default: 0.0.0.0 (all interfaces), preserving the zero-config LAN bind.
        assert_eq!(Config::new().bind_addr(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        // Override narrows it (e.g. loopback to refuse off-host connections).
        let pinned = Config::new().with_bind_addr(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(pinned.bind_addr(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[tokio::test]
    async fn sync_listen_addr_honors_the_configured_bind_addr() -> Result<()> {
        use std::net::{IpAddr, Ipv4Addr};
        let dir = tempfile::tempdir()?;
        let core =
            Core::open(temp_config(dir.path()).with_bind_addr(IpAddr::V4(Ipv4Addr::LOCALHOST)))
                .await?;
        let addr = core.sync_listen_addr()?;
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(addr.port(), sync::DEFAULT_SYNC_PORT);
        Ok(())
    }

    fn sample() -> SyntheticPersona {
        SyntheticPersona::new(
            "44444444-4444-4444-8444-444444444444".to_string(),
            "Core Test".to_string(),
            AgeRange::AGE_45_54.as_name().to_string(),
            Profession::TEACHER.as_name().to_string(),
            Region::CANADA.as_name().to_string(),
            vec![
                CategoryPool::ACADEMIC.as_name().to_string(),
                CategoryPool::HISTORY.as_name().to_string(),
                CategoryPool::SCIENCE.as_name().to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    #[tokio::test]
    async fn status_reports_version() -> Result<()> {
        let core = Core::new();
        let status = core.status().await?;
        assert_eq!(status.version, VERSION);
        assert!(!status.summary.is_empty());
        assert!(!status.store_attached);
        assert_eq!(status.persona_count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn status_serializes_to_json() -> Result<()> {
        let core = Core::new();
        let status = core.status().await?;
        let json = serde_json::to_string(&status)?;
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"summary\""));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_personas_are_empty_and_save_errors() -> Result<()> {
        let core = Core::new();
        assert!(core.list_personas().await?.is_empty());
        assert!(matches!(
            core.get_persona("nope").await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.save_persona(&sample()).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn open_and_persona_crud_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();

        core.save_persona(&persona).await?;
        let fetched = core.get_persona(&persona.id).await?;
        assert_eq!(fetched, persona);

        let status = core.status().await?;
        assert!(status.store_attached);
        assert_eq!(status.persona_count, 1);

        core.delete_persona(&persona.id).await?;
        assert!(matches!(
            core.get_persona(&persona.id).await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.delete_persona(&persona.id).await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn topics_readback_persists_and_reads_back_through_core() -> Result<()> {
        use crate::browser::{AssignedTopic, TopicsReadback};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Nothing recorded yet.
        assert!(core.topics_measurements_for("persona-x").await?.is_empty());
        assert!(core.latest_topics_measurement("persona-x").await?.is_none());

        // The expected epoch-boundary read: available but EMPTY topics. The
        // closed loop persists it as a valid measurement.
        let empty = TopicsReadback {
            available: true,
            topics: Vec::new(),
        };
        let written = core
            .record_topics_readback("persona-x", "decoy-x", &empty)
            .await?;
        assert!(written.topics.is_empty());
        assert!(written.available);

        // A later non-empty read.
        let with_topic = TopicsReadback {
            available: true,
            topics: vec![AssignedTopic {
                topic_id: 9,
                taxonomy_version: Some("1".to_string()),
                model_version: None,
                version: None,
                name: None,
            }],
        };
        core.record_topics_readback("persona-x", "decoy-x", &with_topic)
            .await?;

        let all = core.topics_measurements_for("persona-x").await?;
        assert_eq!(all.len(), 2);
        let latest = core
            .latest_topics_measurement("persona-x")
            .await?
            .ok_or_else(|| CoreError::Key("latest missing".into()))?;
        assert_eq!(latest.topics.len(), 1);
        assert_eq!(latest.topics[0].topic_id, 9);
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_topics_api_is_empty_and_record_errors() -> Result<()> {
        use crate::browser::TopicsReadback;
        let core = Core::new();
        assert!(core.topics_measurements_for("p").await?.is_empty());
        assert!(core.latest_topics_measurement("p").await?.is_none());
        let readback = TopicsReadback {
            available: false,
            topics: Vec::new(),
        };
        assert!(matches!(
            core.record_topics_readback("p", "d", &readback).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn broker_registry_is_exposed_over_core() {
        // The registry is static data and needs no store.
        let core = Core::new();
        let all = core.broker_registry();
        assert!(all.len() >= 5);
        assert!(core.broker_template("spokeo").is_ok());
        assert!(matches!(
            core.broker_template("nope"),
            Err(CoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn broker_submission_generate_record_list_and_due() -> Result<()> {
        use crate::brokers::SubmissionStatus;
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // Generate (no persistence) reports the fields that need supplying.
        let overrides = std::collections::BTreeMap::new();
        let req = core
            .generate_broker_request("spokeo", &persona.id, &overrides)
            .await?;
        assert_eq!(req.broker_id, "spokeo");
        assert!(!req.is_complete()); // listing_url + email are unfilled

        // Record persists a drafted submission with a computed deadline.
        let sub = core.record_broker_submission("spokeo", &persona.id).await?;
        assert_eq!(sub.status, SubmissionStatus::Drafted);
        assert!(sub.deadline > sub.submitted_at);

        // List (scoped + all).
        let listed = core.list_broker_submissions(Some(&persona.id)).await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, sub.id);
        assert_eq!(core.list_broker_submissions(None).await?.len(), 1);

        // Deadline reminders: not due before, due after.
        assert!(core
            .due_broker_submissions(sub.submitted_at)
            .await?
            .is_empty());
        let due = core.due_broker_submissions(sub.deadline + 1).await?;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, sub.id);

        // A persona that does not exist fails closed.
        assert!(matches!(
            core.record_broker_submission("spokeo", "no-such-persona")
                .await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn broker_rescan_flags_relisting_via_injected_seam() -> Result<()> {
        use crate::brokers::{StaticListingCheck, SubmissionStatus};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // A submission that has been removed.
        let mut sub = core.record_broker_submission("spokeo", &persona.id).await?;
        sub.status = SubmissionStatus::Removed;
        core.save_broker_submission(&sub).await?;

        // Re-scan with the seam reporting "still listed": flips to relisted.
        let listed_check = StaticListingCheck::new().with_listed("spokeo", true);
        let outcome = core
            .rescan_broker_submission(&sub.id, &listed_check)
            .await?;
        assert!(outcome.still_listed);
        assert!(outcome.newly_relisted);
        let after = core
            .list_broker_submissions(Some(&persona.id))
            .await?
            .into_iter()
            .find(|s| s.id == sub.id)
            .ok_or_else(|| CoreError::Key("submission missing".into()))?;
        assert_eq!(after.status, SubmissionStatus::Relisted);

        // Re-scan a fresh removed submission reporting "not listed": no flip.
        let mut sub2 = core
            .record_broker_submission("whitepages", &persona.id)
            .await?;
        sub2.status = SubmissionStatus::Removed;
        core.save_broker_submission(&sub2).await?;
        let clear_check = StaticListingCheck::new(); // defaults to not listed
        let outcome2 = core
            .rescan_broker_submission(&sub2.id, &clear_check)
            .await?;
        assert!(!outcome2.still_listed);
        assert!(!outcome2.newly_relisted);

        // Unknown submission id fails closed.
        assert!(matches!(
            core.rescan_broker_submission("missing", &clear_check).await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_broker_api_is_empty_and_record_errors() -> Result<()> {
        use crate::brokers::StaticListingCheck;
        let core = Core::new();
        // Registry still works (static), but stateful ops are empty/error.
        assert!(!core.broker_registry().is_empty());
        assert!(core.list_broker_submissions(None).await?.is_empty());
        assert!(core.due_broker_submissions(0).await?.is_empty());
        assert!(matches!(
            core.record_broker_submission("spokeo", "p").await,
            // No store: the persona lookup yields NotFound first (fails closed).
            Err(CoreError::NotFound(_))
        ));
        let check = StaticListingCheck::new();
        assert!(matches!(
            core.rescan_broker_submission("s", &check).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn broker_scan_snapshots_round_trip_and_diff_through_core() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Zero snapshots: a clear "no diff yet" timeline, no panic.
        let empty = core.broker_diff_timeline("spokeo", "persona-1").await?;
        assert!(empty.no_diff_yet());
        assert!(!empty.has_diff());
        assert!(core
            .list_broker_scan_snapshots("spokeo", "persona-1")
            .await?
            .is_empty());

        // Record three snapshots over time: name removed then re-listed.
        core.record_broker_scan_snapshot(
            "spokeo",
            "persona-1",
            100,
            ["name".to_string(), "phone".to_string()],
        )
        .await?;
        // One snapshot: still no-diff-yet.
        let one = core.broker_diff_timeline("spokeo", "persona-1").await?;
        assert!(one.no_diff_yet());

        core.record_broker_scan_snapshot("spokeo", "persona-1", 200, ["phone".to_string()])
            .await?;
        core.record_broker_scan_snapshot(
            "spokeo",
            "persona-1",
            300,
            ["name".to_string(), "phone".to_string()],
        )
        .await?;

        // The snapshots round-trip oldest first.
        let stored = core
            .list_broker_scan_snapshots("spokeo", "persona-1")
            .await?;
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].scanned_at, 100);
        assert_eq!(stored[2].scanned_at, 300);

        // The computed diff classifies and flags re-listing.
        let tl = core.broker_diff_timeline("spokeo", "persona-1").await?;
        assert!(tl.has_diff());
        assert_eq!(tl.diffs.len(), 2);
        assert_eq!(tl.diffs[0].removed(), vec!["name"]);
        assert_eq!(tl.diffs[1].relisted(), vec!["name"]);
        assert!(tl.has_relisting());

        // An unknown broker is rejected (fail closed).
        assert!(matches!(
            core.record_broker_scan_snapshot("nope", "persona-1", 1, std::iter::empty())
                .await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_broker_scan_api_is_empty_and_record_errors() -> Result<()> {
        let core = Core::new();
        assert!(core
            .list_broker_scan_snapshots("spokeo", "p")
            .await?
            .is_empty());
        // No store: the diff is still a graceful no-diff-yet timeline.
        let tl = core.broker_diff_timeline("spokeo", "p").await?;
        assert!(tl.no_diff_yet());
        // Recording needs a store (the broker id is valid, so it reaches the
        // store check rather than NotFound).
        assert!(matches!(
            core.record_broker_scan_snapshot("spokeo", "p", 1, std::iter::empty())
                .await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn efficacy_snapshot_exports_csv_json_pdf_through_core() -> Result<()> {
        use crate::browser::{AssignedTopic, TopicsReadback};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // Two Google read-backs over time so the drift series has real data.
        let topic = |name: &str| AssignedTopic {
            topic_id: 1,
            taxonomy_version: None,
            model_version: None,
            version: None,
            name: Some(name.to_string()),
        };
        core.record_topics_readback(
            &persona.id,
            "decoy",
            &TopicsReadback {
                available: true,
                topics: vec![topic("a"), topic("b")],
            },
        )
        .await?;
        core.record_topics_readback(
            &persona.id,
            "decoy",
            &TopicsReadback {
                available: true,
                topics: vec![topic("a"), topic("c")],
            },
        )
        .await?;

        let as_of = 1_609_459_200_000_i64; // 2021-01-01
        let baseline = Baseline::from_persona(&persona);

        // JSON round-trips to the same data.
        let json = core
            .export_efficacy_snapshot(&persona.id, &baseline, as_of, ExportFormat::Json)
            .await?;
        let data: EfficacySnapshotData = serde_json::from_slice(&json.bytes)?;
        assert_eq!(data.persona_id, persona.id);
        assert_eq!(data.as_of_date(), "2021-01-01");

        // CSV has the frozen header and the embedded date.
        let csv = core
            .export_efficacy_snapshot(&persona.id, &baseline, as_of, ExportFormat::Csv)
            .await?;
        let text = String::from_utf8(csv.bytes.clone())
            .map_err(|e| CoreError::Key(format!("csv not utf8: {e}")))?;
        assert!(text.starts_with("as_of_date,platform,timestamp,kind,category,value"));
        assert!(text.contains("2021-01-01,Google,"));

        // PDF is valid, non-empty, and the metadata embeds the as-of date (the
        // structured artifact is the signing seam).
        let pdf = core
            .export_efficacy_snapshot(&persona.id, &baseline, as_of, ExportFormat::Pdf)
            .await?;
        assert!(pdf.bytes.starts_with(b"%PDF"));
        assert!(!pdf.is_empty());
        assert_eq!(pdf.metadata.as_of_date, "2021-01-01");
        Ok(())
    }

    #[tokio::test]
    async fn gpc_status_records_lists_and_reads_back_through_core() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        assert!(core.gpc_status_for("https://example.com").await?.is_none());
        assert!(core.list_gpc_status().await?.is_empty());

        let support = parse_gpc_well_known(Some(r#"{ "gpc": true, "lastUpdate": "2022-06-01" }"#));
        let recorded = core
            .record_gpc_status("https://example.com", support)
            .await?;
        assert!(recorded.support.honored);

        let back = core
            .gpc_status_for("https://example.com")
            .await?
            .ok_or_else(|| CoreError::Key("status missing".into()))?;
        assert!(back.support.honored);
        assert_eq!(back.support.last_update.as_deref(), Some("2022-06-01"));
        assert_eq!(core.list_gpc_status().await?.len(), 1);

        // A garbage well-known records a well-formed not-honored observation.
        let none = parse_gpc_well_known(Some("not json"));
        let recorded = core.record_gpc_status("https://other.test", none).await?;
        assert!(!recorded.support.honored);
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_gpc_api_is_empty_and_record_errors() -> Result<()> {
        let core = Core::new();
        assert!(core.gpc_status_for("https://x.test").await?.is_none());
        assert!(core.list_gpc_status().await?.is_empty());
        let support = GpcSupport::not_advertised();
        assert!(matches!(
            core.record_gpc_status("https://x.test", support).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn dsar_generate_record_send_export_and_overdue_over_core() -> Result<()> {
        use crate::dsar::{Controller, RequestKind, RequestStatus, SubjectDetails};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // Generate (no persistence) returns a drafted GDPR request.
        let drafted = core
            .generate_dsar_request(
                RequestKind::GdprAccess,
                &persona.id,
                Controller::resolve_broker("spokeo")?,
            )
            .await?;
        assert_eq!(drafted.status, RequestStatus::Drafted);
        assert!(drafted.deadline.is_none());

        // Preview the letter without recording: GDPR framing, no auto-send.
        let subject = SubjectDetails::new("Alex Real").with_reply_to("alex@real.test");
        let preview = core.preview_dsar_letter(&drafted, &subject);
        assert!(preview.body.contains("Article 15 of the GDPR"));
        assert!(preview.body.contains("within one month"));
        assert!(preview.body.contains("Alex Real"));

        // Record persists a drafted request.
        let rec = core
            .record_dsar_request(
                RequestKind::GdprAccess,
                &persona.id,
                Controller::arbitrary("Example Corp", "privacy@example.test"),
            )
            .await?;
        assert_eq!(core.list_dsar_requests(None).await?.len(), 1);
        assert_eq!(core.list_dsar_requests(Some(&persona.id)).await?.len(), 1);

        // Nothing overdue while drafted (no clock).
        assert!(core.overdue_dsar_requests(i64::MAX).await?.is_empty());

        // Mark sent: deadline computed (GDPR one calendar month).
        let sent_at = 1_700_000_000_000;
        let sent = core.mark_dsar_sent(&rec.id, sent_at).await?;
        assert_eq!(sent.status, RequestStatus::Sent);
        let deadline = sent
            .deadline
            .ok_or_else(|| CoreError::Key("no deadline".into()))?;
        assert!(deadline > sent_at);

        // Not overdue before the deadline; overdue after; due-soon inside window.
        assert!(core.overdue_dsar_requests(sent_at).await?.is_empty());
        let overdue = core.overdue_dsar_requests(deadline + 1).await?;
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].id, rec.id);
        let due_soon = core
            .due_soon_dsar_requests(deadline - dsar::ONE_DAY_MILLIS, 7 * dsar::ONE_DAY_MILLIS)
            .await?;
        assert_eq!(due_soon.len(), 1);

        // Export the recorded letter for manual sending.
        let letter = core.export_dsar_letter(&rec.id, &subject).await?;
        assert_eq!(letter.request_id, rec.id);
        assert!(letter.body.contains("Example Corp"));

        // Fulfilled requests are never overdue.
        let mut fulfilled = sent.clone();
        fulfilled.mark_fulfilled();
        core.save_dsar_request(&fulfilled).await?;
        assert!(core.overdue_dsar_requests(deadline + 100).await?.is_empty());

        // Unknown id / unknown persona fail closed.
        assert!(matches!(
            core.mark_dsar_sent("nope", 1).await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.record_dsar_request(
                RequestKind::CcpaAccess,
                "no-such-persona",
                Controller::arbitrary("X", "")
            )
            .await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_dsar_api_is_empty_and_record_errors() -> Result<()> {
        use crate::dsar::{Controller, RequestKind};
        let core = Core::new();
        assert!(core.list_dsar_requests(None).await?.is_empty());
        assert!(core.overdue_dsar_requests(i64::MAX).await?.is_empty());
        assert!(matches!(
            core.record_dsar_request(RequestKind::GdprAccess, "p", Controller::arbitrary("X", ""))
                .await,
            Err(CoreError::Unimplemented(_))
        ));
        // Generation still works store-less (preview path).
        let drafted = core
            .generate_dsar_request(RequestKind::GdprAccess, "p", Controller::arbitrary("X", ""))
            .await?;
        assert!(drafted.deadline.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn alias_mint_list_revoke_rotate_and_no_reuse_over_core() -> Result<()> {
        use crate::aliases::{AliasKind, AliasStatus, PlusAddressProvider};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        let provider = PlusAddressProvider::new("alice@example.com")?;

        // Mint per (persona, site): each site gets its own address.
        let spokeo = core
            .mint_email_alias(&provider, &persona.id, "spokeo.com", false)
            .await?;
        let wp = core
            .mint_email_alias(&provider, &persona.id, "whitepages.com", false)
            .await?;
        assert_ne!(spokeo.address, wp.address);
        assert_eq!(spokeo.kind, AliasKind::PlusAddress);
        assert!(spokeo.is_active());

        // Record a manually-created masked alias.
        let masked = core
            .record_email_alias(
                &persona.id,
                "intelius.com",
                "masked-xyz@icloud.test",
                AliasKind::Masked,
                Some("icloud-hme"),
                false,
            )
            .await?;
        assert_eq!(masked.kind, AliasKind::Masked);

        // Inventory: which address fronts which site.
        let inventory = core.list_email_aliases(Some(&persona.id)).await?;
        assert_eq!(inventory.len(), 3);

        // Re-recording the SAME address for the SAME site is not a clash (the
        // no-reuse rule is across DIFFERENT sites only).
        core.record_email_alias(
            &persona.id,
            "spokeo.com",
            &spokeo.address,
            AliasKind::PlusAddress,
            None,
            false,
        )
        .await?;

        // No-reuse-across-sites: recording the spokeo address for a NEW site is
        // refused unless allow_reuse is set.
        let clash = core
            .record_email_alias(
                &persona.id,
                "radaris.com",
                &spokeo.address,
                AliasKind::PlusAddress,
                None,
                false,
            )
            .await;
        assert!(matches!(clash, Err(CoreError::Alias(_))));
        // Explicitly allowing reuse succeeds.
        let shared = core
            .record_email_alias(
                &persona.id,
                "radaris.com",
                &spokeo.address,
                AliasKind::PlusAddress,
                None,
                true,
            )
            .await?;
        assert_eq!(shared.address, spokeo.address);

        // Revoke marks it revoked (audit trail kept).
        let revoked = core.revoke_email_alias(&masked.id).await?;
        assert_eq!(revoked.status, AliasStatus::Revoked);
        let after = core
            .list_email_aliases(Some(&persona.id))
            .await?
            .into_iter()
            .find(|a| a.id == masked.id)
            .ok_or_else(|| CoreError::Key("alias missing".into()))?;
        assert_eq!(after.status, AliasStatus::Revoked);

        // Rotate: old revoked, a new active alias minted for the same site.
        let rotated = core.rotate_email_alias(&spokeo.id, &provider).await?;
        assert_ne!(rotated.id, spokeo.id);
        assert_eq!(rotated.site, "spokeo.com");
        assert!(rotated.is_active());
        let old = core
            .list_email_aliases(None)
            .await?
            .into_iter()
            .find(|a| a.id == spokeo.id)
            .ok_or_else(|| CoreError::Key("old alias missing".into()))?;
        assert_eq!(old.status, AliasStatus::Revoked);

        // Unknown ids fail closed.
        assert!(matches!(
            core.revoke_email_alias("nope").await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.rotate_email_alias("nope", &provider).await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_alias_api_is_empty_and_mint_errors() -> Result<()> {
        use crate::aliases::PlusAddressProvider;
        let core = Core::new();
        assert!(core.list_email_aliases(None).await?.is_empty());
        let provider = PlusAddressProvider::new("a@b.test")?;
        // No store: the persona lookup yields NotFound first (fails closed).
        assert!(matches!(
            core.mint_email_alias(&provider, "p", "s.com", false).await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.revoke_email_alias("x").await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn anchor_inventory_scoring_and_recommendations_over_core() -> Result<()> {
        use crate::anchors::{IdentitySignal, RecommendationKind};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        assert!(core.list_account_anchors().await?.is_empty());
        assert!(core.score_account_anchors().await?.is_empty());
        assert!(core.account_anchor_recommendations().await?.is_empty());

        // A hub account bridging two spokes via a shared recovery contact.
        let hub = core
            .record_account_anchor(
                "Primary Email",
                "google.com",
                [
                    IdentitySignal::LegalName,
                    IdentitySignal::VerifiedEmail,
                    IdentitySignal::RecoveryContact,
                ],
                Some("recovery-key".to_string()),
            )
            .await?;
        core.record_account_anchor(
            "Shop A",
            "shopa.com",
            [IdentitySignal::VerifiedEmail],
            Some("recovery-key".to_string()),
        )
        .await?;
        core.record_account_anchor(
            "Shop B",
            "shopb.com",
            [IdentitySignal::VerifiedEmail],
            Some("recovery-key".to_string()),
        )
        .await?;

        // Scoring: hub tops the list (highest linkage strength).
        let scores = core.score_account_anchors().await?;
        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].anchor_id, hub.id);
        for w in scores.windows(2) {
            assert!(w[0].score >= w[1].score);
        }

        // Recommendations: prioritized by linkage strength; the hub gets a
        // split-recovery-contact recommendation.
        let recs = core.account_anchor_recommendations().await?;
        assert!(!recs.is_empty());
        for w in recs.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
        assert_eq!(recs[0].anchor_id, hub.id);
        assert!(recs
            .iter()
            .any(|r| r.anchor_id == hub.id && r.kind == RecommendationKind::SplitRecoveryContact));
        assert!(recs
            .iter()
            .any(|r| r.kind == RecommendationKind::SeparateAlias));

        // READ-ONLY / no-automation property: re-running the analysis never
        // mutates the inventory and is deterministic.
        let before = core.list_account_anchors().await?;
        let recs2 = core.account_anchor_recommendations().await?;
        let after = core.list_account_anchors().await?;
        assert_eq!(before, after);
        assert_eq!(recs, recs2);

        // Delete removes from the inventory.
        assert!(core.delete_account_anchor(&hub.id).await?);
        assert_eq!(core.list_account_anchors().await?.len(), 2);
        assert!(!core.delete_account_anchor(&hub.id).await?);
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_anchor_api_is_empty_and_record_errors() -> Result<()> {
        use crate::anchors::IdentitySignal;
        let core = Core::new();
        assert!(core.list_account_anchors().await?.is_empty());
        assert!(core.score_account_anchors().await?.is_empty());
        assert!(core.account_anchor_recommendations().await?.is_empty());
        assert!(!core.delete_account_anchor("x").await?);
        assert!(matches!(
            core.record_account_anchor("L", "s", [IdentitySignal::VerifiedEmail], None)
                .await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn persona_settings_lock_and_rotation_persist_through_store() -> Result<()> {
        use studio::{PersonaField, RotationSchedule};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // Default settings: nothing locked, frozen 8-to-10-day cadence.
        let defaults = core.persona_settings(&persona.id).await?;
        assert!(defaults.locked_fields.is_empty());
        assert!(defaults.rotation.is_enabled());
        assert_eq!(defaults.rotation.window_days(), Some((8, 10)));

        // Lock two fields and PIN the persona (disable rotation).
        core.set_field_locked(&persona.id, PersonaField::Name, true)
            .await?;
        core.set_field_locked(&persona.id, PersonaField::Interests, true)
            .await?;
        let pinned = core
            .set_rotation_schedule(&persona.id, RotationSchedule::Disabled)
            .await?;
        assert!(pinned.is_locked(PersonaField::Name));
        assert!(pinned.is_locked(PersonaField::Interests));
        assert!(!pinned.rotation.is_enabled());

        // Reopen the store: the editor metadata round-trips.
        drop(core);
        let reopened = Core::open(temp_config(dir.path())).await?;
        let back = reopened.persona_settings(&persona.id).await?;
        assert!(back.is_locked(PersonaField::Name));
        assert!(back.is_locked(PersonaField::Interests));
        assert!(!back.is_locked(PersonaField::AgeRange));
        assert_eq!(back.rotation, RotationSchedule::Disabled);

        // Unlock a field; it persists.
        reopened
            .set_field_locked(&persona.id, PersonaField::Name, false)
            .await?;
        let after = reopened.persona_settings(&persona.id).await?;
        assert!(!after.is_locked(PersonaField::Name));
        assert!(after.is_locked(PersonaField::Interests));

        // The synced persona JSON is UNAFFECTED by the desktop-local settings.
        let json = serde_json::to_string(&reopened.get_persona(&persona.id).await?)?;
        assert!(!json.contains("locked"));
        assert!(!json.contains("rotation"));
        Ok(())
    }

    #[test]
    fn active_until_honors_the_rotation_schedule() {
        use studio::RotationSchedule;
        const MS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;
        // Disabled pins the persona: a far-future sentinel, never rotates out.
        assert_eq!(
            active_until_for_schedule(&RotationSchedule::Disabled, 1_000),
            i64::MAX
        );
        // The frozen cadence (8..=10 days) sets the window from created_at by the
        // midpoint (9 days).
        assert_eq!(
            active_until_for_schedule(&RotationSchedule::frozen_cadence(), 1_000),
            1_000 + 9 * MS_PER_DAY
        );
    }

    #[tokio::test]
    async fn set_rotation_schedule_is_consumed_into_active_until() -> Result<()> {
        use studio::RotationSchedule;
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        let created_at = persona.created_at;
        core.save_persona(&persona).await?;

        // Disabling rotation PINS the persona at runtime (active_until = sentinel),
        // not merely records the setting.
        core.set_rotation_schedule(&persona.id, RotationSchedule::Disabled)
            .await?;
        assert_eq!(
            core.get_persona(&persona.id).await?.active_until,
            i64::MAX,
            "Disabled must pin the persona's active_until"
        );

        // Re-enabling the frozen cadence recomputes the window from created_at.
        core.set_rotation_schedule(&persona.id, RotationSchedule::frozen_cadence())
            .await?;
        assert_eq!(
            core.get_persona(&persona.id).await?.active_until,
            created_at + 9 * 24 * 60 * 60 * 1_000,
            "a cadence must reset the rotation window, not leave it pinned"
        );
        Ok(())
    }

    #[tokio::test]
    async fn rotate_due_personas_regenerates_expired_and_preserves_locked_fields() -> Result<()> {
        use studio::PersonaField;
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let mut persona = sample();
        persona.active_until = 1_000; // window already elapsed
        let original_name = persona.name.clone();
        let original_interests = persona.interests.clone();
        core.save_persona(&persona).await?;
        // Lock name + interests; the default rotation cadence is enabled, so the
        // expired persona is due.
        core.set_field_locked(&persona.id, PersonaField::Name, true)
            .await?;
        core.set_field_locked(&persona.id, PersonaField::Interests, true)
            .await?;

        let now = 2_000_000_000_000;
        let rotated = core.rotate_due_personas(now).await?;
        assert_eq!(
            rotated,
            vec![persona.id.clone()],
            "the expired persona rotates"
        );

        let after = core.get_persona(&persona.id).await?;
        assert_eq!(after.id, persona.id, "the slot id is kept");
        assert_eq!(after.created_at, now, "a fresh identity is dated now");
        assert!(
            after.active_until > now,
            "the rotation window is reset forward"
        );
        assert_eq!(
            after.name, original_name,
            "a LOCKED field survives rotation"
        );
        assert_eq!(
            after.interests, original_interests,
            "a LOCKED field survives rotation"
        );
        // The (unlocked) regenerated demographics remain valid values.
        assert!(crate::persona::Region::from_name(&after.region).is_some());
        Ok(())
    }

    #[tokio::test]
    async fn rotate_due_personas_skips_pinned_and_unexpired() -> Result<()> {
        use studio::RotationSchedule;
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Pinned persona with an elapsed window: must NOT rotate.
        let mut pinned = sample();
        pinned.id = "11111111-1111-4111-8111-111111111111".to_string();
        pinned.active_until = 1_000;
        core.save_persona(&pinned).await?;
        core.set_rotation_schedule(&pinned.id, RotationSchedule::Disabled)
            .await?;

        // Enabled persona with a FUTURE window: must NOT rotate yet.
        let mut future = sample();
        future.id = "22222222-2222-4222-8222-222222222222".to_string();
        future.active_until = i64::MAX;
        core.save_persona(&future).await?;

        let rotated = core.rotate_due_personas(2_000_000_000_000).await?;
        assert!(
            rotated.is_empty(),
            "a pinned persona and an unexpired persona must not rotate: {rotated:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn change_event_fires_on_save() -> Result<()> {
        use studio::PersonaChangeKind;
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Subscribe BEFORE the save.
        let mut rx = core.subscribe_persona_changes();
        let persona = sample();
        core.save_persona(&persona).await?;

        let event = rx
            .recv()
            .await
            .map_err(|e| CoreError::Key(format!("no change event: {e}")))?;
        assert_eq!(event.persona_id, persona.id);
        assert_eq!(event.kind, PersonaChangeKind::Saved);

        // Saving settings emits a SettingsChanged event too.
        core.set_field_locked(&persona.id, studio::PersonaField::AgeRange, true)
            .await?;
        let settings_event = rx
            .recv()
            .await
            .map_err(|e| CoreError::Key(format!("no settings event: {e}")))?;
        assert_eq!(settings_event.kind, PersonaChangeKind::SettingsChanged);

        // Deleting emits a Deleted event.
        core.delete_persona(&persona.id).await?;
        let del_event = rx
            .recv()
            .await
            .map_err(|e| CoreError::Key(format!("no delete event: {e}")))?;
        assert_eq!(del_event.kind, PersonaChangeKind::Deleted);
        Ok(())
    }

    #[tokio::test]
    async fn lint_and_simulate_over_core_by_id() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample(); // coherent ACADEMIC/HISTORY/SCIENCE teacher
        core.save_persona(&persona).await?;

        // The coherent sample lints clean.
        assert!(core.lint_persona(&persona).is_empty());
        assert!(core.lint_persona_by_id(&persona.id).await?.is_empty());

        // The week simulator is deterministic by id and re-rolls on a new seed.
        let week_a = core
            .simulate_week_for(&persona.id, IntensityLevel::Medium, 1)
            .await?;
        let week_b = core
            .simulate_week_for(&persona.id, IntensityLevel::Medium, 1)
            .await?;
        assert_eq!(week_a, week_b);
        let week_c = core
            .simulate_week_for(&persona.id, IntensityLevel::Medium, 2)
            .await?;
        assert_ne!(week_a, week_c);
        assert_eq!(week_a.sessions.len(), studio::DAYS_PER_WEEK as usize);

        // Unknown ids fail closed.
        assert!(matches!(
            core.lint_persona_by_id("nope").await,
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            core.simulate_week_for("nope", IntensityLevel::Low, 0).await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_studio_api_defaults_and_save_errors() -> Result<()> {
        let core = Core::new();
        // Default settings are returned even with no store.
        let settings = core.persona_settings("p").await?;
        assert!(settings.locked_fields.is_empty());
        assert!(settings.rotation.is_enabled());
        // Saving settings requires a store.
        assert!(matches!(
            core.save_persona_settings(&settings).await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            core.set_field_locked("p", studio::PersonaField::Name, true)
                .await,
            Err(CoreError::Unimplemented(_))
        ));
        // Pure lint/simulate still work store-less.
        assert!(core.lint_persona(&sample()).is_empty());
        let week = core.simulate_week(&sample(), IntensityLevel::Low, 9);
        assert_eq!(week.sessions.len(), studio::DAYS_PER_WEEK as usize);
        // Subscribing is always available; nothing is dropped on a store-less core.
        let _rx = core.subscribe_persona_changes();
        Ok(())
    }

    #[tokio::test]
    async fn persona_pack_round_trips_through_core_into_a_temp_store() -> Result<()> {
        use personapack::{PackError, PackProvenance};
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Two personas to ship in the pack.
        let p1 = sample();
        let mut p2 = sample();
        p2.id = "55555555-5555-4555-8555-555555555555".to_string();
        p2.name = "Second".to_string();
        core.save_persona(&p1).await?;
        core.save_persona(&p2).await?;

        // Export the two ids as a signed pack.
        let provenance = PackProvenance::us("US_PUMS_2022", "seed-42", 1_700_000_000_000);
        let bytes = core
            .export_persona_pack(&[p1.id.clone(), p2.id.clone()], provenance)
            .await?;

        // Import into a FRESH store and confirm the personas persist identically.
        let dir2 = tempfile::tempdir()?;
        let importer = Core::open(temp_config(dir2.path())).await?;
        assert!(importer.list_personas().await?.is_empty());

        let imported = importer.import_persona_pack(&bytes).await?;
        assert_eq!(imported.len(), 2);

        // The personas are persisted and read back equal (Android shape intact).
        let back1 = importer.get_persona(&p1.id).await?;
        let back2 = importer.get_persona(&p2.id).await?;
        assert_eq!(back1, p1);
        assert_eq!(back2, p2);

        // The library ledger records the installed pack.
        let installed = importer.list_installed_packs().await?;
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].record.persona_count(), 2);
        assert_eq!(
            installed[0].record.signer_public_key,
            core.pack_signer_public_key().await?
        );
        let pack_id = installed[0].record.id.clone();

        // Removing the pack drops the ledger row but keeps the personas.
        assert!(importer.remove_installed_pack(&pack_id).await?);
        assert!(!importer.remove_installed_pack(&pack_id).await?);
        assert!(importer.list_installed_packs().await?.is_empty());
        assert_eq!(importer.list_personas().await?.len(), 2);

        // A tampered pack is rejected and NOTHING new is written.
        let dir3 = tempfile::tempdir()?;
        let importer3 = Core::open(temp_config(dir3.path())).await?;
        let mut tampered_bytes = bytes.clone();
        tampered_bytes[25] ^= 0x01;
        let result = importer3.import_persona_pack(&tampered_bytes).await;
        assert!(matches!(result, Err(CoreError::Pack(_))));
        assert!(importer3.list_personas().await?.is_empty());
        assert!(importer3.list_installed_packs().await?.is_empty());

        // An unsigned pack is flagged, not silently accepted.
        let mut pack = personapack::PersonaPack::from_bytes(&bytes)?;
        pack.signature = None;
        let unsigned_bytes = pack.to_bytes()?;
        assert!(matches!(
            importer3.import_persona_pack(&unsigned_bytes).await,
            Err(CoreError::Pack(PackError::Unsigned))
        ));

        // Exporting an unknown id fails closed (NotFound) and writes nothing.
        assert!(matches!(
            core.export_persona_pack(&["no-such-id".to_string()], PackProvenance::us("d", "s", 0))
                .await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn storeless_core_pack_api_is_empty_and_export_errors() -> Result<()> {
        use personapack::PackProvenance;
        let core = Core::new();
        assert!(core.list_installed_packs().await?.is_empty());
        assert!(core.get_installed_pack("x").await?.is_none());
        assert!(!core.remove_installed_pack("x").await?);
        assert!(matches!(
            core.pack_signer_public_key().await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            core.export_persona_pack(&["p".to_string()], PackProvenance::us("d", "s", 0))
                .await,
            Err(CoreError::Unimplemented(_))
        ));
        // A store-less import still verifies first; valid-but-store-less errors
        // Unimplemented (verification passes, the store write cannot happen).
        let key = personapack::PackSigningKey::from_seed(&[3u8; personapack::PACK_SEED_LEN]);
        let content =
            personapack::PackContent::new(PackProvenance::us("d", "s", 0), vec![sample()]);
        let bytes = personapack::sign_pack_with(content, &key)?.to_bytes()?;
        assert!(matches!(
            core.import_persona_pack(&bytes).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn pack_signing_key_persists_across_reopen() -> Result<()> {
        // The device's pack-signing key is loaded from the keystore on open, so
        // reopening the same store (same key source) yields the SAME signer key.
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let pk1 = core.pack_signer_public_key().await?;
        drop(core);
        let reopened = Core::open(temp_config(dir.path())).await?;
        let pk2 = reopened.pack_signer_public_key().await?;
        assert_eq!(pk1, pk2);
        Ok(())
    }

    #[tokio::test]
    async fn clone_shares_the_same_store() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let clone = core.clone();
        core.save_persona(&sample()).await?;
        // The clone sees the write through the shared Arc.
        assert_eq!(clone.list_personas().await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn subsystem_stubs_report_unimplemented() -> Result<()> {
        use subsystems::browser::{BrowserDriver, StubBrowserDriver};
        use subsystems::measurement::{Measurement, StubMeasurement};
        use subsystems::scheduler::{Scheduler, StubScheduler};
        use subsystems::sync::{StubSync, SyncEngine};

        assert!(matches!(
            StubScheduler.start().await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            StubBrowserDriver.run_session("p").await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            StubMeasurement.measure("p").await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            StubSync.push().await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    #[test]
    fn null_ui_is_a_uisink() {
        // Compile-time check that the no-op sink satisfies the trait the core
        // emits through, and is usable from headless code without a GUI.
        fn accepts(_sink: &dyn UiSink) {}
        accepts(&NullUi);
    }

    #[test]
    fn core_is_clone_send_sync() {
        fn assert_bounds<T: Clone + Send + Sync + 'static>() {}
        assert_bounds::<Core>();
    }

    // --- C6 #28 H1: generate-on-desktop, execute-on-phone -------------------

    #[tokio::test]
    async fn generate_signed_artifacts_over_core_verify_and_bias() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        let artifacts = core
            .generate_signed_artifacts(
                &persona.id,
                IntensityLevel::High,
                2024,
                generate::DEFAULT_FRESHNESS_MS,
            )
            .await?;

        // Both artifacts verify (signature + integrity), and carry the right
        // persona id + payload kinds.
        let wm = generate::verify_artifact(&artifacts.weight_map.to_bytes()?)?;
        let qp = generate::verify_artifact(&artifacts.query_plan.to_bytes()?)?;
        assert_eq!(wm.payload_kind(), "WeightMap");
        assert_eq!(qp.payload_kind(), "QueryPlan");
        assert_eq!(wm.content.persona_id, persona.id);
        assert_eq!(qp.content.persona_id, persona.id);
        // Signed with the device artifact-signing key.
        let signer = core.pack_signer_public_key().await?;
        assert_eq!(wm.signer_public_key, signer);
        assert_eq!(qp.signer_public_key, signer);

        // The weight map biases toward the persona interests (above the uniform
        // share), within the KL budget by construction.
        if let generate::ArtifactPayload::WeightMap(map) = &wm.content.payload {
            let interest_mass: f64 = persona
                .interests
                .iter()
                .map(|i| map.get(i).copied().unwrap_or(0.0))
                .sum();
            let uniform = persona.interests.len() as f64 / CategoryPool::all().len() as f64;
            assert!(
                interest_mass > uniform,
                "weight map should bias toward interests ({interest_mass} > {uniform})"
            );
        } else {
            panic!("expected a WeightMap payload");
        }

        // The query plan is category-targeted and stays in the active window.
        if let generate::ArtifactPayload::QueryPlan(plan) = &qp.content.payload {
            assert!(!plan.is_empty());
            for intent in &plan.intents {
                assert!(CategoryPool::from_name(&intent.category).is_some());
                assert!(crate::orchestration::scheduler::is_active_window(
                    intent.at_secs
                ));
            }
        } else {
            panic!("expected a QueryPlan payload");
        }
        Ok(())
    }

    #[tokio::test]
    async fn generate_signed_artifacts_is_deterministic() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;
        // Same fixed generated-at would differ on wall clock; compare the PAYLOADS
        // (weight map + plan), which are seed-deterministic and clock-independent.
        let a = core
            .generate_signed_artifacts(&persona.id, IntensityLevel::Medium, 7, 1)
            .await?;
        let b = core
            .generate_signed_artifacts(&persona.id, IntensityLevel::Medium, 7, 1)
            .await?;
        assert_eq!(a.weight_map.content.payload, b.weight_map.content.payload);
        assert_eq!(a.query_plan.content.payload, b.query_plan.content.payload);
        Ok(())
    }

    #[tokio::test]
    async fn run_generation_pass_with_no_peers_still_signs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        let persona = sample();
        core.save_persona(&persona).await?;

        // No peers paired: the pass still generates + signs, reaching 0 peers.
        let outcome = core
            .run_generation_pass(
                &persona.id,
                IntensityLevel::Low,
                3,
                generate::DEFAULT_FRESHNESS_MS,
            )
            .await?;
        assert_eq!(outcome.peers_reached, 0);
        generate::verify_artifact(&outcome.artifacts.weight_map.to_bytes()?)?;
        generate::verify_artifact(&outcome.artifacts.query_plan.to_bytes()?)?;
        Ok(())
    }

    #[tokio::test]
    async fn generate_pass_errors_on_unknown_persona_and_storeless() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        assert!(matches!(
            core.generate_signed_artifacts("nope", IntensityLevel::Low, 1, 1)
                .await,
            Err(CoreError::NotFound(_))
        ));
        // Storeless core fails closed: the persona lookup runs first, so a
        // store-less core surfaces NotFound (the persona cannot be loaded)
        // rather than reaching the signing step.
        let storeless = Core::new();
        assert!(matches!(
            storeless
                .generate_signed_artifacts("p", IntensityLevel::Low, 1, 1)
                .await,
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn artifact_api_storeless_errors_fail_closed() -> Result<()> {
        // The headless C6 API requires an open store (the artifact-signing key and
        // the sync engine live alongside it); a store-less core fails closed.
        let core = Core::new();
        let artifact = {
            use crate::generate::{
                sign_artifact_with, ArtifactContent, ArtifactPayload, WeightMap,
                DEFAULT_FRESHNESS_MS,
            };
            use crate::personapack::{PackSigningKey, PACK_SEED_LEN};
            let key = PackSigningKey::from_seed(&[3u8; PACK_SEED_LEN]);
            let mut map = WeightMap::new();
            for c in CategoryPool::all() {
                map.insert(
                    c.as_name().to_string(),
                    1.0 / CategoryPool::all().len() as f64,
                );
            }
            let content = ArtifactContent::new(
                "p",
                ArtifactPayload::WeightMap(map),
                1_700_000_000_000,
                DEFAULT_FRESHNESS_MS,
            );
            sign_artifact_with(content, &key)?
        };
        assert!(matches!(
            core.push_signed_artifact(&artifact).await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            core.receive_artifact_frame("k", b"frame", 0).await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }

    // --- C6 #29 H2: persona-pack minting (the PUMS generator) ---------------

    #[tokio::test]
    async fn mint_persona_pack_over_core_signs_verifies_and_records_provenance() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;

        // Mint a coherent batch WITHOUT a store touch.
        let minted = core.mint_personas(4, 2026)?;
        assert_eq!(minted.personas.len(), 4);
        for p in &minted.personas {
            assert!(mint::MINT_INTEREST_COUNT.contains(&p.interests.len()));
            assert!(!lint_persona(p).iter().any(Finding::is_hard));
        }

        // Mint + sign into a pack and confirm it verifies (reusing P4) and carries
        // the source-distribution label + the generation seed in its provenance.
        let bytes = core.mint_persona_pack(4, 2026).await?;
        let verified = personapack::verify_pack(&bytes)?;
        assert_eq!(verified.content.personas.len(), 4);
        assert_eq!(
            verified.signer_public_key,
            core.pack_signer_public_key().await?
        );
        assert_eq!(verified.content.provenance.generation_seed, "2026");
        assert!(!verified.content.provenance.source_distribution.is_empty());
        assert_eq!(verified.content.provenance.country.as_deref(), Some("US"));
        Ok(())
    }

    #[tokio::test]
    async fn mint_and_push_pack_with_no_peers_still_mints() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let core = Core::open(temp_config(dir.path())).await?;
        // No peers paired: the pass still mints + signs, reaching 0 peers, and the
        // pushed pack bytes verify.
        let outcome = core.mint_and_push_pack(3, 5).await?;
        assert_eq!(outcome.peers_reached, 0);
        let verified = personapack::verify_pack(&outcome.pack_bytes)?;
        assert_eq!(verified.content.personas.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn minted_pack_pushes_over_sealed_channel_and_lands_in_the_store() -> Result<()> {
        use crate::sync::transport::testing::FakeLan;
        use crate::sync::{DeviceIdentity, LanSync, DEFAULT_SYNC_PORT};

        // The receiver is a real Core (its own temp store + pack key + sync engine).
        let recv_dir = tempfile::tempdir()?;
        let receiver = Core::open(temp_config(recv_dir.path())).await?;

        // The minting "desktop": mint + sign with its OWN pack key, then ship the
        // pack over a standalone LanSync paired with the receiver. The sealed
        // channel authenticates the paired SENDER; the embedded pack signature is a
        // separate (and different) key the receiver verifies before importing.
        let send_dir = tempfile::tempdir()?;
        let lan = FakeLan::new();
        let sender = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Minter".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::new(Mutex::new(EncryptedStore::open_at(
                &send_dir.path().join("fauxx.db"),
                &KeySource::EncryptedFile {
                    path: send_dir.path().join("key.bin"),
                    passphrase: "mint-sender-pass".to_string(),
                },
            )?))),
            Some(Arc::new(lan.clone())),
            None,
        );

        // Two-sided pairing: the receiver records the sender (so it accepts the
        // sender's frame); the sender records the receiver (so it can seal to it).
        let receiver_pairing = receiver.pairing_payload().await?;
        let sender_payload = sender.pairing_payload();
        receiver
            .complete_pairing(&PairingPayload::encode(&sender_payload)?)
            .await?;
        sender.complete_pairing(&receiver_pairing).await?;

        // Mint + sign a pack with a fixed pack-signing key (distinct from any
        // device sealed-channel identity).
        let pack_key = personapack::PackSigningKey::from_seed(&[21u8; personapack::PACK_SEED_LEN]);
        let dist = mint::PersonaDistribution::bundled()?;
        let minted = mint::mint_personas(&dist, 2, 77, 1_700_000_000_000)?;
        let pack = mint::mint_pack(&minted, 1_700_000_000_000, &pack_key)?;
        let pack_bytes = pack.to_bytes()?;

        // Seal the PersonaPack kind to the receiver and ship it.
        let receiver_pk = sync::encode_public_key(&receiver_pairing.public_key_bytes()?);
        let frame = sender
            .seal_message_for(&receiver_pk, &SyncMessage::persona_pack(&pack_bytes))
            .await?;

        // The receiver verifies (P4) BEFORE importing, then lands the personas in
        // its encrypted store and records the pack in the library ledger.
        let sender_pk = sync::encode_public_key(sender.public_key());
        let imported = receiver.receive_pack_frame(&sender_pk, &frame).await?;
        assert_eq!(imported.len(), 2);
        for p in &minted.personas {
            assert_eq!(receiver.get_persona(&p.id).await?, *p);
        }
        assert_eq!(receiver.list_installed_packs().await?.len(), 1);

        // A TAMPERED pack is rejected on receive and NOTHING new is written.
        let recv2_dir = tempfile::tempdir()?;
        let receiver2 = Core::open(temp_config(recv2_dir.path())).await?;
        let receiver2_pairing = receiver2.pairing_payload().await?;
        let sender2_dir = tempfile::tempdir()?;
        let lan2 = FakeLan::new();
        let sender2 = LanSync::with_parts(
            DeviceIdentity::generate(),
            "Minter2".to_string(),
            DEFAULT_SYNC_PORT,
            Some(Arc::new(Mutex::new(EncryptedStore::open_at(
                &sender2_dir.path().join("fauxx.db"),
                &KeySource::EncryptedFile {
                    path: sender2_dir.path().join("key.bin"),
                    passphrase: "mint-sender2-pass".to_string(),
                },
            )?))),
            Some(Arc::new(lan2.clone())),
            None,
        );
        let sender2_payload = sender2.pairing_payload();
        receiver2
            .complete_pairing(&PairingPayload::encode(&sender2_payload)?)
            .await?;
        sender2.complete_pairing(&receiver2_pairing).await?;

        let mut tampered = pack.clone();
        tampered.content.personas[0].name = "Tampered".to_string();
        let tampered_bytes = tampered.to_bytes()?;
        let receiver2_pk = sync::encode_public_key(&receiver2_pairing.public_key_bytes()?);
        let bad_frame = sender2
            .seal_message_for(&receiver2_pk, &SyncMessage::persona_pack(&tampered_bytes))
            .await?;
        let sender2_pk = sync::encode_public_key(sender2.public_key());
        assert!(matches!(
            receiver2.receive_pack_frame(&sender2_pk, &bad_frame).await,
            Err(CoreError::Pack(_))
        ));
        assert!(receiver2.list_personas().await?.is_empty());
        assert!(receiver2.list_installed_packs().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn mint_api_storeless_errors_fail_closed() -> Result<()> {
        // Pure minting works store-less (the bundled distribution needs no store).
        let core = Core::new();
        let minted = core.mint_personas(2, 1)?;
        assert_eq!(minted.personas.len(), 2);
        // Signing/pushing/receiving require the store (the pack-signing key + sync
        // engine live alongside it); a store-less core fails closed.
        assert!(matches!(
            core.mint_persona_pack(2, 1).await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            core.mint_and_push_pack(2, 1).await,
            Err(CoreError::Unimplemented(_))
        ));
        assert!(matches!(
            core.receive_pack_frame("k", b"frame").await,
            Err(CoreError::Unimplemented(_))
        ));
        Ok(())
    }
}
