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

//! The user-initiated update check.
//!
//! # The rule this module exists to keep
//!
//! Fauxx tells users it does not phone home. That claim is worth more than the
//! convenience of an automatic update check, so there is no timer, no
//! background poll, and no check at startup, at shutdown, or on any schedule.
//! [`check_for_update`] runs **only** when a person presses the button, and it
//! is the only function here that opens a socket.
//!
//! If you are adding a caller: a caller that is not a direct response to a user
//! action is a bug, not a feature, and it silently converts a privacy promise
//! into a false statement.
//!
//! # What a check discloses, exactly
//!
//! One HTTPS GET to the GitHub releases API. GitHub therefore learns, at the
//! moment the user asks: the requesting IP address, and the User-Agent, which
//! is [`USER_AGENT`] and carries the app name and version and nothing else. No
//! machine identifier, no OS string, no install id, no persona data of any kind
//! is sent, and no cookie is stored or returned.
//!
//! # Why it does not use the persona's egress
//!
//! Personas can be bound to a proxy, VPN, or Tor exit (C7 #30). This request
//! deliberately ignores all of that and goes out over the ordinary system
//! route. Two reasons, and both matter:
//!
//! - Sending app traffic through a persona's exit would put a real, identifying
//!   request (this app, this version) on the same exit as that persona's decoy
//!   browsing, which is precisely the correlation the per-persona egress exists
//!   to prevent.
//! - The check is about the machine, not about any persona. It is not decoy
//!   traffic, and it must never be counted or shaped as if it were.
//!
//! "Ignores" is enforced, not assumed. The client is built with `.no_proxy()`,
//! which also switches off reqwest's default environment-proxy pickup, so an
//! exported `ALL_PROXY` / `HTTP_PROXY` / `HTTPS_PROXY` does NOT capture this
//! request. That matters here more than it would in most apps: a privacy-minded
//! user plausibly has `ALL_PROXY` pointed at the very same local Tor SOCKS port
//! one of their personas egresses through, and inheriting it would produce
//! exactly the correlation described above.
//!
//! What remains is genuine network-layer routing (a system VPN, a transparent
//! proxy), which is outside any application's control and is visible to the
//! user who configured it. A user who wants this request to take a particular
//! route should arrange it there.
//!
//! # TLS
//!
//! rustls with the `ring` provider, verifying against the platform trust store
//! (`rustls-platform-verifier`), so an enterprise or proxy root the user has
//! actually installed is honoured. The provider is installed lazily on the first
//! check, so a user who never presses the button never initializes a TLS stack.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{CoreError, Result};

/// The releases page a user is sent to in order to actually download an update.
///
/// The app does not download or install anything itself: it reports what the
/// latest release is and hands off to the browser. Installing over the top of a
/// running privacy tool is a meaningfully larger surface than reading a version
/// string, and it is not what was asked for.
pub const RELEASES_URL: &str = "https://github.com/digital-grease/fauxx-desktop/releases/latest";

/// The GitHub API endpoint queried by [`check_for_update`].
///
/// `/releases/latest` excludes drafts and pre-releases, so an `rc` tag is never
/// offered to someone on a stable build.
const API_URL: &str = "https://api.github.com/repos/digital-grease/fauxx-desktop/releases/latest";

/// The exact User-Agent sent. GitHub rejects API requests without one.
///
/// Deliberately just the app and its version: no OS, no architecture, no
/// machine or install identifier. See the module docs.
pub const USER_AGENT: &str = concat!("fauxx-desktop/", env!("CARGO_PKG_VERSION"));

/// How long to wait before giving up, so a hung network cannot wedge the UI.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Installs the rustls crypto provider exactly once, on first use.
///
/// reqwest is built with `rustls-no-provider` (see the workspace manifest: the
/// bundled-provider feature would pull aws-lc-rs and cmake, a second C build on
/// top of the OpenSSL already vendored for SQLCipher), so the process must
/// install one itself before any TLS client is built.
///
/// Doing it lazily, here, rather than at startup is deliberate: a user who
/// never presses the button never initializes a TLS stack at all.
fn ensure_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        // `install_default` returns Err if a provider is already installed,
        // which is a benign race with any other caller, not a failure.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// How this build compares to the latest published release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    /// This build is the latest published release.
    UpToDate,
    /// A newer release is published.
    UpdateAvailable,
    /// This build is newer than the latest published release (a development or
    /// pre-release build). Not an error, and not something to nag about.
    Newer,
}

/// The result of one update check.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    /// The version of this build.
    pub current: String,
    /// The latest published release version, as a plain version string with any
    /// leading `v` stripped.
    pub latest: String,
    /// How [`current`](Self::current) compares to [`latest`](Self::latest).
    pub status: UpdateStatus,
    /// The page to send the user to in order to download it.
    pub release_url: String,
}

impl UpdateCheck {
    /// A short, plain-language line suitable for showing directly in a UI.
    pub fn summary(&self) -> String {
        match self.status {
            UpdateStatus::UpToDate => {
                format!("You are on the latest release ({}).", self.current)
            }
            UpdateStatus::UpdateAvailable => {
                format!(
                    "Version {} is available. You have {}.",
                    self.latest, self.current
                )
            }
            UpdateStatus::Newer => format!(
                "You are running {}, which is newer than the latest release ({}).",
                self.current, self.latest
            ),
        }
    }
}

/// The subset of the GitHub release object we read. Everything else in the
/// response is ignored rather than deserialized, so an unrelated API change
/// cannot break the check.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
}

/// Check the latest published release. **Call only in direct response to a user
/// action**; see the module docs for why that is a hard rule.
///
/// Returns [`CoreError::Network`] when the check cannot be completed (offline,
/// timed out, rate-limited, unexpected response). The caller shows the message;
/// a failed check is a normal outcome, not a fault.
pub async fn check_for_update() -> Result<UpdateCheck> {
    check_against(API_URL).await
}

/// The check, parameterized over the endpoint so tests can point it at a local
/// server instead of the real API.
async fn check_against(url: &str) -> Result<UpdateCheck> {
    // A fresh client per check, with cookies and proxies left off: nothing about
    // this request should persist, and it must not inherit a persona's egress
    // (see the module docs).
    ensure_crypto_provider();

    let client = reqwest::Client::builder()
        // `.no_proxy()` is load-bearing, not tidiness. reqwest defaults
        // `auto_sys_proxy` to TRUE, and the ENVIRONMENT half of that lookup is
        // NOT behind the `system-proxy` feature this crate leaves disabled:
        // hyper-util's `Builder::from_env` reads ALL_PROXY / HTTP_PROXY /
        // HTTPS_PROXY unconditionally, and only the macOS/Windows OS-settings
        // lookup is feature-gated.
        //
        // Without this call, a user who exports `ALL_PROXY=socks5://127.0.0.1:9050`
        // for Tor — precisely the kind of user this app is for, and quite
        // possibly the SAME endpoint one of their personas egresses through —
        // would have this app-identifying request tunnelled out of that exit,
        // correlating a real request for "fauxx-desktop/<version>" with that
        // persona's decoy browsing. That is the exact correlation per-persona
        // egress exists to prevent, and the one the module docs promise cannot
        // happen. It also makes the check fail outright when the env proxy is a
        // SOCKS URL, because the `socks` feature is not enabled either.
        .no_proxy()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        // A redirect is not expected from this endpoint. Allowing an unbounded
        // chain would let a hijacked response walk the request to another host,
        // so cap it rather than trusting it.
        .redirect(reqwest::redirect::Policy::limited(2))
        .build()
        .map_err(|e| CoreError::Network(format!("could not build the update-check client: {e}")))?;

    let response = client.get(url).send().await.map_err(|e| {
        // The common case here is simply being offline. Say that, rather than
        // surfacing a transport error the user cannot act on.
        if e.is_timeout() {
            CoreError::Network(
                "the update check timed out. Check your connection and try again.".to_string(),
            )
        } else if e.is_connect() {
            CoreError::Network(
                "could not reach GitHub to check for updates. Are you online?".to_string(),
            )
        } else {
            CoreError::Network(format!("the update check failed: {e}"))
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        // 403/429 from this endpoint is nearly always the unauthenticated rate
        // limit (60/hour per IP), which is a wait-and-retry, not a failure the
        // user should debug.
        let message = if status.as_u16() == 403 || status.as_u16() == 429 {
            "GitHub is rate-limiting update checks from this network. Try again later.".to_string()
        } else {
            format!("GitHub returned {status} for the update check.")
        };
        return Err(CoreError::Network(message));
    }

    let body = response.text().await.map_err(|e| {
        CoreError::Network(format!("could not read the update-check response: {e}"))
    })?;
    let release: GithubRelease = serde_json::from_str(&body)
        .map_err(|e| CoreError::Network(format!("unexpected update-check response: {e}")))?;

    build_check(crate::VERSION, &release)
}

/// Compare this build against a release object. Split out so the comparison is
/// unit-tested without any network.
fn build_check(current_version: &str, release: &GithubRelease) -> Result<UpdateCheck> {
    let latest_raw = normalize_tag(&release.tag_name);
    let latest = semver::Version::parse(latest_raw).map_err(|e| {
        CoreError::Network(format!(
            "could not read the latest release version {:?}: {e}",
            release.tag_name
        ))
    })?;
    let current = semver::Version::parse(current_version).map_err(|e| {
        CoreError::Network(format!(
            "could not read this build's version {current_version:?}: {e}"
        ))
    })?;

    // semver ordering, not string ordering: it is what gets `0.3.0` > `0.10.0`
    // wrong, and what gets `0.1.0-rc.8` < `0.1.0` right. This project ships rc
    // tags, so both cases are real here.
    let status = match current.cmp(&latest) {
        std::cmp::Ordering::Less => UpdateStatus::UpdateAvailable,
        std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
        std::cmp::Ordering::Greater => UpdateStatus::Newer,
    };

    Ok(UpdateCheck {
        current: current.to_string(),
        latest: latest.to_string(),
        status,
        release_url: release
            .html_url
            .clone()
            .unwrap_or_else(|| RELEASES_URL.to_string()),
    })
}

/// Strip the `v` prefix this project's tags carry (`v0.3.0` -> `0.3.0`).
fn normalize_tag(tag: &str) -> &str {
    tag.trim().strip_prefix('v').unwrap_or(tag.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_string(),
            html_url: None,
        }
    }

    fn status_of(current: &str, tag: &str) -> UpdateStatus {
        match build_check(current, &release(tag)) {
            Ok(check) => check.status,
            Err(e) => panic!("{current} vs {tag} must compare: {e}"),
        }
    }

    #[test]
    fn the_v_prefix_on_a_tag_is_not_part_of_the_version() {
        assert_eq!(normalize_tag("v0.3.0"), "0.3.0");
        assert_eq!(normalize_tag("0.3.0"), "0.3.0");
        assert_eq!(normalize_tag("  v1.2.3  "), "1.2.3");
    }

    #[test]
    fn an_older_build_is_offered_the_update() {
        assert_eq!(status_of("0.2.1", "v0.3.0"), UpdateStatus::UpdateAvailable);
    }

    #[test]
    fn the_current_release_reports_up_to_date() {
        assert_eq!(status_of("0.3.0", "v0.3.0"), UpdateStatus::UpToDate);
    }

    #[test]
    fn a_development_build_ahead_of_the_release_is_not_nagged() {
        assert_eq!(status_of("0.4.0", "v0.3.0"), UpdateStatus::Newer);
    }

    /// String comparison says "0.3.0" > "0.10.0". Semver says otherwise, and
    /// this project will reach 0.10 eventually.
    #[test]
    fn comparison_is_numeric_not_lexicographic() {
        assert_eq!(status_of("0.3.0", "v0.10.0"), UpdateStatus::UpdateAvailable);
        assert_eq!(status_of("0.10.0", "v0.3.0"), UpdateStatus::Newer);
    }

    /// This project has shipped `v0.1.0-rc.7` / `rc.8`. A pre-release sorts
    /// BELOW its own release, so someone on an rc must be offered the stable.
    #[test]
    fn a_pre_release_build_is_offered_the_stable_release() {
        assert_eq!(
            status_of("0.1.0-rc.8", "v0.1.0"),
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(
            status_of("0.1.0-rc.7", "v0.1.0-rc.8"),
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(status_of("0.1.0", "v0.1.0-rc.8"), UpdateStatus::Newer);
    }

    #[test]
    fn an_unparseable_tag_is_an_error_not_a_wrong_answer() {
        // Reporting "up to date" because we could not read the tag would be the
        // worst possible failure mode: silently wrong, in the reassuring direction.
        assert!(build_check("0.3.0", &release("not-a-version")).is_err());
        assert!(build_check("0.3.0", &release("")).is_err());
    }

    #[test]
    fn the_release_url_falls_back_to_the_releases_page() {
        let check = match build_check("0.3.0", &release("v0.3.0")) {
            Ok(c) => c,
            Err(e) => panic!("must build: {e}"),
        };
        assert_eq!(check.release_url, RELEASES_URL);
    }

    #[test]
    fn the_release_url_is_used_when_the_api_supplies_one() {
        let mut r = release("v0.4.0");
        r.html_url =
            Some("https://github.com/digital-grease/fauxx-desktop/releases/tag/v0.4.0".into());
        let check = match build_check("0.3.0", &r) {
            Ok(c) => c,
            Err(e) => panic!("must build: {e}"),
        };
        assert!(check.release_url.ends_with("/tag/v0.4.0"));
    }

    /// The User-Agent is the entire content of what we disclose beyond the IP.
    /// Pin it so a well-meaning edit cannot start leaking host details.
    #[test]
    fn the_user_agent_carries_only_the_app_and_version() {
        assert!(USER_AGENT.starts_with("fauxx-desktop/"));
        assert!(USER_AGENT.contains(crate::VERSION));
        for leak in ["linux", "windows", "macos", "x86", "aarch", "(", ";"] {
            assert!(
                !USER_AGENT.to_ascii_lowercase().contains(leak),
                "User-Agent must not disclose host details, found {leak:?} in {USER_AGENT:?}"
            );
        }
    }

    /// Being offline is the ordinary failure, not an exceptional one, so it has
    /// to produce a sentence a user can act on rather than a transport dump.
    /// Uses a closed local port, so the test needs no external network.
    #[tokio::test]
    async fn an_unreachable_endpoint_reports_a_connection_problem_in_plain_words() {
        let Err(err) = check_against("http://127.0.0.1:1/releases/latest").await else {
            panic!("a closed port must not report a successful check");
        };
        let message = err.to_string();
        assert!(
            message.contains("Are you online?") || message.contains("timed out"),
            "the failure must read as a connection problem, got {message:?}"
        );
        // And it must never imply the user is up to date.
        assert!(
            !message.to_ascii_lowercase().contains("up to date"),
            "a failed check must never read as success: {message:?}"
        );
    }

    /// The env-proxy finding from the adversarial sweep. A proxy set in the
    /// environment must NOT capture this request: it would put an
    /// app-identifying call on the same exit as a persona's decoy traffic.
    ///
    /// Asserted behaviourally: point the check at a closed local port with a
    /// deliberately unroutable proxy exported, and the failure must be the
    /// CONNECTION to that local port, not a proxy error. If `.no_proxy()` were
    /// removed, reqwest would dial the proxy instead and the error would name
    /// it. The env vars are set and cleared around a single-threaded body since
    /// they are process-global.
    #[tokio::test]
    async fn an_environment_proxy_does_not_capture_the_update_check() {
        // SAFETY-ish: process-global mutation, restricted to this test's scope.
        // Both vars are removed again before returning.
        std::env::set_var("ALL_PROXY", "socks5://127.0.0.1:9");
        std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");

        let outcome = check_against("http://127.0.0.1:1/releases/latest").await;

        std::env::remove_var("ALL_PROXY");
        std::env::remove_var("HTTPS_PROXY");

        let Err(err) = outcome else {
            panic!("a closed port must not report a successful check");
        };
        let message = err.to_string().to_ascii_lowercase();
        assert!(
            !message.contains("socks") && !message.contains("proxy"),
            "the update check must ignore an environment proxy, but the failure \
             mentions one: {message:?}"
        );
    }

    #[test]
    fn summaries_name_both_versions_so_the_user_can_act() {
        let check = match build_check("0.2.1", &release("v0.3.0")) {
            Ok(c) => c,
            Err(e) => panic!("must build: {e}"),
        };
        let summary = check.summary();
        assert!(summary.contains("0.3.0"), "{summary}");
        assert!(summary.contains("0.2.1"), "{summary}");
    }
}
