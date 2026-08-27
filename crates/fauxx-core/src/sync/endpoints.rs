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

//! This device's dialable LAN endpoints.
//!
//! # Why a name is not enough (#38)
//!
//! Discovery advertises this device under an mDNS host name (`Fauxx-Desktop.
//! local.`), and the pairing payload carries that same name as its connection
//! hint. Resolving it needs a working mDNS resolver on the *other* device, and
//! a phone frequently does not have one for a bare `.local.` host name: Android
//! resolves service *instances* through its discovery API, but a plain hostname
//! lookup does not go through mDNS at all. The phone then holds a peer it
//! cannot dial and reports no route to it, which is what several users hit on a
//! network where nothing was actually wrong.
//!
//! A literal `IP:port` sidesteps every part of that. This module enumerates the
//! host's own interfaces so the desktop can put real addresses in the pairing
//! payload, print them from the CLI, and show them in the GUI for a user typing
//! them into the phone's connect-by-address field.
//!
//! # Ordering, not filtering
//!
//! We cannot know which of our addresses the phone shares a subnet with, so we
//! do not guess and discard: everything plausible is offered, best candidate
//! first, and the far end (or the user) picks. Only addresses that *cannot*
//! carry a LAN connection are dropped outright: loopback (never reachable from
//! another device) and link-local (IPv4 169.254/16 and IPv6 fe80::/10, which
//! need a zone index this payload has no way to carry).
//!
//! What remains is ranked, because the first address shown is the one a user
//! will type:
//!
//! 1. Private IPv4 on a physical interface: the overwhelmingly common case for
//!    a phone and a desktop on the same Wi-Fi.
//! 2. Other IPv4 on a physical interface.
//! 3. IPv6 on a physical interface.
//! 4. Anything on a virtual interface, recognised by name. A container bridge
//!    address is technically a private IPv4 but is never routable from a phone,
//!    and on a developer's machine there are often several. Ranking them last
//!    keeps them out of the way without pretending we know they are useless.
//!
//! Recognising those names takes two lists, because `if_addrs` reports a
//! different KIND of string per platform: Unix gives the kernel device name
//! (`docker0`, `wg0`), Windows gives the adapter's prose friendly name
//! ("VirtualBox Host-Only Network"). See the `VIRTUAL_IFACE_PREFIXES` and
//! `VIRTUAL_IFACE_FRAGMENTS` tables in this module.

use std::net::IpAddr;

/// A dialable endpoint on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    /// The interface address.
    pub ip: IpAddr,
    /// The interface this address belongs to (`wlan0`, `docker0`, ...).
    pub interface: String,
    /// Whether [`interface`](Self::interface) looks like a virtual or container
    /// interface rather than a physical one.
    pub virtual_interface: bool,
}

impl LocalEndpoint {
    /// Render as `IP:port`, bracketing IPv6 so the result parses back as a
    /// [`SocketAddr`](std::net::SocketAddr).
    pub fn to_socket_string(&self, port: u16) -> String {
        match self.ip {
            IpAddr::V4(v4) => format!("{v4}:{port}"),
            IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        }
    }
}

/// Interface-name prefixes that indicate a virtual, container, or tunnel
/// interface rather than a physical LAN one.
const VIRTUAL_IFACE_PREFIXES: &[&str] = &[
    "docker",
    "br-",
    "veth",
    "virbr",
    "vboxnet",
    "tun",
    "tap",
    "wg",
    "zt",
    "tailscale",
    "utun",
    "ham",
    "cni",
    "flannel",
    "kube",
];

/// Windows adapter FRIENDLY-NAME fragments that indicate the same thing as
/// [`VIRTUAL_IFACE_PREFIXES`].
///
/// This second list exists because `if_addrs` does not report the same kind of
/// string on every platform: on Unix `Interface::name` is the kernel device
/// name, but on Windows it is the adapter's *friendly name*, which is prose. A
/// VirtualBox host-only adapter is the `docker0` of that platform, but it
/// arrives as "VirtualBox Host-Only Network", which matches none of the
/// prefixes above (`vboxnet` is the Linux and macOS name; `virbr` diverges from
/// `virtualbox` at the fourth byte). Without this list every hypervisor adapter
/// on a Windows host is classified physical, ranks as ordinary private IPv4, and
/// competes with the real Wi-Fi address for the pairing payload's three slots,
/// which on a developer machine pushes the one reachable address out of the QR
/// entirely.
///
/// Matched as a case-insensitive SUBSTRING, because friendly names carry
/// manufacturer prefixes and "#2"-style suffixes around the meaningful word.
const VIRTUAL_IFACE_FRAGMENTS: &[&str] = &[
    "virtualbox",
    "vmware",
    "vmnet",
    "hyper-v",
    "vethernet",
    "loopback pseudo",
    "bluetooth network",
    "tap-windows",
    "tailscale",
    "wireguard",
    "zerotier",
    "npcap",
    "virtual adapter",
    "virtual ethernet",
];

/// Whether an interface looks virtual rather than physical.
///
/// Checks the Unix device-name prefixes and the Windows friendly-name fragments
/// on every platform rather than behind a `cfg`: the cost is a few string
/// compares, and a misclassification here silently pushes the one address a
/// phone can actually reach out of the pairing QR.
fn is_virtual_interface(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIRTUAL_IFACE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || VIRTUAL_IFACE_FRAGMENTS
            .iter()
            .any(|fragment| lower.contains(fragment))
}

/// Whether an address can never carry a connection from another device on the
/// LAN, and so should not be offered at all.
fn is_undialable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local() || v4.is_unspecified(),
        // `is_unicast_link_local` is fe80::/10: it needs a scope/zone index to
        // be dialable, which a pairing payload cannot carry.
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || is_v6_link_local(v6),
    }
}

/// fe80::/10. Hand-rolled because `Ipv6Addr::is_unicast_link_local` is unstable.
fn is_v6_link_local(v6: &std::net::Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

/// Whether an IPv4 address is in a private (RFC 1918) range.
fn is_private_v4(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.is_private())
}

/// Sort rank; lower sorts first. See the module docs for the rationale.
fn rank(endpoint: &LocalEndpoint) -> u8 {
    match (
        endpoint.virtual_interface,
        is_private_v4(&endpoint.ip),
        endpoint.ip.is_ipv4(),
    ) {
        (false, true, _) => 0,
        (false, false, true) => 1,
        (false, false, false) => 2,
        (true, _, _) => 3,
    }
}

/// Every address on this host that another device on the LAN could dial, best
/// candidate first.
///
/// Returns an empty vector if the interfaces cannot be enumerated; callers
/// treat that as "no hint available" and fall back to the mDNS host name, which
/// is the behaviour that predates this module.
pub fn local_endpoints() -> Vec<LocalEndpoint> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        tracing::warn!("LAN sync: could not enumerate local interfaces; no address hints offered");
        return Vec::new();
    };
    from_interfaces(
        interfaces
            .into_iter()
            .map(|iface| (iface.name.clone(), iface.ip())),
    )
}

/// The ordering/filtering core, over an arbitrary `(interface, ip)` sequence so
/// it can be tested without touching the host's real network configuration.
fn from_interfaces(addrs: impl Iterator<Item = (String, IpAddr)>) -> Vec<LocalEndpoint> {
    let mut endpoints: Vec<LocalEndpoint> = addrs
        .filter(|(_, ip)| !is_undialable(ip))
        .map(|(name, ip)| LocalEndpoint {
            ip,
            virtual_interface: is_virtual_interface(&name),
            interface: name,
        })
        .collect();

    // Deduplicate: the same address can appear on more than one interface entry.
    endpoints.sort_by(|a, b| a.ip.cmp(&b.ip).then_with(|| a.interface.cmp(&b.interface)));
    endpoints.dedup_by(|a, b| a.ip == b.ip);

    // Stable sort by rank, so the within-rank address order stays deterministic
    // (it is the numeric order established above). A user reading two addresses
    // off the screen should see the same two, in the same order, every time.
    endpoints.sort_by_key(rank);
    endpoints
}

/// Every dialable endpoint as an `IP:port` string, best candidate first, capped
/// at `limit`.
///
/// This is the *display* list: it includes virtual-interface addresses (ranked
/// last) because a user staring at a failed pairing may genuinely need to try
/// one. For the addresses that ride in the pairing payload, use
/// [`pairing_socket_strings`].
pub fn local_socket_strings(port: u16, limit: usize) -> Vec<String> {
    local_endpoints()
        .into_iter()
        .take(limit)
        .map(|endpoint| endpoint.to_socket_string(port))
        .collect()
}

/// The endpoints to embed in the pairing payload: physical interfaces only,
/// best candidate first, capped at [`PAIRING_ADDR_LIMIT`].
///
/// Two reasons this is narrower than [`local_socket_strings`]:
///
/// - **A container bridge is never reachable from a phone.** A developer
///   machine can easily carry four or five `172.x` Docker bridge addresses,
///   each of which looks like an ordinary private IPv4. Spending the payload's
///   address slots on them would push the one address that works off the end of
///   the list.
/// - **The payload is a QR code.** Every extra address makes it denser and
///   harder for a phone camera to read, and users on #38 already reported that
///   scanning did not work for them. Trading a scan failure for an address hint
///   would be no trade at all.
///
/// If this host has *only* virtual interfaces, their addresses are offered
/// rather than sending nothing: an unlikely guess beats no hint.
pub fn pairing_socket_strings(port: u16) -> Vec<String> {
    select_for_pairing(&local_endpoints(), PAIRING_ADDR_LIMIT)
        .into_iter()
        .map(|endpoint| endpoint.to_socket_string(port))
        .collect()
}

/// The selection half of [`pairing_socket_strings`], over an explicit list so
/// it can be tested without touching the host's real network configuration.
fn select_for_pairing(all: &[LocalEndpoint], limit: usize) -> Vec<&LocalEndpoint> {
    let physical: Vec<&LocalEndpoint> = all.iter().filter(|e| !e.virtual_interface).collect();
    if physical.is_empty() {
        all.iter().take(limit).collect()
    } else {
        physical.into_iter().take(limit).collect()
    }
}

/// How many addresses ride in the pairing payload. See
/// [`pairing_socket_strings`].
pub const PAIRING_ADDR_LIMIT: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        match s.parse() {
            Ok(addr) => addr,
            Err(e) => panic!("test address {s:?} must parse: {e}"),
        }
    }

    fn endpoints(pairs: &[(&str, &str)]) -> Vec<LocalEndpoint> {
        from_interfaces(
            pairs
                .iter()
                .map(|(iface, addr)| ((*iface).to_string(), ip(addr))),
        )
    }

    #[test]
    fn drops_addresses_that_cannot_be_dialed_from_another_device() {
        let got = endpoints(&[
            ("lo", "127.0.0.1"),
            ("lo", "::1"),
            // A 169.254 address means DHCP failed; nothing routes to it.
            ("wlan0", "169.254.10.4"),
            // fe80::/10 needs a zone index the payload cannot carry.
            ("wlan0", "fe80::1c2f:aaff:fe11:2233"),
            ("wlan0", "192.168.1.50"),
        ]);
        assert_eq!(got.len(), 1, "only the real LAN address survives: {got:?}");
        assert_eq!(got[0].ip, ip("192.168.1.50"));
    }

    /// The #38 case: a developer machine where Docker's bridge is also a
    /// private IPv4, so "first private address" alone would offer an address no
    /// phone can ever reach.
    #[test]
    fn ranks_the_real_lan_address_above_a_container_bridge() {
        let got = endpoints(&[
            ("docker0", "172.17.0.1"),
            ("br-9f3c1a", "172.18.0.1"),
            ("wlan0", "192.168.1.50"),
        ]);
        assert_eq!(
            got[0].ip,
            ip("192.168.1.50"),
            "the physical interface must be offered first, got {got:?}"
        );
        assert!(got[0].interface == "wlan0");
        // The bridges are still offered, just last: we do not claim to know
        // they are useless, only that they are worse guesses.
        assert_eq!(got.len(), 3);
        assert!(got[1..].iter().all(|e| e.virtual_interface));
    }

    #[test]
    fn ranks_private_ipv4_above_public_ipv4_and_ipv6() {
        let got = endpoints(&[
            ("eth0", "2001:db8::5"),
            ("eth0", "203.0.113.7"),
            ("eth0", "10.0.0.9"),
        ]);
        let order: Vec<IpAddr> = got.iter().map(|e| e.ip).collect();
        assert_eq!(
            order,
            vec![ip("10.0.0.9"), ip("203.0.113.7"), ip("2001:db8::5")]
        );
    }

    #[test]
    fn a_vpn_or_tunnel_interface_ranks_last() {
        let got = endpoints(&[("wg0", "10.8.0.2"), ("eth0", "192.168.1.50")]);
        assert_eq!(got[0].interface, "eth0");
        assert!(got[1].virtual_interface, "wg0 must be flagged virtual");
    }

    #[test]
    fn the_same_address_on_two_interfaces_is_offered_once() {
        let got = endpoints(&[("eth0", "192.168.1.50"), ("eth0:1", "192.168.1.50")]);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn ipv6_is_bracketed_so_the_result_parses_as_a_socket_address() {
        let endpoint = LocalEndpoint {
            ip: ip("2001:db8::5"),
            interface: "eth0".to_string(),
            virtual_interface: false,
        };
        let rendered = endpoint.to_socket_string(45999);
        assert_eq!(rendered, "[2001:db8::5]:45999");
        assert!(
            rendered.parse::<std::net::SocketAddr>().is_ok(),
            "the rendered endpoint must parse back: {rendered}"
        );
    }

    #[test]
    fn ipv4_renders_as_a_parseable_socket_address() {
        let endpoint = LocalEndpoint {
            ip: ip("192.168.1.50"),
            interface: "wlan0".to_string(),
            virtual_interface: false,
        };
        let rendered = endpoint.to_socket_string(45999);
        assert_eq!(rendered, "192.168.1.50:45999");
        assert!(rendered.parse::<std::net::SocketAddr>().is_ok());
    }

    #[test]
    fn ordering_is_stable_across_calls() {
        let pairs = [
            ("docker0", "172.17.0.1"),
            ("eth0", "192.168.1.50"),
            ("wlan0", "192.168.1.51"),
        ];
        assert_eq!(endpoints(&pairs), endpoints(&pairs));
    }

    #[test]
    fn no_interfaces_yields_no_hints_rather_than_an_error() {
        assert!(from_interfaces(std::iter::empty()).is_empty());
    }

    /// The real machine this was developed on carries four Docker bridges
    /// alongside one Wi-Fi address. Without physical-first selection the payload
    /// would spend two of its three slots on bridges no phone can reach.
    #[test]
    fn the_pairing_payload_spends_no_slot_on_a_container_bridge() {
        let all = endpoints(&[
            ("docker0", "172.17.0.1"),
            ("br-9f3c1a", "172.18.0.1"),
            ("br-2ab4d0", "172.19.0.1"),
            ("br-77c1e9", "172.20.0.1"),
            ("wlan0", "192.168.13.13"),
        ]);
        let chosen = select_for_pairing(&all, PAIRING_ADDR_LIMIT);
        assert_eq!(
            chosen.len(),
            1,
            "only the physical address belongs in the payload, got {chosen:?}"
        );
        assert_eq!(chosen[0].ip, ip("192.168.13.13"));
    }

    #[test]
    fn the_pairing_payload_carries_several_physical_addresses_when_they_exist() {
        let all = endpoints(&[
            ("eth0", "192.168.1.50"),
            ("wlan0", "192.168.1.51"),
            ("docker0", "172.17.0.1"),
        ]);
        let chosen = select_for_pairing(&all, PAIRING_ADDR_LIMIT);
        assert_eq!(chosen.len(), 2);
        assert!(chosen.iter().all(|e| !e.virtual_interface));
    }

    #[test]
    fn the_pairing_payload_respects_the_qr_size_cap() {
        let all = endpoints(&[
            ("eth0", "192.168.1.50"),
            ("eth1", "192.168.1.51"),
            ("eth2", "192.168.1.52"),
            ("eth3", "192.168.1.53"),
        ]);
        assert_eq!(
            select_for_pairing(&all, PAIRING_ADDR_LIMIT).len(),
            PAIRING_ADDR_LIMIT
        );
    }

    /// A host with nothing but a VPN tunnel still gets a hint: an unlikely
    /// guess beats sending none at all.
    #[test]
    fn a_host_with_only_virtual_interfaces_still_offers_something() {
        let all = endpoints(&[("wg0", "10.8.0.2")]);
        let chosen = select_for_pairing(&all, PAIRING_ADDR_LIMIT);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].ip, ip("10.8.0.2"));
    }

    /// The #4 finding from the adversarial sweep. On Windows `if_addrs` reports
    /// the adapter FRIENDLY NAME, not a kernel device name, so the Unix prefix
    /// list never fired and every hypervisor adapter looked physical. With
    /// VirtualBox and VMware installed that is three rank-0 private IPv4
    /// addresses ahead of the real Wi-Fi one, which then falls off the end of
    /// the three-slot pairing payload entirely.
    #[test]
    fn windows_hypervisor_adapters_do_not_take_the_pairing_slots() {
        let all = endpoints(&[
            ("VirtualBox Host-Only Network", "192.168.56.1"),
            ("VMware Network Adapter VMnet1", "192.168.40.1"),
            ("VMware Network Adapter VMnet8", "192.168.75.1"),
            ("Wi-Fi", "192.168.100.20"),
        ]);
        assert_eq!(
            all[0].ip,
            ip("192.168.100.20"),
            "the real Wi-Fi address must rank first, got {all:?}"
        );
        let chosen = select_for_pairing(&all, PAIRING_ADDR_LIMIT);
        assert_eq!(
            chosen.len(),
            1,
            "only the physical adapter belongs in the QR"
        );
        assert_eq!(chosen[0].ip, ip("192.168.100.20"));
    }

    #[test]
    fn windows_friendly_names_for_tunnels_are_recognised() {
        for name in [
            "VirtualBox Host-Only Network",
            "VMware Network Adapter VMnet8",
            "Hyper-V Virtual Ethernet Adapter",
            "vEthernet (Default Switch)",
            "TAP-Windows Adapter V9",
            "Tailscale Tunnel",
            "ZeroTier One [abcdef]",
        ] {
            assert!(
                is_virtual_interface(name),
                "{name:?} must be classified virtual"
            );
        }
    }

    /// The inverse: ordinary physical adapters must NOT be demoted, or the bug
    /// simply changes direction and the real address gets pushed down instead.
    #[test]
    fn ordinary_physical_adapter_names_stay_physical() {
        for name in [
            "Wi-Fi",
            "Ethernet",
            "Ethernet 2",
            "Intel(R) Wi-Fi 6 AX201 160MHz",
            "Realtek PCIe GbE Family Controller",
            "en0",
            "enp5s0",
            "wlan0",
        ] {
            assert!(!is_virtual_interface(name), "{name:?} must stay physical");
        }
    }

    #[test]
    fn virtual_interface_detection_is_case_insensitive() {
        assert!(is_virtual_interface("Docker0"));
        assert!(is_virtual_interface("veth1a2b"));
        assert!(!is_virtual_interface("enp5s0"));
        assert!(!is_virtual_interface("wlan0"));
    }
}
