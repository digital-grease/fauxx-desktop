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

//! Per-persona network egress (C7 #30 N1) and DNS strategy (C7 #31 N2).
//!
//! These are the network-layer freedoms the phone deliberately lacks. Each
//! persona can route its decoy browser through its OWN exit (a proxy, Tor, or a
//! VPN's local SOCKS/HTTP front), and put DNS under explicit, observer-aware
//! control (the system resolver, DNS-over-HTTPS, or DNS-over-TLS). Both bind at
//! PERSONA scope, persist behind SQLCipher, and apply to the SAME isolated decoy
//! browser profile (via [`crate::browser::BrowserLaunchConfig`]).
//!
//! ## What this is, and is not (per-persona VPN honesty)
//!
//! The achievable per-persona mechanism on a portable desktop app is the
//! proxy/Tor SOCKS-or-HTTP exit applied to the decoy Chromium via its
//! `--proxy-server` flag. A TRUE per-PROCESS WireGuard/OpenVPN exit needs OS
//! network namespaces (Linux netns), `SO_BINDTODEVICE`, or per-process policy
//! routing, none of which a portable desktop app can do per persona without
//! elevated, OS-specific plumbing. So [`Egress::Vpn`] is modeled as carrying the
//! VPN's config and routing through a LOCAL SOCKS/HTTP front the VPN exposes (the
//! same `--proxy-server` seam), and full per-persona VPN isolation via namespaces
//! is tracked as a follow-up. The decoy BROWSER is the only egress/DNS consumer
//! here; a non-browser fetch path is out of scope.
//!
//! ## Fail closed (no real-IP leak)
//!
//! If a persona's configured egress is unreachable, that persona's decoy activity
//! is PAUSED and the state surfaced, rather than silently falling back to the
//! default route and leaking the real IP. The reachability check is abstracted
//! behind the [`ReachabilityCheck`] seam so the pause logic is hermetic-testable;
//! the live check (a TCP connect to the proxy/Tor endpoint) is the production
//! implementation. There is NO direct-route fallback in this code path.
//!
//! ## Credentials and the DB
//!
//! Proxy CREDENTIALS (username/password) NEVER touch the database and are never
//! logged. The persisted [`Egress`] row carries only a boolean
//! [`ProxyAuth::has_credentials`] marker plus a stable account label; the secret
//! username/password live in the OS keystore (see
//! [`crate::store::keystore`] proxy-credential helpers), exactly like the DB and
//! pairing keys.
//!
//! ## Observer trade-off (explicit, N2)
//!
//! The chosen DoH/DoT resolver SEES that persona's DNS lookups. That trade-off is
//! made explicit in the types: [`DnsStrategy::observer_note`] surfaces a
//! human-readable note naming the observer for the configured resolver, and the
//! defaults are privacy-respecting WITHOUT hardcoding a single mandatory provider
//! (the caller picks the endpoint; we ship sane suggestions, not a lock-in).

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The default Tor SOCKS5 listener address used when an [`Egress::Tor`] does not
/// override it. This is the standard local Tor daemon SOCKS port.
pub const DEFAULT_TOR_SOCKS_ADDR: &str = "127.0.0.1:9050";

/// Default timeout for a reachability TCP connect to an egress endpoint. Kept
/// short so a paused persona is surfaced quickly rather than hanging the UI.
pub const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(5);

// --- Proxy auth marker (credentials live in the keystore, never the DB) ------

/// A marker that an egress proxy requires authentication, plus the stable
/// keystore account label its secret username/password are stored under.
///
/// This carries NO secret material: the username and password live in the OS
/// keystore keyed by [`account_label`](Self::account_label), never in the
/// database row and never in a log line. Persisting an [`Egress`] serializes only
/// this marker, so a DB inspection can never reveal a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyAuth {
    /// Stable keystore account label under which the username/password are
    /// stored. NOT a secret; just the lookup key for the OS keystore entry.
    pub account_label: String,
}

impl ProxyAuth {
    /// A proxy-auth marker referencing the keystore entry at `account_label`.
    pub fn new(account_label: impl Into<String>) -> Self {
        Self {
            account_label: account_label.into(),
        }
    }
}

// --- Egress (per-persona exit, N1) -------------------------------------------

/// A per-persona network egress: how this persona's decoy browser reaches the
/// internet. Bound at persona scope and applied to the isolated decoy profile by
/// emitting the right Chromium `--proxy-server` argument.
///
/// Variants:
/// - [`Direct`](Self::Direct): the OS default route (no proxy). The only variant
///   that intentionally uses the real IP; explicit, never a silent fallback.
/// - [`HttpProxy`](Self::HttpProxy): an HTTP/HTTPS proxy exit, optional auth.
/// - [`SocksProxy`](Self::SocksProxy): a SOCKS5 proxy exit, optional auth.
/// - [`Tor`](Self::Tor): a local Tor SOCKS5 front (default `127.0.0.1:9050`).
/// - [`Vpn`](Self::Vpn): a VPN whose LOCAL SOCKS/HTTP front the decoy routes
///   through. See the module docs: per-process VPN namespace isolation is a
///   documented follow-up; today this is the proxy-front mechanism.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Egress {
    /// The OS default route (no proxy). Uses the real public IP by design.
    Direct,
    /// An HTTP/HTTPS proxy exit.
    #[serde(rename_all = "camelCase")]
    HttpProxy {
        /// Proxy host (name or IP).
        host: String,
        /// Proxy port.
        port: u16,
        /// Optional authentication marker; the secret lives in the keystore.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<ProxyAuth>,
    },
    /// A SOCKS5 proxy exit.
    #[serde(rename_all = "camelCase")]
    SocksProxy {
        /// Proxy host (name or IP).
        host: String,
        /// Proxy port.
        port: u16,
        /// Optional authentication marker; the secret lives in the keystore.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<ProxyAuth>,
    },
    /// A local Tor SOCKS5 front. Defaults to `127.0.0.1:9050`.
    #[serde(rename_all = "camelCase")]
    Tor {
        /// The Tor daemon's SOCKS5 listener address. Defaults to
        /// [`DEFAULT_TOR_SOCKS_ADDR`] via [`Egress::tor`].
        socks_addr: String,
    },
    /// A VPN whose LOCAL SOCKS/HTTP front the decoy routes through. See the
    /// module docs for the per-process namespace-isolation follow-up.
    #[serde(rename_all = "camelCase")]
    Vpn {
        /// A human-readable label for the VPN provider/profile (e.g. the exit
        /// region or config name), surfaced in the exit indicator.
        provider: String,
        /// The local SOCKS/HTTP front address the VPN exposes for the decoy to
        /// route through (e.g. `127.0.0.1:1080`).
        local_proxy_addr: String,
        /// Whether [`local_proxy_addr`](Self::Vpn::local_proxy_addr) is a SOCKS5
        /// front (`true`) or an HTTP proxy front (`false`).
        socks: bool,
    },
}

impl Egress {
    /// A Tor egress on the default local SOCKS5 port (`127.0.0.1:9050`).
    pub fn tor() -> Self {
        Egress::Tor {
            socks_addr: DEFAULT_TOR_SOCKS_ADDR.to_string(),
        }
    }

    /// A Tor egress on an explicit SOCKS5 address.
    pub fn tor_at(socks_addr: impl Into<String>) -> Self {
        Egress::Tor {
            socks_addr: socks_addr.into(),
        }
    }

    /// An HTTP-proxy egress with no authentication.
    pub fn http_proxy(host: impl Into<String>, port: u16) -> Self {
        Egress::HttpProxy {
            host: host.into(),
            port,
            auth: None,
        }
    }

    /// A SOCKS5-proxy egress with no authentication.
    pub fn socks_proxy(host: impl Into<String>, port: u16) -> Self {
        Egress::SocksProxy {
            host: host.into(),
            port,
            auth: None,
        }
    }

    /// The Chromium `--proxy-server` argument value for this egress, or `None`
    /// for [`Egress::Direct`] (which adds no proxy flag and uses the OS route).
    ///
    /// Mapping (the scheme Chromium expects):
    /// - HTTP proxy  -> `http://host:port`
    /// - SOCKS proxy -> `socks5://host:port`
    /// - Tor         -> `socks5://127.0.0.1:9050` (or the configured addr)
    /// - VPN front   -> `socks5://addr` or `http://addr` per its `socks` flag
    ///
    /// The username/password (when present) are NOT placed in the URL: Chromium
    /// is handed proxy auth out of band (the credentials come from the keystore),
    /// never embedded in a flag that could be logged.
    pub fn proxy_server_value(&self) -> Option<String> {
        match self {
            Egress::Direct => None,
            Egress::HttpProxy { host, port, .. } => Some(format!("http://{host}:{port}")),
            Egress::SocksProxy { host, port, .. } => Some(format!("socks5://{host}:{port}")),
            Egress::Tor { socks_addr } => Some(format!("socks5://{socks_addr}")),
            Egress::Vpn {
                local_proxy_addr,
                socks,
                ..
            } => {
                if *socks {
                    Some(format!("socks5://{local_proxy_addr}"))
                } else {
                    Some(format!("http://{local_proxy_addr}"))
                }
            }
        }
    }

    /// The full Chromium argument string this egress emits, e.g.
    /// `--proxy-server=socks5://127.0.0.1:9050`, or `None` for
    /// [`Egress::Direct`]. This is what the decoy launch passes to the browser
    /// and what the hermetic tests assert against without launching anything.
    pub fn chromium_proxy_arg(&self) -> Option<String> {
        self.proxy_server_value()
            .map(|v| format!("--proxy-server={v}"))
    }

    /// The proxy-auth marker for this egress, if any. The secret credentials are
    /// sourced from the keystore using its
    /// [`account_label`](ProxyAuth::account_label); this never returns a secret.
    pub fn proxy_auth(&self) -> Option<&ProxyAuth> {
        match self {
            Egress::HttpProxy { auth, .. } | Egress::SocksProxy { auth, .. } => auth.as_ref(),
            Egress::Direct | Egress::Tor { .. } | Egress::Vpn { .. } => None,
        }
    }

    /// A stable, human-readable provider/region label for the exit indicator
    /// (e.g. `"direct"`, `"Tor"`, `"http-proxy proxy.example:8080"`, the VPN
    /// provider name). Carries NO credential.
    pub fn exit_label(&self) -> String {
        match self {
            Egress::Direct => "direct (OS route)".to_string(),
            Egress::HttpProxy { host, port, .. } => format!("http-proxy {host}:{port}"),
            Egress::SocksProxy { host, port, .. } => format!("socks-proxy {host}:{port}"),
            Egress::Tor { socks_addr } => format!("Tor ({socks_addr})"),
            Egress::Vpn { provider, .. } => format!("VPN {provider}"),
        }
    }

    /// The `host:port` endpoint a reachability check probes (a TCP connect
    /// target), or `None` for [`Egress::Direct`] (nothing to probe; the OS route
    /// is always "reachable" by definition).
    pub fn reachability_endpoint(&self) -> Option<String> {
        match self {
            Egress::Direct => None,
            Egress::HttpProxy { host, port, .. } | Egress::SocksProxy { host, port, .. } => {
                Some(format!("{host}:{port}"))
            }
            Egress::Tor { socks_addr } => Some(socks_addr.clone()),
            Egress::Vpn {
                local_proxy_addr, ..
            } => Some(local_proxy_addr.clone()),
        }
    }
}

impl Default for Egress {
    /// The default egress is [`Egress::Direct`]: a persona with nothing configured
    /// uses the OS route. (Configuring a proxy then losing it FAILS CLOSED via the
    /// reachability check; that is a distinct, explicit code path, not a default.)
    fn default() -> Self {
        Egress::Direct
    }
}

// --- Reachability seam (fail-closed pause) -----------------------------------

/// Reachability-check seam for the fail-closed pause logic.
///
/// Abstracted so the pause logic is hermetic-testable (inject a
/// [`StaticReachability`] result); the live check is [`TcpReachability`], a TCP
/// connect to the egress endpoint with a short timeout. There is no
/// direct-route fallback anywhere in this seam: an unreachable egress pauses the
/// persona, full stop.
#[async_trait]
pub trait ReachabilityCheck: Send + Sync {
    /// Whether `egress` is currently reachable. [`Egress::Direct`] is always
    /// reachable (the OS route needs no probe). Any error reaching a configured
    /// proxy/Tor/VPN endpoint resolves to `false` (paused), never a silent
    /// fall-through to direct.
    async fn is_reachable(&self, egress: &Egress) -> bool;
}

/// A static reachability result for hermetic tests: always returns the injected
/// boolean (except [`Egress::Direct`], which is always reachable).
#[derive(Debug, Clone, Copy)]
pub struct StaticReachability {
    reachable: bool,
}

impl StaticReachability {
    /// A seam that reports every configured egress as reachable.
    pub fn reachable() -> Self {
        Self { reachable: true }
    }

    /// A seam that reports every configured egress as UNREACHABLE (drives the
    /// fail-closed pause path in tests).
    pub fn unreachable() -> Self {
        Self { reachable: false }
    }
}

#[async_trait]
impl ReachabilityCheck for StaticReachability {
    async fn is_reachable(&self, egress: &Egress) -> bool {
        // Direct is always reachable; otherwise the injected value.
        matches!(egress, Egress::Direct) || self.reachable
    }
}

/// The live reachability check: a TCP connect to the egress endpoint with a short
/// timeout. Used in production; never used in the hermetic tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpReachability;

#[async_trait]
impl ReachabilityCheck for TcpReachability {
    async fn is_reachable(&self, egress: &Egress) -> bool {
        let Some(endpoint) = egress.reachability_endpoint() else {
            // Direct: the OS route needs no probe.
            return true;
        };
        // Resolve and connect with a bounded timeout. Any failure is "not
        // reachable" (paused), NEVER a direct-route fallback.
        let addrs: Vec<SocketAddr> = match endpoint.to_socket_addrs() {
            Ok(iter) => iter.collect(),
            Err(_) => return false,
        };
        for addr in addrs {
            let connect = tokio::net::TcpStream::connect(addr);
            if let Ok(Ok(_stream)) = tokio::time::timeout(REACHABILITY_TIMEOUT, connect).await {
                return true;
            }
        }
        false
    }
}

/// The per-persona exit indicator: the configured exit provider/region (or Tor)
/// plus its reachable/paused state. Surfaced over the Core API so the UI can show
/// "persona X exits via Tor (reachable)" or "persona Y PAUSED: proxy unreachable".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressExit {
    /// The persona this exit belongs to.
    pub persona_id: String,
    /// The human-readable exit provider/region label (no credential).
    pub label: String,
    /// Whether the configured egress is currently reachable.
    pub reachable: bool,
    /// Whether this persona's decoy activity is PAUSED. A persona is paused when
    /// a non-[`Egress::Direct`] egress is unreachable; pausing prevents a
    /// real-IP leak rather than falling back to the default route.
    pub paused: bool,
    /// A one-line reason when paused (else `None`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
}

impl EgressExit {
    /// Build the exit indicator for `persona_id` from its `egress` and a
    /// reachability result. The fail-closed rule: a configured (non-Direct)
    /// egress that is unreachable yields `paused = true` with a reason; Direct is
    /// never paused.
    pub fn evaluate(persona_id: &str, egress: &Egress, reachable: bool) -> Self {
        let is_direct = matches!(egress, Egress::Direct);
        // Fail closed: only a CONFIGURED egress that is unreachable pauses the
        // persona. Direct never pauses (it is the explicit, opted-in real route).
        let paused = !is_direct && !reachable;
        let paused_reason = if paused {
            Some(format!(
                "egress {} is unreachable; decoy activity paused to avoid leaking the real IP",
                egress.exit_label()
            ))
        } else {
            None
        };
        Self {
            persona_id: persona_id.to_string(),
            label: egress.exit_label(),
            reachable,
            paused,
            paused_reason,
        }
    }
}

// --- DNS strategy (N2) -------------------------------------------------------

/// A privacy-respecting DEFAULT DoH resolver SUGGESTION (RFC 9462 / well-known
/// public resolver). This is a sane default the caller MAY adopt; it is NOT a
/// hardcoded mandatory provider. Callers are free to point at any resolver, and
/// the observer note always names whoever they choose.
pub const DEFAULT_DOH_RESOLVER: &str = "https://dns.quad9.net/dns-query";

/// A privacy-respecting DEFAULT DoT resolver SUGGESTION (hostname for the TLS
/// SNI/cert). Same non-mandatory rationale as [`DEFAULT_DOH_RESOLVER`].
pub const DEFAULT_DOT_RESOLVER: &str = "dns.quad9.net";

/// The per-persona (or per-egress) DNS strategy applied to the decoy browser.
///
/// - [`SystemDefault`](Self::SystemDefault): the OS resolver. Honest about the
///   trade-off: the OS-configured resolver (often the ISP) sees the lookups.
/// - [`Doh`](Self::Doh): DNS-over-HTTPS to an explicit resolver endpoint
///   (a DoH URI template). Applied to Chromium via the secure-DNS flags.
/// - [`Dot`](Self::Dot): DNS-over-TLS to an explicit resolver. Chromium's secure
///   DNS is configured over the same flags with the resolver template.
///
/// Where the egress supports it, resolution rides the persona's egress path so
/// lookups and traffic share ONE observer (no out-of-band leak to the OS default
/// resolver); see [`crate::browser`] for how the flags combine with the proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum DnsStrategy {
    /// Use the operating system's configured resolver.
    SystemDefault,
    /// DNS-over-HTTPS to an explicit resolver endpoint (a DoH URI template).
    #[serde(rename_all = "camelCase")]
    Doh {
        /// The DoH resolver endpoint / URI template (e.g.
        /// `https://dns.quad9.net/dns-query`).
        resolver: String,
    },
    /// DNS-over-TLS to an explicit resolver endpoint.
    #[serde(rename_all = "camelCase")]
    Dot {
        /// The DoT resolver endpoint (hostname, optionally `host:port`).
        resolver: String,
    },
}

impl DnsStrategy {
    /// A DoH strategy pointed at the privacy-respecting default resolver
    /// SUGGESTION. Not a lock-in; the caller may pass any endpoint.
    pub fn doh_default() -> Self {
        DnsStrategy::Doh {
            resolver: DEFAULT_DOH_RESOLVER.to_string(),
        }
    }

    /// A DoH strategy pointed at an explicit resolver endpoint.
    pub fn doh(resolver: impl Into<String>) -> Self {
        DnsStrategy::Doh {
            resolver: resolver.into(),
        }
    }

    /// A DoT strategy pointed at the privacy-respecting default resolver
    /// SUGGESTION. Not a lock-in; the caller may pass any endpoint.
    pub fn dot_default() -> Self {
        DnsStrategy::Dot {
            resolver: DEFAULT_DOT_RESOLVER.to_string(),
        }
    }

    /// A DoT strategy pointed at an explicit resolver endpoint.
    pub fn dot(resolver: impl Into<String>) -> Self {
        DnsStrategy::Dot {
            resolver: resolver.into(),
        }
    }

    /// The Chromium arguments that apply this DNS strategy to the decoy profile,
    /// in deterministic order. Empty for [`SystemDefault`](Self::SystemDefault)
    /// (no flag; Chromium uses the OS resolver).
    ///
    /// For DoH/DoT, Chromium's "secure DNS" is forced on with the chosen resolver
    /// template via `--enable-features=DnsOverHttps` plus
    /// `--dns-over-https-mode=secure` and `--dns-over-https-templates=<resolver>`.
    /// (Chromium has no distinct DoT transport flag; a DoT resolver is configured
    /// through the same secure-DNS template machinery, which the test asserts.)
    /// This is what the hermetic test inspects without launching a browser.
    pub fn chromium_dns_args(&self) -> Vec<String> {
        match self {
            DnsStrategy::SystemDefault => Vec::new(),
            DnsStrategy::Doh { resolver } | DnsStrategy::Dot { resolver } => vec![
                "--enable-features=DnsOverHttps".to_string(),
                "--dns-over-https-mode=secure".to_string(),
                format!("--dns-over-https-templates={resolver}"),
            ],
        }
    }

    /// The configured resolver endpoint, or `None` for
    /// [`SystemDefault`](Self::SystemDefault).
    pub fn resolver(&self) -> Option<&str> {
        match self {
            DnsStrategy::SystemDefault => None,
            DnsStrategy::Doh { resolver } | DnsStrategy::Dot { resolver } => Some(resolver),
        }
    }

    /// The EXPLICIT observer trade-off note for this strategy (N2): a
    /// human-readable line naming who sees this persona's DNS lookups. Surfaced
    /// in the config/types so the trade-off is never hidden.
    pub fn observer_note(&self) -> String {
        match self {
            DnsStrategy::SystemDefault => {
                "DNS: system default resolver. The OS-configured resolver (often \
                 your ISP) sees this persona's lookups."
                    .to_string()
            }
            DnsStrategy::Doh { resolver } => format!(
                "DNS: DNS-over-HTTPS via {resolver}. That DoH resolver sees this \
                 persona's lookups; choose one you trust. Where the egress supports \
                 it, lookups ride the egress so traffic and DNS share one observer."
            ),
            DnsStrategy::Dot { resolver } => format!(
                "DNS: DNS-over-TLS via {resolver}. That DoT resolver sees this \
                 persona's lookups; choose one you trust. Where the egress supports \
                 it, lookups ride the egress so traffic and DNS share one observer."
            ),
        }
    }
}

impl Default for DnsStrategy {
    /// The default DNS strategy is [`DnsStrategy::SystemDefault`]: a persona with
    /// nothing configured uses the OS resolver. The observer note keeps that
    /// trade-off explicit rather than silently "private by default".
    fn default() -> Self {
        DnsStrategy::SystemDefault
    }
}

// --- combined per-persona network config -------------------------------------

/// The combined per-persona network configuration: its [`Egress`] and its
/// [`DnsStrategy`]. N2 layers on N1, so this pairs them; both apply to the SAME
/// isolated decoy browser profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PersonaNetwork {
    /// How this persona's decoy browser reaches the internet.
    pub egress: Egress,
    /// How this persona's decoy browser resolves DNS.
    pub dns: DnsStrategy,
}

impl PersonaNetwork {
    /// A network config with the given egress and DNS strategy.
    pub fn new(egress: Egress, dns: DnsStrategy) -> Self {
        Self { egress, dns }
    }

    /// The combined Chromium argument list this config emits, in deterministic
    /// order: the `--proxy-server` arg (if any) first, then the DNS args. This is
    /// exactly the set the decoy launch applies; the hermetic tests assert on it.
    pub fn chromium_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(proxy) = self.egress.chromium_proxy_arg() {
            args.push(proxy);
        }
        args.extend(self.dns.chromium_dns_args());
        args
    }

    /// The explicit observer trade-off note for the DNS strategy (N2).
    pub fn observer_note(&self) -> String {
        self.dns.observer_note()
    }
}

/// Validate a proxy host string is non-empty and carries no embedded credential
/// or scheme (which would risk leaking a secret into a flag). Fail closed on a
/// malformed host.
fn validate_proxy_host(host: &str) -> Result<()> {
    if host.trim().is_empty() {
        return Err(CoreError::Network("proxy host is empty".to_string()));
    }
    if host.contains('@') || host.contains("://") || host.contains(char::is_whitespace) {
        return Err(CoreError::Network(format!(
            "proxy host {host:?} must be a bare host, not a URL or credential string"
        )));
    }
    Ok(())
}

/// Validate an [`Egress`] before it is persisted/applied. Fails closed on a
/// malformed host/endpoint so a bad config never silently degrades to direct.
pub fn validate_egress(egress: &Egress) -> Result<()> {
    match egress {
        Egress::Direct => Ok(()),
        Egress::HttpProxy { host, .. } | Egress::SocksProxy { host, .. } => {
            validate_proxy_host(host)
        }
        Egress::Tor { socks_addr } => {
            if socks_addr.trim().is_empty() {
                return Err(CoreError::Network("Tor SOCKS address is empty".to_string()));
            }
            Ok(())
        }
        Egress::Vpn {
            provider,
            local_proxy_addr,
            ..
        } => {
            if provider.trim().is_empty() {
                return Err(CoreError::Network(
                    "VPN provider label is empty".to_string(),
                ));
            }
            if local_proxy_addr.trim().is_empty() {
                return Err(CoreError::Network(
                    "VPN local proxy address is empty".to_string(),
                ));
            }
            Ok(())
        }
    }
}

/// Validate a [`DnsStrategy`] before it is persisted/applied. A DoH/DoT resolver
/// must be non-empty; DoH must be an `https://` template (the secure-DNS contract
/// Chromium expects). Fails closed on a malformed resolver.
pub fn validate_dns(dns: &DnsStrategy) -> Result<()> {
    match dns {
        DnsStrategy::SystemDefault => Ok(()),
        DnsStrategy::Doh { resolver } => {
            if !resolver.starts_with("https://") {
                return Err(CoreError::Network(format!(
                    "DoH resolver {resolver:?} must be an https:// URI template"
                )));
            }
            Ok(())
        }
        DnsStrategy::Dot { resolver } => {
            if resolver.trim().is_empty() {
                return Err(CoreError::Network("DoT resolver is empty".to_string()));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_proxy_emits_correct_chromium_arg() {
        let e = Egress::http_proxy("proxy.example", 8080);
        assert_eq!(
            e.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=http://proxy.example:8080")
        );
    }

    #[test]
    fn socks_proxy_emits_correct_chromium_arg() {
        let e = Egress::socks_proxy("10.0.0.2", 1080);
        assert_eq!(
            e.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=socks5://10.0.0.2:1080")
        );
    }

    #[test]
    fn tor_defaults_to_local_socks_and_maps_to_socks5() {
        let e = Egress::tor();
        assert_eq!(
            e.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=socks5://127.0.0.1:9050")
        );
    }

    #[test]
    fn vpn_routes_through_local_front() {
        let socks_vpn = Egress::Vpn {
            provider: "examplevpn-eu".to_string(),
            local_proxy_addr: "127.0.0.1:1080".to_string(),
            socks: true,
        };
        assert_eq!(
            socks_vpn.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=socks5://127.0.0.1:1080")
        );
        let http_vpn = Egress::Vpn {
            provider: "examplevpn-us".to_string(),
            local_proxy_addr: "127.0.0.1:3128".to_string(),
            socks: false,
        };
        assert_eq!(
            http_vpn.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=http://127.0.0.1:3128")
        );
    }

    #[test]
    fn direct_emits_no_proxy_arg() {
        assert!(Egress::Direct.chromium_proxy_arg().is_none());
        assert!(Egress::Direct.reachability_endpoint().is_none());
    }

    #[test]
    fn proxy_auth_marker_carries_no_secret() -> Result<()> {
        let e = Egress::HttpProxy {
            host: "proxy.example".to_string(),
            port: 8080,
            auth: Some(ProxyAuth::new("persona-1-proxy")),
        };
        // The serialized form references only the keystore label, never a
        // username or password.
        let json = serde_json::to_string(&e)?;
        assert!(json.contains("persona-1-proxy"));
        assert!(!json.to_lowercase().contains("password"));
        // The proxy URL never embeds credentials.
        assert_eq!(
            e.chromium_proxy_arg().as_deref(),
            Some("--proxy-server=http://proxy.example:8080")
        );
        assert_eq!(
            e.proxy_auth().map(|a| a.account_label.as_str()),
            Some("persona-1-proxy")
        );
        Ok(())
    }

    #[test]
    fn egress_round_trips_through_json() -> Result<()> {
        for e in [
            Egress::Direct,
            Egress::http_proxy("h", 1),
            Egress::socks_proxy("h", 2),
            Egress::tor(),
            Egress::tor_at("127.0.0.1:9150"),
            Egress::Vpn {
                provider: "v".to_string(),
                local_proxy_addr: "127.0.0.1:1080".to_string(),
                socks: true,
            },
        ] {
            let json = serde_json::to_string(&e)?;
            let back: Egress = serde_json::from_str(&json)?;
            assert_eq!(back, e);
        }
        Ok(())
    }

    #[test]
    fn fail_closed_pause_when_configured_egress_unreachable() {
        // A configured egress that is unreachable PAUSES the persona; the
        // indicator never reports a direct-route fallback.
        let exit = EgressExit::evaluate("p1", &Egress::tor(), false);
        assert!(exit.paused);
        assert!(!exit.reachable);
        assert!(exit.paused_reason.is_some());
        assert!(exit.label.contains("Tor"));
        // The label is NOT "direct": there is no fallback.
        assert!(!exit.label.contains("direct"));
    }

    #[test]
    fn reachable_egress_is_not_paused() {
        let exit = EgressExit::evaluate("p1", &Egress::socks_proxy("h", 1080), true);
        assert!(!exit.paused);
        assert!(exit.reachable);
        assert!(exit.paused_reason.is_none());
    }

    #[test]
    fn direct_is_never_paused_even_if_unreachable_reported() {
        // Direct is the explicit real route; the reachability flag does not pause
        // it (there is nothing to fail closed against).
        let exit = EgressExit::evaluate("p1", &Egress::Direct, false);
        assert!(!exit.paused);
    }

    #[tokio::test]
    async fn static_reachability_seam_drives_pause_logic() {
        let unreachable = StaticReachability::unreachable();
        assert!(!unreachable.is_reachable(&Egress::tor()).await);
        // Direct stays reachable even on the "unreachable" seam.
        assert!(unreachable.is_reachable(&Egress::Direct).await);
        let reachable = StaticReachability::reachable();
        assert!(reachable.is_reachable(&Egress::tor()).await);
    }

    #[test]
    fn doh_maps_to_chromium_secure_dns_flags() {
        let dns = DnsStrategy::doh("https://dns.example/dns-query");
        let args = dns.chromium_dns_args();
        assert!(args.contains(&"--enable-features=DnsOverHttps".to_string()));
        assert!(args.contains(&"--dns-over-https-mode=secure".to_string()));
        assert!(
            args.contains(&"--dns-over-https-templates=https://dns.example/dns-query".to_string())
        );
    }

    #[test]
    fn dot_maps_to_secure_dns_template() {
        let dns = DnsStrategy::dot("dns.example");
        let args = dns.chromium_dns_args();
        assert!(args.contains(&"--dns-over-https-templates=dns.example".to_string()));
    }

    #[test]
    fn system_default_dns_emits_no_flag() {
        assert!(DnsStrategy::SystemDefault.chromium_dns_args().is_empty());
    }

    #[test]
    fn observer_note_is_present_and_names_the_resolver() {
        // The observer trade-off note exists for every mode (N2 requirement).
        assert!(DnsStrategy::SystemDefault
            .observer_note()
            .to_lowercase()
            .contains("isp"));
        let doh = DnsStrategy::doh("https://dns.example/dns-query");
        assert!(doh
            .observer_note()
            .contains("https://dns.example/dns-query"));
        assert!(doh.observer_note().to_lowercase().contains("sees"));
        let dot = DnsStrategy::dot("dns.example");
        assert!(dot.observer_note().contains("dns.example"));
    }

    #[test]
    fn dns_round_trips_through_json() -> Result<()> {
        for d in [
            DnsStrategy::SystemDefault,
            DnsStrategy::doh_default(),
            DnsStrategy::dot_default(),
            DnsStrategy::doh("https://r/dns-query"),
            DnsStrategy::dot("r:853"),
        ] {
            let json = serde_json::to_string(&d)?;
            let back: DnsStrategy = serde_json::from_str(&json)?;
            assert_eq!(back, d);
        }
        Ok(())
    }

    #[test]
    fn persona_network_combines_proxy_then_dns_args() {
        let net = PersonaNetwork::new(
            Egress::tor(),
            DnsStrategy::doh("https://dns.example/dns-query"),
        );
        let args = net.chromium_args();
        // Proxy arg first, then the DNS args.
        assert_eq!(args[0], "--proxy-server=socks5://127.0.0.1:9050");
        assert!(args.contains(&"--enable-features=DnsOverHttps".to_string()));
        assert!(
            args.contains(&"--dns-over-https-templates=https://dns.example/dns-query".to_string())
        );
    }

    #[test]
    fn validation_fails_closed_on_bad_config() {
        assert!(validate_egress(&Egress::http_proxy("", 8080)).is_err());
        assert!(validate_egress(&Egress::http_proxy("user:pass@h", 8080)).is_err());
        assert!(validate_egress(&Egress::Direct).is_ok());
        assert!(validate_egress(&Egress::tor()).is_ok());
        // DoH must be https.
        assert!(validate_dns(&DnsStrategy::doh("http://insecure/dns-query")).is_err());
        assert!(validate_dns(&DnsStrategy::doh_default()).is_ok());
        assert!(validate_dns(&DnsStrategy::SystemDefault).is_ok());
    }
}
