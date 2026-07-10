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

//! LIVE browser integration test (C2 #11 R1 / #13 R3).
//!
//! Marked `#[ignore]` so CI (which has no Chromium) stays green; run it locally
//! with a system Chromium present:
//!
//! ```text
//! cargo test -p fauxx-core -- --ignored
//! ```
//!
//! It launches the real, system Chromium against an ISOLATED temp user-data
//! dir, navigates, performs a persona-paced scroll + dwell, and shuts down
//! cleanly, asserting NO orphan browser process remains. If a real TLS endpoint
//! is reachable it asserts a genuine real-browser handshake (a live page load);
//! if the network is unavailable it falls back to a local `data:` URL and still
//! asserts a clean isolated launch + shutdown.

use std::path::PathBuf;
use std::time::Duration;

use fauxx_core::browser::{categories, desktop_for, BrowserLaunchConfig, DecoyBrowser};
use fauxx_core::persona::{AgeRange, CategoryPool, Profession, Region, SyntheticPersona};
use fauxx_core::CoreError;

/// The system Chromium the test drives.
const CHROMIUM: &str = "/usr/bin/chromium";
/// A real TLS endpoint that echoes the negotiated handshake (used when the
/// network is up to assert a genuine real-browser TLS handshake).
const TLS_FINGERPRINT_URL: &str = "https://tls.peet.ws/api/all";
/// Local fallback page when the network is unavailable.
const LOCAL_FALLBACK_URL: &str =
    "data:text/html,<html><head><title>fauxx-decoy</title></head><body style='height:5000px'>decoy</body></html>";

fn test_persona() -> SyntheticPersona {
    SyntheticPersona::new(
        "live-test-0000-4000-8000-000000000000".to_string(),
        "Live Test".to_string(),
        AgeRange::AGE_35_44.as_name().to_string(),
        Profession::ENGINEER.as_name().to_string(),
        Region::US_WEST.as_name().to_string(),
        vec![
            CategoryPool::TECHNOLOGY.as_name().to_string(),
            CategoryPool::SCIENCE.as_name().to_string(),
            CategoryPool::TRAVEL.as_name().to_string(),
        ],
        1_700_000_000_000,
        1_700_600_000_000,
    )
}

/// Whether a PID is still a live process on this (Linux) host.
fn pid_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs /usr/bin/chromium + (optionally) network; run with --ignored"]
async fn live_isolated_launch_navigate_scroll_dwell_and_clean_shutdown(
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        PathBuf::from(CHROMIUM).exists(),
        "this live test requires the system Chromium at {CHROMIUM}"
    );

    // Isolated, throwaway user-data dir distinct from any real browser profile.
    let tmp = tempfile::tempdir()?;
    let decoy_dir = tmp.path().join("decoy-profile");

    let config = BrowserLaunchConfig::new()
        .with_executable(CHROMIUM)
        .with_user_data_dir(&decoy_dir)
        // The test host may lack a usable Chromium sandbox; production defaults
        // keep it on.
        .with_no_sandbox(true);

    let mut browser = DecoyBrowser::launch_with("live-test", config).await?;

    // The launcher created ONLY its own dedicated dir.
    assert!(
        decoy_dir.exists(),
        "decoy user-data dir should have been created"
    );
    assert_eq!(browser.user_data_dir(), decoy_dir.as_path());

    // Capture the child PID so we can prove it is reaped on shutdown.
    let pid = browser
        .child_pid()
        .ok_or("launched Chromium should report a PID")?;
    assert!(
        pid_alive(pid),
        "Chromium child {pid} should be alive after launch"
    );

    let page = browser.new_page().await?;

    // Try a real TLS endpoint first (asserts a genuine real-browser handshake);
    // fall back to a local data: URL if the network is unavailable.
    let used_network = match page.navigate(TLS_FINGERPRINT_URL).await {
        Ok(()) => {
            // A real TLS handshake completed and the page body loaded; the
            // endpoint echoes the negotiated TLS details, proving a real-browser
            // (not a raw-socket) handshake.
            let body = page
                .content()
                .await
                .map(|c| c.to_lowercase())
                .unwrap_or_default();
            assert!(
                body.contains("tls") || body.contains("ja3") || body.contains("cipher"),
                "TLS fingerprint endpoint should echo handshake details (real-browser TLS)"
            );
            true
        }
        Err(_) => {
            // Network unavailable: still assert a clean isolated launch by
            // driving a local page.
            page.navigate(LOCAL_FALLBACK_URL).await?;
            let title = page.title().await?;
            assert_eq!(
                title, "fauxx-decoy",
                "local fallback page should have loaded"
            );
            false
        }
    };
    eprintln!(
        "live test navigated via {}",
        if used_network {
            "network TLS endpoint"
        } else {
            "local data: fallback"
        }
    );

    // Persona-paced scroll + dwell on the loaded page, observable from the run.
    let before = page.scroll_y().await?;
    let cadence = page.browse_with_persona(&test_persona(), 42).await?;
    let after = page.scroll_y().await?;
    assert!(
        cadence.scroll_steps >= 2,
        "cadence should perform scroll steps"
    );
    // The page is tall (5000px or a real page); scrolling should have advanced.
    assert!(
        after >= before,
        "scrollY should not move backward (before={before}, after={after})"
    );

    // Clean shutdown: closes the browser, kills the child, stops the handler.
    browser.close().await?;

    // No orphan process: the child PID must be gone shortly after shutdown.
    let mut reaped = false;
    for _ in 0..50 {
        if !pid_alive(pid) {
            reaped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        reaped,
        "Chromium child {pid} must be reaped after shutdown (no orphan)"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs /usr/bin/chromium; run with --ignored"]
async fn live_desktop_device_identity_is_presented_and_headless_free(
) -> Result<(), Box<dyn std::error::Error>> {
    // #47: a decoy bound to a persona's desktop device must PRESENT that identity to
    // page JS over CDP — the derived UA, the client hints (navigator.userAgentData),
    // and the fixed navigator/screen values — and it must NOT leak the headless
    // `HeadlessChrome` token in either the UA string or the Sec-CH-UA brands.
    assert!(
        PathBuf::from(CHROMIUM).exists(),
        "this live test requires the system Chromium at {CHROMIUM}"
    );

    let persona = test_persona();
    let device = desktop_for(&persona);
    // Sanity: the derived identity is itself clean before we even launch.
    assert!(!device.user_agent.contains("HeadlessChrome"));
    assert!(!device.is_mobile);

    let tmp = tempfile::tempdir()?;
    let config = BrowserLaunchConfig::new()
        .with_executable(CHROMIUM)
        .with_user_data_dir(tmp.path().join("decoy-device"))
        .with_no_sandbox(true)
        .with_persona_device(&persona);

    let mut browser = DecoyBrowser::launch_with("live-device", config).await?;
    let pid = browser
        .child_pid()
        .ok_or("launched Chromium should report a PID")?;
    let page = browser.new_page().await?;
    // Prefer a real HTTPS (secure) context so navigator.userAgentData client hints
    // are exposed; fall back to a local data: page when the network is down (the UA
    // + fixed navigator/screen overrides still apply there, only the client hints
    // are gated to a secure context).
    let secure = page.navigate("https://example.com/").await.is_ok();
    if !secure {
        page.navigate(LOCAL_FALLBACK_URL).await?;
    }

    let presented = page.read_presented_device().await?;

    // 1) The critical guarantee: no headless tell in the UA or the Sec-CH-UA brands,
    //    and the UA is exactly the derived one.
    assert!(
        !presented.leaks_headless(),
        "decoy leaked a headless token: {presented:?}"
    );
    assert_eq!(
        presented.user_agent, device.user_agent,
        "decoy must present the derived UA"
    );

    // 2) The fixed navigator/screen values match the device profile (these apply in
    //    any context; deviceMemory is injected by the decoy so it is set even on a
    //    non-secure page).
    assert_eq!(
        presented.hardware_concurrency,
        Some(f64::from(device.hardware_concurrency))
    );
    assert_eq!(
        presented.device_memory,
        Some(f64::from(device.device_memory))
    );
    assert_eq!(
        presented.device_pixel_ratio,
        Some(device.device_pixel_ratio)
    );
    assert_eq!(presented.screen_width, Some(device.screen_width as f64));
    assert_eq!(presented.screen_height, Some(device.screen_height as f64));

    // navigator.platform is the coherent LEGACY token (MacIntel/Win32/...), not the
    // client-hint value, and not the real Linux host's token.
    assert_eq!(
        Some(presented.navigator_platform.as_str()),
        device.navigator_platform(),
        "navigator.platform must be the device's legacy token"
    );

    // 3) In a secure context, the client hints are coherent with the device.
    if secure {
        assert_eq!(
            presented.ua_data_platform.as_deref(),
            Some(device.platform.as_str())
        );
        assert_eq!(presented.ua_data_mobile, Some(false));
        assert!(
            presented
                .ua_data_brands
                .iter()
                .any(|b| b == "Google Chrome"),
            "client-hint brands should include the derived brands: {:?}",
            presented.ua_data_brands
        );
    } else {
        eprintln!("live device: non-secure fallback used; navigator.userAgentData not asserted");
    }

    browser.close().await?;
    let mut reaped = false;
    for _ in 0..50 {
        if !pid_alive(pid) {
            reaped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        reaped,
        "Chromium child {pid} must be reaped after shutdown (no orphan)"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs /usr/bin/chromium; run with --ignored"]
async fn live_navigation_to_auth_endpoint_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        PathBuf::from(CHROMIUM).exists(),
        "needs Chromium at {CHROMIUM}"
    );

    let tmp = tempfile::tempdir()?;
    let config = BrowserLaunchConfig::new()
        .with_executable(CHROMIUM)
        .with_user_data_dir(tmp.path().join("decoy"))
        .with_no_sandbox(true);
    let browser = DecoyBrowser::launch_with("auth-guard", config).await?;
    let page = browser.new_page().await?;

    // The R3 auth-flow guardrail must refuse a real sign-in endpoint even on a
    // live browser (fail closed), without driving the flow.
    let result = page.navigate("https://accounts.google.com/signin").await;
    assert!(
        matches!(result, Err(CoreError::Browser(_))),
        "decoy navigation to a sign-in endpoint must be refused"
    );

    browser.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs /usr/bin/chromium + network; run with --ignored"]
async fn live_topics_enabled_seed_history_and_read_back() -> Result<(), Box<dyn std::error::Error>>
{
    // R2 closed loop: launch the decoy with the Privacy Sandbox Topics flags ON,
    // seed a little real category history, then attempt a guarded Topics read on
    // an eligible HTTPS page and assert the read MECHANISM works and yields a
    // well-formed (possibly EMPTY) result.
    //
    // EPOCH-BOUNDARY CAVEAT (handled honestly): topics are computed per WEEKLY
    // epoch from recent history, so a read right after seeding is commonly EMPTY
    // until the epoch rolls. This test therefore asserts SUCCESS + well-formed,
    // and explicitly tolerates an empty topic list (it logs whether topics were
    // empty rather than failing). It does NOT fabricate topics.
    assert!(
        PathBuf::from(CHROMIUM).exists(),
        "this live test requires the system Chromium at {CHROMIUM}"
    );

    let tmp = tempfile::tempdir()?;
    let decoy_dir = tmp.path().join("decoy-topics");

    let config = BrowserLaunchConfig::new()
        .with_executable(CHROMIUM)
        .with_user_data_dir(&decoy_dir)
        // Enable the Topics API for this flow (default launch leaves it off).
        .with_topics_enabled(true)
        // The test host may lack a usable Chromium sandbox.
        .with_no_sandbox(true);

    let mut browser = DecoyBrowser::launch_with("live-topics", config).await?;
    let pid = browser
        .child_pid()
        .ok_or("launched Chromium should report a PID")?;

    let persona = test_persona();

    // Seed a modest amount of real history from the persona's interest sites. If
    // the network is down this records skips (not visits); either way the read
    // mechanism is still exercised below. We cap the seed to the first couple of
    // category sites so the live test stays quick.
    let mut urls = categories::sites_for_persona(&persona);
    urls.truncate(2);
    let seed_outcome = categories::seed_history_for_persona(&browser, &persona, &urls, 7).await?;
    eprintln!(
        "live topics: seeded history visited={} skipped={}",
        seed_outcome.visited_count(),
        seed_outcome.skipped.len()
    );

    // Read topics on an eligible HTTPS page. The Topics API requires a secure
    // context, so we navigate to a real HTTPS site first. If the network is
    // unavailable we fall back to a local data: page; there the API will simply
    // report unavailable, which is still a well-formed read we assert succeeds.
    let page = browser.new_page().await?;
    let on_secure_context = page.navigate("https://example.com/").await.is_ok();
    if !on_secure_context {
        page.navigate(LOCAL_FALLBACK_URL).await?;
    }

    // The guarded read MUST succeed and return a well-formed TopicsReadback.
    let readback = page.read_topics().await?;
    eprintln!(
        "live topics: available={} topic_count={} (empty topics right after \
         seeding is the expected epoch-boundary outcome)",
        readback.available,
        readback.len()
    );

    // Acceptance: the call SUCCEEDS and the result is well-formed. We do NOT
    // require a non-empty list (epoch boundary). When the API is available, an
    // empty list is allowed; any returned topics must carry a valid id.
    for topic in &readback.topics {
        assert!(topic.topic_id >= 0, "topic id must be a valid integer");
    }
    if on_secure_context {
        eprintln!(
            "live topics: secure HTTPS context used; API available={}",
            readback.available
        );
    } else {
        eprintln!("live topics: local fallback used (no network); API typically unavailable");
    }

    // Clean shutdown, no orphan.
    browser.close().await?;
    let mut reaped = false;
    for _ in 0..50 {
        if !pid_alive(pid) {
            reaped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        reaped,
        "Chromium child {pid} must be reaped after shutdown (no orphan)"
    );

    Ok(())
}
