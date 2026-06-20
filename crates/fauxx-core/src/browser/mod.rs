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

//! Real-browser decoy-profile automation (C2 #11 R1) with the strict-separation
//! guardrail (C2 #13 R3).
//!
//! This module launches a REAL, system Chromium against a DEDICATED, isolated
//! decoy user-data directory and drives it over the Chrome DevTools Protocol
//! (CDP) so a persona's synthetic browsing actually influences the real Topics
//! API on a throwaway profile. S1 ratified delegating real-browser Topics
//! influence to this desktop companion on a dedicated decoy profile, which is
//! what this implements.
//!
//! ## Why chromiumoxide (raw CDP) over thirtyfour (WebDriver)
//!
//! We drive Chromium over CDP via `chromiumoxide` rather than over WebDriver via
//! `thirtyfour`, because CDP gives the direct, low-level control the decoy needs
//! and WebDriver abstracts away:
//!
//! - **Direct `--user-data-dir` control**, the linchpin of the R3 isolation
//!   guardrail: we choose and verify the exact profile directory at launch.
//! - **Per-profile `--proxy-server`** so a decoy can later route through its own
//!   egress (C7) without disturbing the real browser.
//! - **Header / request control** for `Sec-GPC` and friends (D4c), set over CDP
//!   without a separate proxy.
//! - **`document.browsingTopics()` reads** (R2): CDP can evaluate page JS to
//!   observe the Topics the decoy has accrued, which is how efficacy is measured.
//!
//! WebDriver offers none of these directly; CDP is the right altitude for a
//! privacy decoy. No GUI/CLI types appear here; the core stays headless.
//!
//! ## Isolation (R3, fail closed)
//!
//! The launcher refuses to start unless the configured decoy dir is verifiably
//! distinct from every detected real browser profile (see [`isolation`]), and it
//! refuses navigation to authenticated-account sign-in endpoints. It NEVER
//! imports cookies, tokens, logins, or cache from a real profile: it only ever
//! creates and uses its own dedicated directory. Everything stays local; no
//! telemetry leaves the machine.
//!
//! ## Lifecycle
//!
//! [`DecoyBrowser::launch`] spawns the system Chromium child process and pumps
//! the CDP `Handler` on a tokio task. [`DecoyBrowser::close`] (and `Drop`) close
//! the browser, kill the child process, and stop the Handler task, leaving NO
//! orphaned browser processes.

pub mod cadence;
pub mod categories;
pub mod gpc;
pub mod isolation;
pub mod search;
pub mod topics;

use std::path::{Path, PathBuf};
use std::time::Duration;

use chromiumoxide::auth::Credentials;
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkEnableParams, Headers as NetworkHeaders, SetExtraHttpHeadersParams,
};
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
use chromiumoxide::{Browser, BrowserConfig};
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use crate::error::{CoreError, Result};
use crate::persona::SyntheticPersona;

pub use cadence::BrowsingCadence;
pub use categories::{
    category_sites, seed_history_for_persona, sites_for_categories, sites_for_persona, SeedOutcome,
};
pub use gpc::{parse_gpc_well_known, GpcSupport, GPC_WELL_KNOWN_PATH};
pub use search::{run_search_session, SearchDispatch, SearchOutcome};
pub use topics::{AssignedTopic, TopicsReadback};

/// Application data-dir coordinates, matching the encrypted store so the decoy
/// profiles live under the same per-OS app data directory.
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";
/// Subdirectory under the app data dir that holds all decoy profiles, one per
/// decoy id. Kept distinct from the encrypted store file and from any real
/// browser profile.
const DECOY_PROFILES_DIR: &str = "decoy-profiles";

/// The system Chromium path used when none is configured. We deliberately do
/// NOT use chromiumoxide's auto-fetcher (its `fetcher`/`native-tls` features are
/// left off); the decoy runs the real, system-installed browser.
pub const DEFAULT_CHROMIUM_PATH: &str = "/usr/bin/chromium";

/// Configuration for launching a decoy-profile browser.
///
/// Construct with [`BrowserLaunchConfig::new`] (system Chromium, headless) and
/// adjust with the builder helpers. Holds no GUI/CLI types.
#[derive(Debug, Clone)]
pub struct BrowserLaunchConfig {
    /// Path to the Chromium/Chrome executable. Defaults to the system Chromium
    /// ([`DEFAULT_CHROMIUM_PATH`]); never the chromiumoxide auto-fetcher.
    executable: PathBuf,
    /// The dedicated decoy user-data directory. Resolved from the app data dir
    /// + decoy id when not set explicitly.
    user_data_dir: Option<PathBuf>,
    /// Whether to run headless (the default; a visible window is opt-in).
    headless: bool,
    /// Known real-browser profile roots the decoy must stay separate from.
    /// Defaults to the OS-detected set; overridable for hermetic tests.
    real_profile_roots: Vec<PathBuf>,
    /// Whether to pass Chromium `--no-sandbox`. Default `false`: the decoy
    /// loads arbitrary web content, so the OS process sandbox stays ON. Only
    /// opt in on hosts where the sandbox is unavailable (some CI/containers,
    /// running as root), accepting the weaker isolation.
    no_sandbox: bool,
    /// Whether to enable the Privacy Sandbox Ads APIs (incl. the Topics API)
    /// for the Topics read-back flow (R2). Default `false` so the standard decoy
    /// launch is UNCHANGED; the Topics flow opts in via
    /// [`with_topics_enabled`](Self::with_topics_enabled), which adds the
    /// `document.browsingTopics()` feature flags at launch.
    topics_enabled: bool,
    /// Whether to emit Global Privacy Control on this decoy profile (D4c #18):
    /// the `Sec-GPC: 1` request header on every navigation plus
    /// `navigator.globalPrivacyControl = true` injected before page scripts run.
    /// Default `true`: GPC is a lawful opt-out signal we WANT every decoy
    /// navigation to carry. Applies ONLY to the isolated decoy profile (R3),
    /// never a real authenticated session. Toggle off with
    /// [`with_gpc_enabled`](Self::with_gpc_enabled) for the rare case a caller
    /// wants to observe a site without announcing GPC.
    gpc_enabled: bool,
    /// The per-persona network config (C7 #30 N1 / #31 N2): the egress
    /// (`--proxy-server`) and DNS strategy (secure-DNS flags) applied to THIS
    /// isolated decoy profile. Default [`crate::network::PersonaNetwork::default`] is Direct
    /// egress + SystemDefault DNS, so the standard launch is unchanged. The
    /// emitted args ride alongside the existing flags at launch.
    network: crate::network::PersonaNetwork,
}

impl BrowserLaunchConfig {
    /// A headless config that runs the system Chromium and detects the OS's real
    /// browser profile roots for the isolation guardrail.
    pub fn new() -> Self {
        Self {
            executable: PathBuf::from(DEFAULT_CHROMIUM_PATH),
            user_data_dir: None,
            headless: true,
            real_profile_roots: isolation::known_real_profile_roots(),
            no_sandbox: false,
            topics_enabled: false,
            // GPC is default-ON for decoy browsing: it is a signal we want to
            // emit on every decoy navigation (D4c #18).
            gpc_enabled: true,
            // Default network: Direct egress + SystemDefault DNS, so the standard
            // launch is unchanged until a persona's egress/DNS is bound (C7).
            network: crate::network::PersonaNetwork::default(),
        }
    }

    /// Override the browser executable path (still the system browser, never the
    /// fetcher).
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = path.into();
        self
    }

    /// Set the dedicated decoy user-data directory explicitly. When unset it is
    /// derived under the app data dir from the decoy id.
    pub fn with_user_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(dir.into());
        self
    }

    /// Run with a visible window (default is headless).
    pub fn with_head(mut self) -> Self {
        self.headless = false;
        self
    }

    /// Opt into Chromium `--no-sandbox`. Default is OFF (the OS sandbox stays
    /// on). Enable only on hosts where the sandbox cannot run (some CI and
    /// container environments, or running as root); this weakens the decoy's
    /// process isolation, so it is deliberately not the default.
    pub fn with_no_sandbox(mut self, no_sandbox: bool) -> Self {
        self.no_sandbox = no_sandbox;
        self
    }

    /// Override the real-browser profile roots used by the isolation guardrail.
    /// Primarily for hermetic tests that inject synthetic roots; production
    /// callers use the OS-detected defaults.
    pub fn with_real_profile_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.real_profile_roots = roots;
        self
    }

    /// Enable the Privacy Sandbox Ads APIs (incl. the Topics API) for the R2
    /// read-back flow. Default is OFF, so the standard decoy launch is unchanged;
    /// enable this only when seeding/reading Topics, since it turns on the
    /// `document.browsingTopics()` feature at launch.
    pub fn with_topics_enabled(mut self, enabled: bool) -> Self {
        self.topics_enabled = enabled;
        self
    }

    /// Toggle Global Privacy Control emission for this decoy launch (D4c #18).
    /// Default is ON; pass `false` to suppress the `Sec-GPC` header and the
    /// `navigator.globalPrivacyControl` flag for a navigation where the caller
    /// wants to observe a site without announcing GPC.
    pub fn with_gpc_enabled(mut self, enabled: bool) -> Self {
        self.gpc_enabled = enabled;
        self
    }

    /// Bind this decoy launch to a persona's network config (C7 #30 N1 / #31 N2):
    /// the egress (emitting `--proxy-server`) and DNS strategy (emitting the
    /// secure-DNS flags). Both apply to THIS isolated decoy profile. Tor maps to
    /// `socks5://127.0.0.1:9050`; DoH/DoT map to the secure-DNS template flags.
    pub fn with_network(mut self, network: crate::network::PersonaNetwork) -> Self {
        self.network = network;
        self
    }

    /// Bind just the [`Egress`](crate::network::Egress) for this launch, leaving
    /// the DNS strategy untouched.
    pub fn with_egress(mut self, egress: crate::network::Egress) -> Self {
        self.network.egress = egress;
        self
    }

    /// Bind just the [`DnsStrategy`](crate::network::DnsStrategy) for this launch,
    /// leaving the egress untouched.
    pub fn with_dns(mut self, dns: crate::network::DnsStrategy) -> Self {
        self.network.dns = dns;
        self
    }

    /// The per-persona network config bound to this launch.
    pub fn network(&self) -> &crate::network::PersonaNetwork {
        &self.network
    }

    /// The exact Chromium argument list this config's network policy emits, in
    /// deterministic order: the `--proxy-server` arg (if any) then the secure-DNS
    /// args. This is what the decoy launch applies and what the hermetic tests
    /// assert against WITHOUT launching a browser.
    pub fn network_chromium_args(&self) -> Vec<String> {
        self.network.chromium_args()
    }

    /// The configured executable path.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// The real-browser profile roots the isolation guardrail checks against.
    pub fn real_profile_roots(&self) -> &[PathBuf] {
        &self.real_profile_roots
    }

    /// Whether the Privacy Sandbox Topics flow is enabled for this launch.
    pub fn topics_enabled(&self) -> bool {
        self.topics_enabled
    }

    /// Whether Global Privacy Control is emitted on this decoy profile (D4c
    /// #18). Default `true`.
    pub fn gpc_enabled(&self) -> bool {
        self.gpc_enabled
    }
}

impl Default for BrowserLaunchConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The dedicated decoy-profiles root under the OS app data dir
/// (`<data_dir>/decoy-profiles`). Errors if no data directory can be resolved.
pub fn decoy_profiles_root() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .ok_or_else(|| CoreError::Browser("could not determine OS data directory".to_string()))?;
    Ok(dirs.data_dir().join(DECOY_PROFILES_DIR))
}

/// The dedicated user-data directory for one decoy id, under the app data dir
/// (`<data_dir>/decoy-profiles/<id>`). Always under the app data dir, never a
/// real browser profile path.
pub fn decoy_dir_for(id: &str) -> Result<PathBuf> {
    Ok(decoy_profiles_root()?.join(sanitize_id(id)))
}

/// Sanitize a decoy id into a single safe path segment: keep alphanumerics,
/// `-`, and `_`; replace anything else with `_`. This stops a crafted id from
/// climbing out of the decoy-profiles root (`..`, `/`) and keeps the dir
/// strictly under the app data dir.
fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "decoy".to_string()
    } else {
        cleaned
    }
}

/// Proxy-auth credentials for an authenticated egress (C7 #30), wrapped so they
/// never leak through `Debug`: chromiumoxide's [`Credentials`] derives a
/// plaintext `Debug`, so it must not appear in a `#[derive(Debug)]` field. The
/// secret lives only here and inside chromiumoxide's per-page network manager for
/// the life of the browser; it never touches the DB or a log.
struct RedactedCredentials(Credentials);

impl RedactedCredentials {
    fn new(username: &str, password: &str) -> Self {
        Self(Credentials {
            username: username.to_owned(),
            password: password.to_owned(),
        })
    }
}

impl std::fmt::Debug for RedactedCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A launched decoy-profile browser. Owns the Chromium child process (via the
/// chromiumoxide [`Browser`]) and the CDP `Handler` task. Closing it (or
/// dropping it) kills the child and stops the task; no orphan process remains.
#[derive(Debug)]
pub struct DecoyBrowser {
    browser: Browser,
    handler_task: Option<JoinHandle<()>>,
    user_data_dir: PathBuf,
    /// Whether Global Privacy Control is emitted on pages opened from this
    /// browser (D4c #18). Carried from the launch config so every
    /// [`new_page`](Self::new_page) applies it consistently.
    gpc_enabled: bool,
    /// Proxy-auth credentials applied to every page opened from this browser
    /// (C7 #30), when the persona's egress uses an AUTHENTICATED proxy. `None`
    /// for Direct/Tor/unauthenticated egress. Redacted from `Debug`.
    proxy_credentials: Option<RedactedCredentials>,
}

impl DecoyBrowser {
    /// Launch a decoy browser for `decoy_id`, deriving the dedicated user-data
    /// dir under the app data dir. See [`DecoyBrowser::launch_with`] for the
    /// full path.
    pub async fn launch(decoy_id: &str) -> Result<Self> {
        Self::launch_with(decoy_id, BrowserLaunchConfig::new()).await
    }

    /// Launch a decoy browser with an explicit config and NO proxy
    /// authentication (Direct / Tor / unauthenticated proxy egress).
    pub async fn launch_with(decoy_id: &str, config: BrowserLaunchConfig) -> Result<Self> {
        Self::launch_inner(decoy_id, config, None).await
    }

    /// Launch a decoy browser whose pages authenticate to an AUTHENTICATED-proxy
    /// egress (C7 #30). `username`/`password` are the keystore-sourced proxy
    /// credentials; they are applied per page via chromiumoxide's built-in
    /// `Fetch.continueWithAuth` handler (so the BROWSER answers the proxy's auth
    /// challenge - Chromium ignores credentials in `--proxy-server`). The secret
    /// rides only the browser, never the DB or a log, and is redacted from
    /// `Debug`.
    pub(crate) async fn launch_with_proxy_auth(
        decoy_id: &str,
        config: BrowserLaunchConfig,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        Self::launch_inner(
            decoy_id,
            config,
            Some(RedactedCredentials::new(username, password)),
        )
        .await
    }

    /// Launch a decoy browser for `decoy_id` with an explicit config.
    ///
    /// Resolves the dedicated decoy user-data dir (from the config or derived
    /// from the id), enforces the R3 isolation guardrail (the dir must be
    /// verifiably distinct from every real browser profile, else this fails
    /// closed), creates ONLY that dir, then launches the system Chromium against
    /// it and pumps the CDP `Handler` on a tokio task.
    async fn launch_inner(
        decoy_id: &str,
        config: BrowserLaunchConfig,
        proxy_credentials: Option<RedactedCredentials>,
    ) -> Result<Self> {
        // Resolve the dedicated decoy dir (explicit override or id-derived).
        let requested_dir = match &config.user_data_dir {
            Some(dir) => dir.clone(),
            None => decoy_dir_for(decoy_id)?,
        };

        // R3 guardrail, fail closed: the decoy dir must be verifiably distinct
        // from every detected real browser profile. This canonicalizes and runs
        // a two-way prefix check; an overlap is refused before we touch disk.
        let decoy_dir = isolation::ensure_isolated_from_real_profiles(
            &requested_dir,
            &config.real_profile_roots,
        )?;

        // Create ONLY our own dedicated directory. The launcher never reads,
        // copies, or imports anything from a real profile; this is the only
        // filesystem write it performs for the profile.
        std::fs::create_dir_all(&decoy_dir).map_err(|e| {
            CoreError::Browser(format!(
                "could not create decoy user-data dir {}: {e}",
                decoy_dir.display()
            ))
        })?;

        tracing::info!(
            target: "fauxx_core::browser",
            decoy_id,
            user_data_dir = %decoy_dir.display(),
            headless = config.headless,
            "launching isolated decoy Chromium profile"
        );

        let browser_config = build_browser_config(&config, &decoy_dir)?;
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| CoreError::Browser(format!("failed to launch Chromium: {e}")))?;

        // Pump the CDP connection: the Handler is a Stream that must be polled
        // for any command/response to flow. It ends when the browser closes.
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    tracing::debug!(
                        target: "fauxx_core::browser",
                        error = %e,
                        "CDP handler event error (browser likely closing)"
                    );
                    break;
                }
            }
        });

        Ok(Self {
            browser,
            handler_task: Some(handler_task),
            user_data_dir: decoy_dir,
            gpc_enabled: config.gpc_enabled,
            proxy_credentials,
        })
    }

    /// Whether Global Privacy Control is emitted on pages from this browser.
    pub fn gpc_enabled(&self) -> bool {
        self.gpc_enabled
    }

    /// The dedicated decoy user-data directory this browser is running against.
    pub fn user_data_dir(&self) -> &Path {
        &self.user_data_dir
    }

    /// The OS process id of the spawned Chromium child, if it is still tracked.
    /// `None` once the child has been reaped. The live integration test uses
    /// this to assert no orphan process survives shutdown.
    pub fn child_pid(&mut self) -> Option<u32> {
        self.browser.get_mut_child().and_then(|c| c.inner.id())
    }

    /// Open a fresh blank page in the decoy profile, ready to navigate.
    ///
    /// When GPC is enabled for this browser (the default, D4c #18), the page is
    /// configured to emit Global Privacy Control on every subsequent navigation:
    /// the `Sec-GPC: 1` request header (via `Network.setExtraHTTPHeaders`) and
    /// `navigator.globalPrivacyControl = true` injected before page scripts run
    /// (via `Page.addScriptToEvaluateOnNewDocument`). Both are applied to THIS
    /// isolated decoy page only, never a real authenticated session.
    pub async fn new_page(&self) -> Result<DecoyPage> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| CoreError::Browser(format!("failed to open decoy page: {e}")))?;
        // Apply proxy authentication FIRST, before any navigation, so the page's
        // network manager is ready to answer the proxy's auth challenge on the
        // very first request (C7 #30). chromiumoxide enables Fetch with
        // `handleAuthRequests` and responds with `Fetch.continueWithAuth` using
        // these credentials internally; we only hand it the secret.
        if let Some(creds) = &self.proxy_credentials {
            page.authenticate(creds.0.clone()).await.map_err(|e| {
                CoreError::Browser(format!("applying decoy proxy authentication failed: {e}"))
            })?;
        }
        let decoy = DecoyPage { page };
        if self.gpc_enabled {
            decoy.apply_gpc().await?;
        }
        Ok(decoy)
    }

    /// Build category-targeted decoy history (R2) by visiting the persona's
    /// interest sites with the persona's paced cadence.
    ///
    /// Resolves the persona's [`CategoryPool`](crate::persona::CategoryPool)
    /// interests to the curated, bundled HTTPS site set
    /// ([`categories::sites_for_persona`]) and drives this browser to visit each
    /// through the guarded navigation path. Returns a [`SeedOutcome`] recording
    /// what was visited and what was skipped (an unreachable or blocked site is
    /// a recorded skip, not a hard failure). `seed` mixes into the cadence so a
    /// fixed `(persona, seed)` pair is reproducible.
    pub async fn seed_history(&self, persona: &SyntheticPersona, seed: u64) -> Result<SeedOutcome> {
        let urls = categories::sites_for_persona(persona);
        categories::seed_history_for_persona(self, persona, &urls, seed).await
    }

    /// Close the browser cleanly: close the CDP session, kill the Chromium child
    /// process, and stop the Handler task. Idempotent; safe to call once.
    pub async fn close(mut self) -> Result<()> {
        self.shutdown().await
    }

    /// Shared shutdown path used by [`close`](Self::close) and `Drop`.
    async fn shutdown(&mut self) -> Result<()> {
        // Ask the browser to close gracefully, then force-kill to guarantee no
        // orphan child survives (close() is best-effort if the page hung).
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        let _ = self.browser.kill().await;
        if let Some(task) = self.handler_task.take() {
            // The Handler stream ends when the connection drops; the task then
            // returns. Abort defensively in case it is still parked.
            task.abort();
        }
        Ok(())
    }
}

impl Drop for DecoyBrowser {
    fn drop(&mut self) {
        // Best-effort: ensure the child is killed and the task aborted even if
        // the caller forgot to `close()`. We cannot await in Drop, so signal a
        // synchronous kill on the inner tokio child. chromiumoxide also spawns
        // the child with `kill_on_drop(true)`, so dropping the Browser reaps it
        // regardless; this just makes the intent explicit and immediate.
        if let Some(child) = self.browser.get_mut_child() {
            let _ = child.inner.start_kill();
        }
        if let Some(task) = self.handler_task.take() {
            task.abort();
        }
    }
}

/// A page in the decoy profile, drivable with persona-paced navigation, scroll,
/// and dwell. Navigation is guardrail-checked (R3): authenticated-account
/// sign-in endpoints are refused.
#[derive(Debug)]
pub struct DecoyPage {
    page: chromiumoxide::Page,
}

impl DecoyPage {
    /// Navigate to `url`, refusing (fail closed) any authenticated-account
    /// sign-in endpoint per the R3 blocklist. Resolves once the page has loaded.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        // R3 guardrail: never drive a real sign-in flow from the decoy. This
        // logs the blocked attempt locally and returns a typed error.
        isolation::ensure_navigation_allowed(url)?;

        self.page
            .goto(url)
            .await
            .map_err(|e| CoreError::Browser(format!("navigation to {url} failed: {e}")))?;
        self.page
            .wait_for_navigation()
            .await
            .map_err(|e| CoreError::Browser(format!("waiting for {url} to load failed: {e}")))?;
        Ok(())
    }

    /// Configure this page to emit Global Privacy Control (D4c #18).
    ///
    /// Two CDP commands, applied before any navigation so they take effect on
    /// the first request:
    ///
    /// - `Network.enable` then `Network.setExtraHTTPHeaders` adds the
    ///   `Sec-GPC: 1` REQUEST header to every request the page issues.
    /// - `Page.addScriptToEvaluateOnNewDocument` injects
    ///   `navigator.globalPrivacyControl = true` so it is observable to page
    ///   scripts; `runImmediately` also applies it to the current `about:blank`
    ///   document, and it re-runs on each new document before the page's own
    ///   scripts execute.
    ///
    /// This is a property-only injection on an ISOLATED decoy page; it does not
    /// navigate, so it cannot bypass the R3 navigation guardrail.
    pub(crate) async fn apply_gpc(&self) -> Result<()> {
        // Network domain must be enabled before extra headers stick.
        self.page
            .execute(NetworkEnableParams::default())
            .await
            .map_err(|e| CoreError::Browser(format!("enabling CDP Network domain failed: {e}")))?;

        // Sec-GPC: 1 on every request issued by this page.
        let headers = NetworkHeaders::new(serde_json::json!({ "Sec-GPC": "1" }));
        self.page
            .execute(SetExtraHttpHeadersParams::new(headers))
            .await
            .map_err(|e| CoreError::Browser(format!("setting Sec-GPC header failed: {e}")))?;

        // navigator.globalPrivacyControl = true, before page scripts run.
        self.page
            .execute(AddScriptToEvaluateOnNewDocumentParams {
                source: gpc::NAVIGATOR_GPC_INJECT_JS.to_string(),
                world_name: None,
                include_command_line_api: None,
                run_immediately: Some(true),
            })
            .await
            .map_err(|e| {
                CoreError::Browser(format!(
                    "injecting navigator.globalPrivacyControl failed: {e}"
                ))
            })?;

        tracing::debug!(
            target: "fauxx_core::browser",
            "applied GPC to decoy page (Sec-GPC request header + navigator.globalPrivacyControl)"
        );
        Ok(())
    }

    /// Read `navigator.globalPrivacyControl` from this page (a read-only
    /// observation; does not navigate). Returns `Some(bool)` when the property
    /// is present, `None` when it is absent (older engines, or GPC not applied).
    /// Used by the live GPC test to confirm the flag is observable to page JS.
    pub async fn read_navigator_gpc(&self) -> Result<Option<bool>> {
        let result = self
            .page
            .evaluate(
                "(() => { const v = navigator.globalPrivacyControl; \
                 return (typeof v === 'boolean') ? v : null; })()",
            )
            .await
            .map_err(|e| {
                CoreError::Browser(format!(
                    "reading navigator.globalPrivacyControl failed: {e}"
                ))
            })?;
        Ok(result.into_value::<Option<bool>>().unwrap_or(None))
    }

    /// Fetch and parse a site's `/.well-known/gpc.json` to detect whether the
    /// site advertises that it honors GPC (D4c #18).
    ///
    /// Resolves the well-known URL for `origin` (an `https://host` origin, with
    /// or without a trailing slash), fetches it through THIS page via a guarded
    /// in-page `fetch` (so it stays on the decoy profile and carries the same
    /// Sec-GPC header), and parses the body with [`parse_gpc_well_known`]. A
    /// missing file (404), a network failure, or a malformed body yields a
    /// well-formed [`GpcSupport`] with `honored == false` rather than an error:
    /// "no advertised support" is a valid observation, not a crash. The origin
    /// itself is still subject to the R3 navigation/host guardrail.
    pub async fn fetch_gpc_well_known(&self, origin: &str) -> Result<GpcSupport> {
        let url = gpc::well_known_url_for(origin);
        // The fetch target must clear the same auth-flow guardrail navigation
        // does; a well-known probe must never hit a sign-in host.
        isolation::ensure_navigation_allowed(&url)?;

        // Fetch inside the page so the request rides this decoy profile (and its
        // Sec-GPC header), returning the raw body text or null on any failure.
        // Cap the body: a `/.well-known/gpc.json` is spec'd to be tiny, so slice
        // to a few KB to stop a pathological server forcing a large string back
        // across the CDP boundary.
        // `evaluate_function` takes a FUNCTION declaration and calls it (awaiting
        // the returned promise), mirroring `BROWSING_TOPICS_READ_JS`. Pass a bare
        // `async () => {...}`, NOT an invoked IIFE: an already-invoked expression
        // evaluates to a Promise, which CDP rejects ("does not evaluate to a
        // function").
        let expr = format!(
            "async () => {{ try {{ \
                const r = await fetch({url:?}, {{ method: 'GET' }}); \
                if (!r.ok) return null; \
                const t = await r.text(); \
                return t.slice(0, 8192); \
             }} catch (e) {{ return null; }} }}",
            url = url
        );
        let raw = self
            .page
            .evaluate_function(expr)
            .await
            .map_err(|e| CoreError::Browser(format!("GPC well-known fetch failed: {e}")))?;
        let body = raw.into_value::<Option<String>>().unwrap_or(None);
        Ok(gpc::parse_gpc_well_known(body.as_deref()))
    }

    /// Scroll the page down by `pixels` (positive scrolls toward the bottom).
    /// Returns the resulting `window.scrollY`, so a headless run is observable.
    pub async fn scroll_by(&self, pixels: i64) -> Result<f64> {
        // window.scrollBy then read back scrollY: observable from a headless run.
        let expr = format!("(() => {{ window.scrollBy(0, {pixels}); return window.scrollY; }})()");
        let result = self
            .page
            .evaluate(expr)
            .await
            .map_err(|e| CoreError::Browser(format!("scroll failed: {e}")))?;
        let scroll_y = result.into_value::<f64>().unwrap_or(0.0);
        Ok(scroll_y)
    }

    /// Read the current scroll position (`window.scrollY`).
    pub async fn scroll_y(&self) -> Result<f64> {
        let result = self
            .page
            .evaluate("window.scrollY")
            .await
            .map_err(|e| CoreError::Browser(format!("read scrollY failed: {e}")))?;
        Ok(result.into_value::<f64>().unwrap_or(0.0))
    }

    /// The page's current title (observable signal that the page loaded).
    pub async fn title(&self) -> Result<String> {
        let result = self
            .page
            .evaluate("document.title")
            .await
            .map_err(|e| CoreError::Browser(format!("read title failed: {e}")))?;
        Ok(result.into_value::<String>().unwrap_or_default())
    }

    /// Run one persona-paced browsing pass on this page: scroll in steps and
    /// dwell, with the timing and depth DERIVED from `persona` (and `seed`) via
    /// [`BrowsingCadence`]. Returns the cadence that was applied so a headless
    /// run is observable/assertable. The persona has no explicit browsing-style
    /// fields yet; C5 adds them and this reads them then.
    pub async fn browse_with_persona(
        &self,
        persona: &SyntheticPersona,
        seed: u64,
    ) -> Result<BrowsingCadence> {
        let cadence = BrowsingCadence::for_persona(persona, seed);
        self.apply_cadence(cadence).await?;
        Ok(cadence)
    }

    /// Apply a fully-specified [`BrowsingCadence`]: scroll in `scroll_steps`
    /// steps with `scroll_pause` between them, then dwell for `dwell`.
    pub async fn apply_cadence(&self, cadence: BrowsingCadence) -> Result<()> {
        for _ in 0..cadence.scroll_steps {
            self.scroll_by(cadence.scroll_step_px).await?;
            sleep(cadence.scroll_pause).await;
        }
        sleep(cadence.dwell).await;
        Ok(())
    }

    /// Dwell (sleep) on the page for `duration`, modeling reading time.
    pub async fn dwell(&self, duration: Duration) -> Result<()> {
        sleep(duration).await;
        Ok(())
    }

    /// The current page HTML (a read-only observation). Does not navigate, so it
    /// cannot bypass the R3 navigation guardrail.
    pub async fn content(&self) -> Result<String> {
        self.page
            .content()
            .await
            .map_err(|e| CoreError::Browser(format!("read page content failed: {e}")))
    }

    /// Read the decoy profile's own assigned Privacy Sandbox Topics from THIS
    /// page (R2, the closed loop).
    ///
    /// This is a GUARDED read: it evaluates `topics::BROWSING_TOPICS_READ_JS`
    /// (an async function the CDP layer awaits) in the current page context and
    /// parses the result into a typed [`TopicsReadback`]. It does NOT navigate,
    /// so it cannot bypass the R3 navigation blocklist; the caller must first
    /// [`navigate`](Self::navigate) to an eligible HTTPS page (the Topics API
    /// requires the feature enabled at launch and a secure context).
    ///
    /// Robust to the epoch boundary: topics are computed per WEEKLY epoch from
    /// recent history, so a read right after seeding history commonly returns an
    /// EMPTY (but well-formed) list until the epoch rolls. That is the expected
    /// outcome inside the observation window, NOT an error: this returns a
    /// successful [`TopicsReadback`] with `topics.is_empty()` true and
    /// `available` reflecting whether the API was callable at all.
    pub async fn read_topics(&self) -> Result<TopicsReadback> {
        let raw = self
            .page
            .evaluate_function(topics::BROWSING_TOPICS_READ_JS)
            .await
            .map_err(|e| CoreError::Browser(format!("Topics read failed: {e}")))?;
        let value = raw
            .into_value::<serde_json::Value>()
            .unwrap_or(serde_json::Value::Null);
        topics::parse_topics_payload(&value)
    }
}

/// Comma-joined Chromium feature names enabled for the Topics read-back flow
/// (R2), passed as a single `--enable-features=...` arg. `BrowsingTopics` is the
/// Topics API itself; `PrivacySandboxAdsAPIsOverride` and
/// `OverridePrivacySandboxSettingsLocalTesting` force the Privacy Sandbox
/// settings on for a throwaway profile that has not gone through the consent UI,
/// which is exactly what an isolated decoy needs to exercise the API locally.
const TOPICS_ENABLE_FEATURES: &str = "BrowsingTopics,\
PrivacySandboxAdsAPIsOverride,\
OverridePrivacySandboxSettingsLocalTesting";

/// Build the chromiumoxide [`BrowserConfig`] for an isolated decoy launch.
///
/// Pins the dedicated `--user-data-dir`, the system executable, headless mode,
/// and hardening flags. `--no-first-run`/`--no-default-browser-check` keep the
/// throwaway profile quiet; `--disable-sync`/`--disable-background-networking`
/// keep it from phoning home. `--no-sandbox` is added ONLY when the config opts
/// in (default off): the decoy loads arbitrary content, so the OS sandbox stays
/// on unless the host cannot run it.
///
/// When `topics_enabled` is set (R2), the Privacy Sandbox / Topics feature flags
/// ([`TOPICS_ENABLE_FEATURES`]) are added so `document.browsingTopics()` is
/// callable on an eligible HTTPS page. The default launch leaves these OFF.
fn build_browser_config(config: &BrowserLaunchConfig, decoy_dir: &Path) -> Result<BrowserConfig> {
    let mut builder = BrowserConfig::builder()
        .chrome_executable(&config.executable)
        .user_data_dir(decoy_dir)
        // The Handler that pumps CDP runs on the tokio runtime; matching the
        // request-timeout to a sane bound keeps a stuck command from hanging.
        .request_timeout(Duration::from_secs(30))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-sync")
        .arg("--disable-background-networking");

    if config.no_sandbox {
        builder = builder.arg("--no-sandbox");
    }

    if config.topics_enabled {
        // Enable the Privacy Sandbox Ads APIs (incl. the Topics API) for the R2
        // read-back flow. Behind the config toggle so the default decoy launch
        // stays unchanged.
        builder = builder.arg(format!("--enable-features={TOPICS_ENABLE_FEATURES}"));
    }

    // Per-persona network policy (C7 #30 N1 / #31 N2): the egress
    // `--proxy-server` arg (Tor -> socks5://127.0.0.1:9050) and the secure-DNS
    // flags (DoH/DoT). Direct + SystemDefault emit nothing, so the standard
    // launch is unchanged. Both apply to this isolated decoy profile only, so a
    // persona's lookups and traffic share one observer.
    for arg in config.network.chromium_args() {
        builder = builder.arg(arg);
    }

    if !config.headless {
        builder = builder.with_head();
    }

    builder
        .build()
        .map_err(|e| CoreError::Browser(format!("invalid browser config: {e}")))
}

/// Sleep helper that no-ops on a zero duration (keeps tests that pass tiny or
/// zero cadences from blocking). Centralized so the timing source is one place.
async fn sleep(duration: Duration) {
    if !duration.is_zero() {
        tokio::time::sleep(duration).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_credentials_are_redacted_in_debug() {
        // The proxy secret must never leak through `Debug` (logs, panics). The
        // wrapper redacts both fields even though chromiumoxide's own
        // `Credentials` derives a plaintext `Debug`.
        let user = "egress-user";
        let pass = "sup3r-s3cret-passphrase";
        let creds = RedactedCredentials::new(user, pass);
        let shown = format!("{creds:?}");
        assert!(!shown.contains(user), "username leaked in Debug: {shown}");
        assert!(!shown.contains(pass), "password leaked in Debug: {shown}");
        assert!(shown.contains("redacted"));
        // The inner credentials are still the real values (for chromiumoxide).
        assert_eq!(creds.0.username, user);
        assert_eq!(creds.0.password, pass);
    }

    #[test]
    fn decoy_dir_is_under_app_data_and_never_a_real_path() -> Result<()> {
        let root = decoy_profiles_root()?;
        let dir = decoy_dir_for("persona-abc")?;
        // Nested under the dedicated decoy-profiles root...
        assert!(dir.starts_with(&root));
        assert!(dir.ends_with("persona-abc"));
        // ...and the root itself is verifiably isolated from the OS-detected
        // real browser profiles (the guardrail accepts it).
        let roots = isolation::known_real_profile_roots();
        assert!(isolation::ensure_isolated_from_real_profiles(&dir, &roots).is_ok());
        Ok(())
    }

    #[test]
    fn decoy_id_is_sanitized_to_a_single_safe_segment() -> Result<()> {
        // A crafted id must not climb out of the decoy-profiles root.
        let root = decoy_profiles_root()?;
        let dir = decoy_dir_for("../../etc/passwd")?;
        assert!(
            dir.starts_with(&root),
            "sanitized id must stay under the root: {dir:?}"
        );
        // Exactly one segment was appended to the root.
        assert_eq!(dir.components().count(), root.components().count() + 1);
        Ok(())
    }

    #[test]
    fn empty_id_falls_back_to_a_default_segment() {
        assert_eq!(sanitize_id(""), "decoy");
        assert_eq!(sanitize_id("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_id("ok-id_1"), "ok-id_1");
    }

    #[test]
    fn launch_config_defaults_to_system_chromium_headless() {
        let cfg = BrowserLaunchConfig::new();
        assert_eq!(cfg.executable(), Path::new(DEFAULT_CHROMIUM_PATH));
        assert!(cfg.headless);
    }

    #[test]
    fn topics_flow_is_opt_in_and_leaves_default_launch_unchanged() {
        // Default decoy launch does NOT enable the Privacy Sandbox Topics flow.
        let default_cfg = BrowserLaunchConfig::new();
        assert!(!default_cfg.topics_enabled());
        // Opting in flips only the toggle; the rest of the config is untouched.
        let topics_cfg = BrowserLaunchConfig::new().with_topics_enabled(true);
        assert!(topics_cfg.topics_enabled());
        assert!(topics_cfg.headless);
        assert_eq!(topics_cfg.executable(), Path::new(DEFAULT_CHROMIUM_PATH));
    }

    #[test]
    fn gpc_is_default_on_and_toggleable() {
        // GPC is a signal we WANT to emit, so the default decoy launch has it on.
        let default_cfg = BrowserLaunchConfig::new();
        assert!(default_cfg.gpc_enabled());
        // The toggle flips only GPC; the rest of the config is untouched.
        let off = BrowserLaunchConfig::new().with_gpc_enabled(false);
        assert!(!off.gpc_enabled());
        assert!(off.headless);
        assert_eq!(off.executable(), Path::new(DEFAULT_CHROMIUM_PATH));
        // Topics and GPC toggles are independent.
        let both = BrowserLaunchConfig::new()
            .with_topics_enabled(true)
            .with_gpc_enabled(false);
        assert!(both.topics_enabled());
        assert!(!both.gpc_enabled());
    }

    #[test]
    fn network_is_default_direct_and_emits_no_args() {
        // The standard launch is unchanged: Direct egress + SystemDefault DNS
        // emit no proxy or DNS flags.
        let cfg = BrowserLaunchConfig::new();
        assert!(cfg.network_chromium_args().is_empty());
    }

    #[test]
    fn binding_egress_emits_proxy_server_arg() {
        use crate::network::{DnsStrategy, Egress};
        // SOCKS proxy.
        let cfg = BrowserLaunchConfig::new().with_egress(Egress::socks_proxy("10.0.0.2", 1080));
        assert!(cfg
            .network_chromium_args()
            .contains(&"--proxy-server=socks5://10.0.0.2:1080".to_string()));
        // Tor maps to the default local SOCKS5 listener.
        let tor = BrowserLaunchConfig::new().with_egress(Egress::tor());
        assert!(tor
            .network_chromium_args()
            .contains(&"--proxy-server=socks5://127.0.0.1:9050".to_string()));
        // HTTP proxy.
        let http =
            BrowserLaunchConfig::new().with_egress(Egress::http_proxy("proxy.example", 8080));
        assert!(http
            .network_chromium_args()
            .contains(&"--proxy-server=http://proxy.example:8080".to_string()));
        // DNS untouched stays empty.
        assert_eq!(http.network().dns, DnsStrategy::SystemDefault);
    }

    #[test]
    fn binding_dns_emits_secure_dns_args() {
        use crate::network::DnsStrategy;
        let cfg =
            BrowserLaunchConfig::new().with_dns(DnsStrategy::doh("https://dns.example/dns-query"));
        let args = cfg.network_chromium_args();
        assert!(args.contains(&"--enable-features=DnsOverHttps".to_string()));
        assert!(
            args.contains(&"--dns-over-https-templates=https://dns.example/dns-query".to_string())
        );
    }

    #[tokio::test]
    async fn launch_refuses_when_decoy_dir_overlaps_a_real_profile() {
        // Inject a "real" root that the configured decoy dir nests inside; the
        // launcher must fail closed BEFORE attempting any browser launch.
        let real = PathBuf::from("/home/u/.config/google-chrome");
        let decoy = PathBuf::from("/home/u/.config/google-chrome/Default");
        let cfg = BrowserLaunchConfig::new()
            .with_user_data_dir(&decoy)
            .with_real_profile_roots(vec![real]);
        let result = DecoyBrowser::launch_with("test", cfg).await;
        assert!(matches!(result, Err(CoreError::Browser(_))));
    }
}
