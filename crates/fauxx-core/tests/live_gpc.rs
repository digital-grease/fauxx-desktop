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

//! LIVE Global Privacy Control integration test (C3 #18, D4c).
//!
//! Marked `#[ignore]` so CI (which has no Chromium) stays green; run it locally
//! with a system Chromium present:
//!
//! ```text
//! cargo test -p fauxx-core -- --ignored
//! ```
//!
//! It launches the decoy with GPC enabled (the default) and navigates it to a
//! self-contained loopback HTTP server (no third-party endpoint, so the
//! assertion is deterministic and not hostage to a flaky external service). It
//! asserts the captured request carried `Sec-GPC: 1` AND that
//! `navigator.globalPrivacyControl` reads back `true`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fauxx_core::browser::{BrowserLaunchConfig, DecoyBrowser};

/// The Chromium/Chrome binary the live tests drive. Defaults to the system
/// `/usr/bin/chromium`; the CI Chromium lane overrides it via the
/// `FAUXX_TEST_CHROMIUM` env var (pointed at the runner's installed browser).
fn chromium_path() -> String {
    std::env::var("FAUXX_TEST_CHROMIUM").unwrap_or_else(|_| "/usr/bin/chromium".to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chromium (FAUXX_TEST_CHROMIUM or /usr/bin/chromium); run with --ignored"]
async fn live_decoy_emits_sec_gpc_header_and_navigator_flag(
) -> Result<(), Box<dyn std::error::Error>> {
    let chromium = chromium_path();
    assert!(
        PathBuf::from(&chromium).exists(),
        "this live test requires Chromium at {chromium} (set FAUXX_TEST_CHROMIUM)"
    );

    // A self-contained loopback HTTP server. It captures each request's raw
    // header block and replies 200 so the decoy navigation always succeeds.
    // Using loopback (not a third party) makes the Sec-GPC:1 assertion
    // deterministic. The server self-terminates on a deadline so no thread is
    // left blocked on accept().
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                          Content-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                    let _ = tx.send(req);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    let tmp = tempfile::tempdir()?;
    let config = BrowserLaunchConfig::new()
        .with_executable(&chromium)
        .with_user_data_dir(tmp.path().join("decoy-gpc"))
        // The test host may lack a usable Chromium sandbox; production keeps it on.
        .with_no_sandbox(true);
    assert!(config.gpc_enabled(), "GPC must default ON for the decoy");

    let browser = DecoyBrowser::launch_with("live-gpc", config).await?;
    assert!(browser.gpc_enabled());
    let page = browser.new_page().await?;

    let url = format!("http://127.0.0.1:{port}/");
    page.navigate(&url).await?;

    // navigator.globalPrivacyControl must read back true on the decoy. This is
    // offline-verifiable and asserted unconditionally.
    let nav_gpc = page.read_navigator_gpc().await?;
    assert_eq!(
        nav_gpc,
        Some(true),
        "navigator.globalPrivacyControl should be true on a GPC-enabled decoy page"
    );

    browser.close().await?;

    // The captured document request must have carried Sec-GPC: 1.
    let mut saw_sec_gpc = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(req) => {
                let lower = req.to_lowercase();
                if lower.contains("sec-gpc: 1") || lower.contains("sec-gpc:1") {
                    saw_sec_gpc = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        saw_sec_gpc,
        "decoy navigation must send the Sec-GPC: 1 request header"
    );

    Ok(())
}

/// AC2 (live honoring DETECTION): the decoy fetches a site's
/// `/.well-known/gpc.json` through the real browser and parses the honoring
/// flag. A loopback server serves `{"gpc": true}`; the decoy navigates to the
/// origin (so the well-known fetch is same-origin) and reads it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chromium (FAUXX_TEST_CHROMIUM or /usr/bin/chromium); run with --ignored"]
async fn live_decoy_detects_well_known_gpc_honoring() -> Result<(), Box<dyn std::error::Error>> {
    let chromium = chromium_path();
    assert!(
        PathBuf::from(&chromium).exists(),
        "this live test requires Chromium at {chromium} (set FAUXX_TEST_CHROMIUM)"
    );

    // Loopback server: reply 200 with the honoring well-known JSON to every
    // request (so both the initial navigation and the /.well-known/gpc.json
    // fetch get it). CORS is a non-issue: the fetch is same-origin.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    std::thread::spawn(move || {
        let start = Instant::now();
        let body = "{\"gpc\":true,\"lastUpdate\":\"2026-01-01\",\"version\":\"1.0\"}";
        while start.elapsed() < Duration::from_secs(20) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Access-Control-Allow-Origin: *\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    let tmp = tempfile::tempdir()?;
    let config = BrowserLaunchConfig::new()
        .with_executable(&chromium)
        .with_user_data_dir(tmp.path().join("decoy-gpc-detect"))
        .with_no_sandbox(true);
    let browser = DecoyBrowser::launch_with("live-gpc-detect", config).await?;
    let page = browser.new_page().await?;

    let origin = format!("http://127.0.0.1:{port}");
    // Navigate to the origin first so the well-known fetch is same-origin.
    page.navigate(&format!("{origin}/")).await?;
    let support = page.fetch_gpc_well_known(&origin).await?;
    browser.close().await?;

    assert!(
        support.honored,
        "the decoy must parse honored=true from the served /.well-known/gpc.json"
    );
    Ok(())
}
