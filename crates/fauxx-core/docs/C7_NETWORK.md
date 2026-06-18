# C7 Network & Identity (per-persona egress + DNS)

This note documents the two network-layer freedoms the desktop companion gives
each persona that the phone deliberately lacks: per-persona EGRESS (#30, N1) and
an observer-aware DNS strategy (#31, N2). Both live in `fauxx-core` behind the
clean async `Core` API; the GUI and CLI are thin clients. N2 layers on N1.

Both features are 100% local (no telemetry), persist behind SQLCipher, FAIL
CLOSED, and apply to the SAME isolated decoy browser profile. The decoy BROWSER
is the only egress/DNS consumer; there is no non-browser fetch path here.

## N1: per-persona egress (`crate::network::Egress`)

Each persona can route its decoy browser through its OWN exit. The `Egress` enum
covers:

- `Direct`: the OS default route (no proxy). The only variant that intentionally
  uses the real public IP, and it is explicit, never a silent fallback.
- `HttpProxy { host, port, auth? }`: an HTTP/HTTPS proxy exit.
- `SocksProxy { host, port, auth? }`: a SOCKS5 proxy exit.
- `Tor { socks_addr }`: a local Tor SOCKS5 front, default `127.0.0.1:9050`.
- `Vpn { provider, local_proxy_addr, socks }`: a VPN routed via its LOCAL
  SOCKS/HTTP front (see the honesty note below).

The egress is bound at PERSONA scope and persisted in the `persona_egress` table
(schema v12 -> v13). It is applied to the decoy by emitting the right Chromium
`--proxy-server` argument into the isolated profile launch
(`BrowserLaunchConfig::with_egress` / `with_network`):

- HTTP proxy  -> `--proxy-server=http://host:port`
- SOCKS proxy -> `--proxy-server=socks5://host:port`
- Tor         -> `--proxy-server=socks5://127.0.0.1:9050`
- VPN front   -> `--proxy-server=socks5://addr` or `http://addr`

### Per-persona VPN honesty (namespaces are a follow-up)

A TRUE per-PROCESS WireGuard/OpenVPN exit needs OS network NAMESPACES (Linux
netns), `SO_BINDTODEVICE`, or per-process policy routing, none of which a
portable desktop app can do per persona without elevated, OS-specific plumbing.
So `Egress::Vpn` is modeled as carrying the VPN's config and routing through a
LOCAL SOCKS/HTTP front the VPN exposes (the same `--proxy-server` seam). Full
per-persona VPN isolation via network namespaces is a documented FOLLOW-UP. The
achievable per-persona mechanism today is the proxy/Tor SOCKS-or-HTTP exit.

### Credentials (keystore, never the DB, never logs)

Proxy CREDENTIALS (username/password) never touch the database and are never
logged. The persisted egress row carries only a non-secret `ProxyAuth` marker:
the keystore ACCOUNT LABEL the secret is stored under. The secret
username/password live in the OS keystore (Secret Service / Keychain / Windows
Credential Manager), with the Argon2id passphrase-file fallback on headless
hosts, exactly like the DB and pairing keys (`crate::store::keystore`
proxy-credential helpers). Clearing a persona's egress also removes its keystore
credential so a secret never outlives its config.

### Fail closed (no real-IP leak)

If a persona's configured (non-`Direct`) egress is UNREACHABLE, that persona's
decoy activity is PAUSED and the state is surfaced via the exit indicator
(`EgressExit`: the configured provider/region or Tor, plus reachable/paused
state and a reason). It is NEVER silently fallen back to the OS default route,
because that would leak the real IP. The reachability check is a TCP connect to
the egress endpoint (`TcpReachability`), abstracted behind the
`ReachabilityCheck` seam so the pause logic is hermetic-testable (inject a
`StaticReachability` result). `Core::launch_persona_decoy_browser` refuses to
launch a paused persona, returning `CoreError::Network` rather than a
direct-route launch.

### Core API (N1)

- `set_persona_egress` / `get_persona_egress` / `clear_persona_egress`
- `set_persona_proxy_credentials` / `has_persona_proxy_credentials` (keystore)
- `persona_egress_exit` (seam) / `persona_egress_exit_live` (TCP) -> `EgressExit`
- `launch_persona_decoy_browser` (seam) / `launch_persona_decoy_browser_live`

## N2: DNS strategy (`crate::network::DnsStrategy`)

Each persona (or its egress) gets an explicit, observer-aware DNS mode:

- `SystemDefault`: the OS resolver (often the ISP).
- `Doh { resolver }`: DNS-over-HTTPS to an explicit `https://` resolver template.
- `Dot { resolver }`: DNS-over-TLS to an explicit resolver endpoint.

Bound at persona scope and persisted in the `persona_dns` table (schema v13 ->
v14). Applied to the decoy via Chromium's secure-DNS flags on the SAME isolated
profile as the egress:

- `--enable-features=DnsOverHttps`
- `--dns-over-https-mode=secure`
- `--dns-over-https-templates=<resolver>`

(Chromium has no distinct DoT transport flag; a DoT resolver is configured
through the same secure-DNS template machinery.) Because the resolver rides the
SAME isolated decoy profile, and where the egress supports it the lookups travel
the egress path, a persona's traffic and DNS share ONE observer, with no
out-of-band leak to the OS default resolver.

### Explicit observer trade-off

The chosen DoH/DoT resolver SEES that persona's lookups. That trade-off is made
EXPLICIT in the types: `DnsStrategy::observer_note` returns a human-readable line
naming the observer for the configured resolver, surfaced over the Core API
(`Core::persona_dns_observer_note`). The defaults are privacy-respecting
SUGGESTIONS (`DEFAULT_DOH_RESOLVER`, `DEFAULT_DOT_RESOLVER`) WITHOUT hardcoding a
single mandatory provider: the caller picks the endpoint, and the note always
names whoever they choose. DNS choices are persisted; the resolver is not logged
as sensitive.

### Core API (N2)

- `set_persona_dns` / `get_persona_dns` / `clear_persona_dns`
- `persona_dns_observer_note`
- `persona_network` (the combined egress + DNS applied to the decoy)

## Tests

Hermetic tests (no real proxies/Tor/network) live in `tests/network_egress.rs`
and the `network`/`browser`/`store::keystore` unit tests: the egress + DNS models
round-trip per persona through a temp `EncryptedFile` store; binding an
HttpProxy/SocksProxy/Tor egress emits the correct `--proxy-server` arg (asserted
on the `BrowserLaunchConfig` arg string, no browser launched); the fail-closed
pause logic pauses an unreachable egress (via the `StaticReachability` seam) and
never yields a direct-route fallback; proxy credentials are absent from the
persisted DB row and are sourced from the keystore; the DoH/DoT resolver maps to
the right Chromium flag/template; and the observer-trade-off note is present.

A LIVE test (`#[ignore]`, `live_two_personas_report_different_public_ips`) drives
two personas with distinct local SOCKS/Tor exits to a public IP-echo endpoint and
asserts they report DIFFERENT public IPs. It needs a real local proxy/Tor + network
and is not run in CI.

## Known limitations / follow-ups

- **Authenticated proxies are not yet applied to the browser.** Chromium ignores
  credentials in the `--proxy-server` flag, so an `HttpProxy`/`SocksProxy` egress
  that requires auth needs a CDP `Fetch.continueWithAuth` (or `Proxy-Authorization`
  header) handler that loads the keystore secret and answers the proxy auth
  challenge. That handler is a follow-up (it must be live-verified against a real
  authenticated proxy, and a mis-wired `Fetch` interceptor would stall all
  requests). Until it lands, the credential storage API is ready but
  `launch_persona_decoy_browser` REFUSES to launch a persona whose egress carries
  a proxy-auth marker, returning a clear `CoreError::Network` rather than starting
  a browser whose every request would fail the auth challenge (fail closed and
  honest, never a Direct fallback). Unauthenticated proxies, Tor, and Direct work
  today. Test: `authenticated_proxy_egress_is_not_yet_launchable`.
- **Reachability is a pre-launch TCP-liveness gate (TOCTOU).** The fail-closed
  check is a fast TCP connect to the egress endpoint at launch time; a proxy that
  dies mid-session is not re-gated by us, but Chromium itself fails closed on a
  proxy that stops responding (a `--proxy-server` browser does NOT silently fall
  back to the direct route), so no real-IP leak results. Higher-fidelity liveness
  (a SOCKS5 handshake or HTTP CONNECT probe rather than a bare TCP connect) is a
  possible future refinement.
- **Per-persona VPN namespace isolation** (true per-process WireGuard/OpenVPN
  exits) is a follow-up; `Egress::Vpn` currently routes through a local SOCKS/HTTP
  front (see the N1 section above).
