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

//! C7 Network & Identity integration tests (issues #30 N1, #31 N2).
//!
//! Hermetic: no real proxies, Tor, or network. The egress + DNS models persist
//! and round-trip per persona through a temp [`EncryptedFile`] store; binding a
//! persona to a proxy/Tor egress emits the right Chromium `--proxy-server` arg
//! (asserted on the [`BrowserLaunchConfig`] arg string, no browser launched);
//! the fail-closed pause logic pauses a persona whose egress reachability check
//! fails (via the [`StaticReachability`] seam) and never yields a direct-route
//! fallback; proxy credentials are NOT in the persisted DB row and are sourced
//! from the keystore; the DoH/DoT resolver maps to the correct Chromium flag;
//! and the observer-trade-off note is present.
//!
//! The LIVE two-distinct-exits test is `#[ignore]` (needs a real local SOCKS/Tor
//! proxy + network); CI does not run it.

use fauxx_core::browser::BrowserLaunchConfig;
use fauxx_core::persona::{AgeRange, CategoryPool, Profession, Region, SyntheticPersona};
use fauxx_core::store::{EncryptedStore, KeySource};
use fauxx_core::{
    Config, Core, CoreError, DnsStrategy, Egress, PersonaNetwork, ProxyAuth, Result,
    StaticReachability,
};

fn temp_config(dir: &std::path::Path) -> Config {
    Config::new()
        .with_path(dir.join("fauxx.db"))
        .with_key_source(KeySource::EncryptedFile {
            path: dir.join("key.bin"),
            passphrase: "c7-network-test-pass".to_string(),
        })
}

fn persona(id: &str) -> SyntheticPersona {
    SyntheticPersona::new(
        id.to_string(),
        "C7 Persona".to_string(),
        AgeRange::AGE_25_34.as_name().to_string(),
        Profession::ENGINEER.as_name().to_string(),
        Region::US_WEST.as_name().to_string(),
        vec![
            CategoryPool::TECHNOLOGY.as_name().to_string(),
            CategoryPool::SCIENCE.as_name().to_string(),
        ],
        1_700_000_000_000,
        1_700_600_000_000,
    )
}

#[tokio::test]
async fn egress_and_dns_persist_and_round_trip_per_persona() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("11111111-1111-4111-8111-111111111111");
    core.save_persona(&p).await?;

    // Default before any binding: Direct + SystemDefault.
    assert_eq!(core.get_persona_egress(&p.id).await?, Egress::Direct);
    assert_eq!(
        core.get_persona_dns(&p.id).await?,
        DnsStrategy::SystemDefault
    );

    // Bind a SOCKS proxy egress and a DoH strategy.
    let egress = Egress::socks_proxy("10.0.0.2", 1080);
    core.set_persona_egress(&p.id, egress.clone()).await?;
    let dns = DnsStrategy::doh("https://dns.example/dns-query");
    core.set_persona_dns(&p.id, dns.clone()).await?;

    assert_eq!(core.get_persona_egress(&p.id).await?, egress);
    assert_eq!(core.get_persona_dns(&p.id).await?, dns);

    // Reopen the store: the bindings survive (persisted behind SQLCipher).
    drop(core);
    let core2 = Core::open(temp_config(dir.path())).await?;
    assert_eq!(core2.get_persona_egress(&p.id).await?, egress);
    assert_eq!(core2.get_persona_dns(&p.id).await?, dns);

    // The combined config pairs them.
    let net = core2.persona_network(&p.id).await?;
    assert_eq!(net, PersonaNetwork::new(egress, dns));

    // Clearing reverts to defaults.
    assert!(core2.clear_persona_egress(&p.id).await?);
    assert!(core2.clear_persona_dns(&p.id).await?);
    assert_eq!(core2.get_persona_egress(&p.id).await?, Egress::Direct);
    assert_eq!(
        core2.get_persona_dns(&p.id).await?,
        DnsStrategy::SystemDefault
    );
    Ok(())
}

#[tokio::test]
async fn binding_proxy_egress_emits_correct_chromium_arg() -> Result<()> {
    // HTTP proxy.
    let http = BrowserLaunchConfig::new().with_egress(Egress::http_proxy("proxy.example", 8080));
    assert!(http
        .network_chromium_args()
        .contains(&"--proxy-server=http://proxy.example:8080".to_string()));

    // SOCKS proxy.
    let socks = BrowserLaunchConfig::new().with_egress(Egress::socks_proxy("10.0.0.2", 1080));
    assert!(socks
        .network_chromium_args()
        .contains(&"--proxy-server=socks5://10.0.0.2:1080".to_string()));

    // Tor maps to the default local SOCKS5 listener.
    let tor = BrowserLaunchConfig::new().with_egress(Egress::tor());
    assert!(tor
        .network_chromium_args()
        .contains(&"--proxy-server=socks5://127.0.0.1:9050".to_string()));

    // Direct emits nothing (uses the OS route).
    let direct = BrowserLaunchConfig::new().with_egress(Egress::Direct);
    assert!(direct.network_chromium_args().is_empty());
    Ok(())
}

#[tokio::test]
async fn doh_and_dot_map_to_correct_chromium_flags_on_the_same_profile() -> Result<()> {
    // The egress and DNS apply to the SAME isolated profile: both args present.
    let cfg = BrowserLaunchConfig::new().with_network(PersonaNetwork::new(
        Egress::tor(),
        DnsStrategy::doh("https://dns.example/dns-query"),
    ));
    let args = cfg.network_chromium_args();
    assert!(args.contains(&"--proxy-server=socks5://127.0.0.1:9050".to_string()));
    assert!(args.contains(&"--enable-features=DnsOverHttps".to_string()));
    assert!(args.contains(&"--dns-over-https-mode=secure".to_string()));
    assert!(args.contains(&"--dns-over-https-templates=https://dns.example/dns-query".to_string()));

    // DoT maps to the same secure-DNS template machinery.
    let dot = BrowserLaunchConfig::new().with_dns(DnsStrategy::dot("dns.example"));
    assert!(dot
        .network_chromium_args()
        .contains(&"--dns-over-https-templates=dns.example".to_string()));
    Ok(())
}

#[tokio::test]
async fn fail_closed_pauses_unreachable_egress_with_no_direct_fallback() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("22222222-2222-4222-8222-222222222222");
    core.save_persona(&p).await?;
    core.set_persona_egress(&p.id, Egress::tor()).await?;

    // Inject an UNREACHABLE result via the seam.
    let down = StaticReachability::unreachable();
    let exit = core.persona_egress_exit(&p.id, &down).await?;
    assert!(exit.paused, "unreachable configured egress must pause");
    assert!(!exit.reachable);
    assert!(exit.paused_reason.is_some());
    // The exit indicator reports the configured exit (Tor), NEVER a direct
    // fallback: there is no leak to the real route.
    assert!(exit.label.contains("Tor"));
    assert!(!exit.label.to_lowercase().contains("direct"));

    // Launching a per-persona decoy while paused FAILS CLOSED (no launch, no
    // direct-route fallback) with a Network error.
    let launch = core
        .launch_persona_decoy_browser(&p.id, "decoy-22", &down)
        .await;
    assert!(matches!(launch, Err(CoreError::Network(_))));

    // A reachable seam clears the pause.
    let up = StaticReachability::reachable();
    let exit_ok = core.persona_egress_exit(&p.id, &up).await?;
    assert!(!exit_ok.paused);
    assert!(exit_ok.reachable);
    Ok(())
}

#[tokio::test]
async fn authenticated_proxy_egress_without_credentials_fails_closed() -> Result<()> {
    // An egress declaring proxy auth but with NO stored credentials must fail
    // closed at launch with a clear error, rather than start a browser whose
    // every request would 407. The REACHABLE seam ensures we are exercising the
    // auth-credential gate, not the reachability pause. This returns before any
    // Chromium launch, so it is hermetic (no browser needed).
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("44444444-4444-4444-8444-444444444444");
    core.save_persona(&p).await?;
    let egress = Egress::HttpProxy {
        host: "proxy.example".to_string(),
        port: 8080,
        auth: Some(ProxyAuth::new("persona-44-egress")),
    };
    core.set_persona_egress(&p.id, egress).await?;
    // Deliberately do NOT store credentials.
    assert!(!core.has_persona_proxy_credentials(&p.id).await?);

    // Reachable seam, so we exercise the auth-credential gate (not the
    // reachability pause). The missing-credentials error returns BEFORE any
    // Chromium launch, keeping this hermetic.
    let up = StaticReachability::reachable();
    let launch = core
        .launch_persona_decoy_browser(&p.id, "decoy-44", &up)
        .await;
    match launch {
        Err(CoreError::Network(msg)) => assert!(
            msg.contains("no credentials are stored"),
            "expected a missing-credentials fail-closed message, got: {msg}"
        ),
        other => panic!("expected a fail-closed Network error for an auth proxy, got {other:?}"),
    }
    Ok(())
}

/// Live: an AUTHENTICATED proxy egress WITH stored credentials launches (the
/// auth-credential gate passes) and applies the credentials per page via CDP.
/// Requires a real Chromium; full proxy-auth round-trip additionally needs a
/// real authenticated proxy at the configured host (navigate to exercise it).
/// Ignored by default like the other live browser tests.
#[tokio::test]
#[ignore = "requires a real Chromium (and an authenticated proxy to exercise the 407 round-trip)"]
async fn authenticated_proxy_egress_with_credentials_launches() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("44444444-4444-4444-8444-444444444444");
    core.save_persona(&p).await?;
    let egress = Egress::HttpProxy {
        host: "127.0.0.1".to_string(),
        port: 8080,
        auth: Some(ProxyAuth::new("persona-44-egress")),
    };
    core.set_persona_egress(&p.id, egress).await?;
    core.set_persona_proxy_credentials(&p.id, "egress-user", "egress-pass")
        .await?;

    let up = StaticReachability::reachable();
    // The auth gate passes (credentials present) and Chromium launches with the
    // proxy configured; `new_page` then applies the credentials via CDP.
    let browser = core
        .launch_persona_decoy_browser(&p.id, "decoy-44-live", &up)
        .await?;
    let _page = browser.new_page().await?;
    browser.close().await?;
    Ok(())
}

#[tokio::test]
async fn direct_egress_is_never_paused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("33333333-3333-4333-8333-333333333333");
    core.save_persona(&p).await?;
    // No egress bound -> Direct. Even the "unreachable" seam does not pause it
    // (Direct is the explicit, opted-in real route; nothing to fail closed on).
    let down = StaticReachability::unreachable();
    let exit = core.persona_egress_exit(&p.id, &down).await?;
    assert!(!exit.paused);
    Ok(())
}

#[tokio::test]
async fn proxy_credentials_are_not_in_db_and_come_from_keystore() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("44444444-4444-4444-8444-444444444444");
    core.save_persona(&p).await?;

    // Bind an HTTP proxy egress carrying a NON-secret auth marker (keystore
    // label), then store the secret credentials in the keystore.
    let label = "persona-44-egress";
    let egress = Egress::HttpProxy {
        host: "proxy.example".to_string(),
        port: 8080,
        auth: Some(ProxyAuth::new(label)),
    };
    core.set_persona_egress(&p.id, egress).await?;

    let secret_user = "egress-user";
    let secret_pw = "sup3r-s3cret-passphrase";
    core.set_persona_proxy_credentials(&p.id, secret_user, secret_pw)
        .await?;
    assert!(core.has_persona_proxy_credentials(&p.id).await?);

    // Assert the secret is NOT in the persisted DB row: read the raw JSON the
    // store persists for this persona's egress and confirm neither the username
    // nor the password (nor the word "password") appears. Only the non-secret
    // keystore label is present.
    let key_source = KeySource::EncryptedFile {
        path: dir.path().join("key.bin"),
        passphrase: "c7-network-test-pass".to_string(),
    };
    let store = EncryptedStore::open_at(&dir.path().join("fauxx.db"), &key_source)?;
    let stored = store
        .get_persona_egress(&p.id)?
        .ok_or_else(|| CoreError::Network("egress missing".into()))?;
    let row_json = serde_json::to_string(&stored)?;
    assert!(
        !row_json.contains(secret_user),
        "username must not be in the DB row"
    );
    assert!(
        !row_json.contains(secret_pw),
        "password must not be in the DB row"
    );
    assert!(
        !row_json.to_lowercase().contains("password"),
        "no password field in the DB row"
    );
    // The non-secret keystore label IS present (so the secret can be found).
    assert!(row_json.contains(label));

    // The secret is sourced from the keystore, and clearing the egress also
    // removes the credential so it does not outlive its config.
    assert!(core.clear_persona_egress(&p.id).await?);
    assert!(!core.has_persona_proxy_credentials(&p.id).await?);
    Ok(())
}

#[tokio::test]
async fn observer_trade_off_note_is_present_and_explicit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("55555555-5555-4555-8555-555555555555");
    core.save_persona(&p).await?;

    // SystemDefault: the note still surfaces the trade-off (OS/ISP resolver).
    let sys_note = core.persona_dns_observer_note(&p.id).await?;
    assert!(sys_note.to_lowercase().contains("isp"));

    // DoH: the note names the chosen resolver and that it SEES the lookups.
    let resolver = "https://dns.example/dns-query";
    core.set_persona_dns(&p.id, DnsStrategy::doh(resolver))
        .await?;
    let doh_note = core.persona_dns_observer_note(&p.id).await?;
    assert!(doh_note.contains(resolver));
    assert!(doh_note.to_lowercase().contains("sees"));
    Ok(())
}

#[tokio::test]
async fn malformed_egress_and_dns_fail_closed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;
    let p = persona("66666666-6666-4666-8666-666666666666");
    core.save_persona(&p).await?;

    // An empty proxy host is refused (fail closed; not silently degraded).
    assert!(matches!(
        core.set_persona_egress(&p.id, Egress::http_proxy("", 8080))
            .await,
        Err(CoreError::Network(_))
    ));
    // A non-https DoH resolver is refused.
    assert!(matches!(
        core.set_persona_dns(&p.id, DnsStrategy::doh("http://insecure/dns-query"))
            .await,
        Err(CoreError::Network(_))
    ));
    // Nothing was persisted: still defaults.
    assert_eq!(core.get_persona_egress(&p.id).await?, Egress::Direct);
    assert_eq!(
        core.get_persona_dns(&p.id).await?,
        DnsStrategy::SystemDefault
    );
    Ok(())
}

#[tokio::test]
async fn storeless_core_network_api_fails_closed() -> Result<()> {
    let core = Core::new();
    // Reads default cleanly.
    assert_eq!(core.get_persona_egress("x").await?, Egress::Direct);
    assert_eq!(core.get_persona_dns("x").await?, DnsStrategy::SystemDefault);
    // Writes require a store.
    assert!(matches!(
        core.set_persona_egress("x", Egress::tor()).await,
        Err(CoreError::Unimplemented(_))
    ));
    assert!(matches!(
        core.set_persona_dns("x", DnsStrategy::doh_default()).await,
        Err(CoreError::Unimplemented(_))
    ));
    Ok(())
}

/// LIVE two-distinct-exits test (C7 #30 N1). `#[ignore]` so CI stays green: it
/// requires TWO real, distinct local SOCKS/Tor exits and network access.
///
/// Run locally with two exits configured (for example a Tor SOCKS proxy on
/// 127.0.0.1:9050 and a second SOCKS proxy on a different exit), then:
///
/// ```text
/// cargo test -p fauxx-core --test network_egress -- --ignored
/// ```
///
/// It binds two personas to the two distinct exits, launches each persona's
/// isolated decoy browser through its own egress, navigates to a public IP-echo
/// endpoint, reads back the observed public IP per persona, and asserts the two
/// personas report DIFFERENT public IPs (proving per-persona egress isolation).
/// Adjust the two exit addresses and the IP-echo URL for your local setup.
#[tokio::test]
#[ignore = "requires two real local SOCKS/Tor exits + network; run with --ignored"]
async fn live_two_personas_report_different_public_ips() -> Result<()> {
    // Two DISTINCT local exits. Replace these with your real local exits.
    let exit_a = Egress::tor_at("127.0.0.1:9050");
    let exit_b = Egress::socks_proxy("127.0.0.1", 9052);
    // A public endpoint that echoes the caller's IP as plain text.
    let ip_echo_url = "https://api.ipify.org";

    let dir = tempfile::tempdir()?;
    let core = Core::open(temp_config(dir.path())).await?;

    let pa = persona("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let pb = persona("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    core.save_persona(&pa).await?;
    core.save_persona(&pb).await?;
    core.set_persona_egress(&pa.id, exit_a).await?;
    core.set_persona_egress(&pb.id, exit_b).await?;

    let ip_a = read_public_ip(&core, &pa.id, "live-decoy-a", ip_echo_url).await?;
    let ip_b = read_public_ip(&core, &pb.id, "live-decoy-b", ip_echo_url).await?;

    assert!(!ip_a.trim().is_empty(), "persona A reported no IP");
    assert!(!ip_b.trim().is_empty(), "persona B reported no IP");
    assert_ne!(
        ip_a.trim(),
        ip_b.trim(),
        "two personas with distinct exits must report different public IPs"
    );
    Ok(())
}

/// Helper for the live test: launch the persona's isolated decoy through its own
/// egress (live reachability check), navigate to the IP echo, and read the body.
#[cfg(test)]
async fn read_public_ip(
    core: &Core,
    persona_id: &str,
    decoy_id: &str,
    url: &str,
) -> Result<String> {
    let browser = core
        .launch_persona_decoy_browser_live(persona_id, decoy_id)
        .await?;
    let page = browser.new_page().await?;
    page.navigate(url).await?;
    let body = page.content().await?;
    let _ = browser.close().await;
    Ok(body)
}
