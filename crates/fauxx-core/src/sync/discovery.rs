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

//! Concrete mDNS discovery (the real LAN wiring).
//!
//! This is the live counterpart to the [`Discovery`]
//! trait, backed by `mdns-sd`. It advertises this device under the
//! [`SERVICE_TYPE`] with a TXT record carrying the
//! protocol version, the public-key fingerprint, and the full base64url public
//! key, and it browses for the same service type, draining resolved peers into
//! a shared snapshot.
//!
//! No internet access is involved: mDNS is link-local multicast only. CI has no
//! multicast, so this type is never exercised by the hermetic unit tests (those
//! use the in-memory double); a live smoke test would be `#[ignore]`d.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::{CoreError, Result};
use crate::sync::peer::DiscoveredPeer;
use crate::sync::transport::Discovery;
use crate::sync::wire::{
    SERVICE_TYPE, SYNC_PROTOCOL_VERSION, TXT_KEY_FINGERPRINT, TXT_KEY_PUBKEY, TXT_KEY_VERSION,
};

/// How this device advertises itself over mDNS.
#[derive(Debug, Clone)]
pub struct AdvertisedDevice {
    /// The mDNS instance name (human-readable device name).
    pub instance_name: String,
    /// The host name to advertise (e.g. `desktop.local.`).
    pub host_name: String,
    /// The sync port.
    pub port: u16,
    /// This device's public key, base64url (no padding), for the TXT record.
    pub public_key_b64: String,
    /// This device's public-key fingerprint, for the TXT record.
    pub fingerprint: String,
}

/// Live mDNS discovery backed by `mdns-sd`.
///
/// Holds the daemon and the browse-event receiver. Discovered peers accumulate
/// in a map keyed by fullname (so repeated resolutions update in place); the
/// receiver is drained non-blockingly on each [`discovered_peers`](Self::discovered_peers)
/// poll rather than on a long-lived background thread. That keeps the daemon's
/// blocking browse channel off the runtime's blocking pool, so process shutdown
/// never has to wait on (or abort) an in-flight `recv`.
pub struct MdnsDiscovery {
    device: AdvertisedDevice,
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    /// The browse-event receiver, `Some` once browsing has started. Dropping it
    /// (on [`stop_advertising`](Discovery::stop_advertising)) stops browsing.
    browse: Arc<Mutex<Option<Receiver<ServiceEvent>>>>,
}

impl std::fmt::Debug for MdnsDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MdnsDiscovery")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl MdnsDiscovery {
    /// Create a discovery handle bound to a freshly created mDNS daemon. Does
    /// not yet advertise or browse; call [`advertise`](Self::advertise) and
    /// [`start_browsing`](Self::start_browsing) for that.
    pub fn new(device: AdvertisedDevice) -> Result<Self> {
        let daemon =
            ServiceDaemon::new().map_err(|e| CoreError::Sync(format!("mDNS daemon: {e}")))?;
        Ok(Self {
            device,
            daemon,
            peers: Arc::new(Mutex::new(HashMap::new())),
            browse: Arc::new(Mutex::new(None)),
        })
    }

    /// Build the `ServiceInfo` describing this device, including the TXT record.
    fn service_info(&self) -> Result<ServiceInfo> {
        let mut props: HashMap<String, String> = HashMap::new();
        props.insert(
            TXT_KEY_VERSION.to_string(),
            SYNC_PROTOCOL_VERSION.to_string(),
        );
        props.insert(
            TXT_KEY_FINGERPRINT.to_string(),
            self.device.fingerprint.clone(),
        );
        props.insert(
            TXT_KEY_PUBKEY.to_string(),
            self.device.public_key_b64.clone(),
        );

        // An empty address set tells mdns-sd to auto-detect this host's
        // routable interface addresses.
        ServiceInfo::new(
            SERVICE_TYPE,
            &self.device.instance_name,
            &self.device.host_name,
            "",
            self.device.port,
            props,
        )
        .map(|info| info.enable_addr_auto())
        .map_err(|e| CoreError::Sync(format!("mDNS service info: {e}")))
    }

    /// Start browsing for peers: open the daemon's browse channel and stash the
    /// receiver. Idempotent: a second call is a no-op while a browse is active.
    /// Resolved peers are drained from the receiver lazily by
    /// [`discovered_peers`](Discovery::discovered_peers), not on a background
    /// thread, so there is no blocking task to outlive the process.
    pub fn start_browsing(&self) -> Result<()> {
        let mut guard = self
            .browse
            .lock()
            .map_err(|_| CoreError::Sync("browse lock poisoned".to_string()))?;
        if guard.is_some() {
            return Ok(());
        }
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| CoreError::Sync(format!("mDNS browse: {e}")))?;
        *guard = Some(receiver);
        Ok(())
    }

    /// Drain every browse event currently queued (non-blocking) into the peer
    /// snapshot. A no-op when browsing has not started. Our own advertisement is
    /// skipped by exact instance label (a prefix match would wrongly hide a peer
    /// whose name merely extends ours, e.g. "Desktop" vs "Desktop-2").
    fn drain_browse_events(&self) -> Result<()> {
        let browse = self
            .browse
            .lock()
            .map_err(|_| CoreError::Sync("browse lock poisoned".to_string()))?;
        let Some(receiver) = browse.as_ref() else {
            return Ok(());
        };
        let mut peers = self
            .peers
            .lock()
            .map_err(|_| CoreError::Sync("peer map lock poisoned".to_string()))?;
        while let Ok(event) = receiver.try_recv() {
            if let ServiceEvent::ServiceResolved(resolved) = event {
                if instance_label(&resolved.fullname) == self.device.instance_name {
                    continue;
                }
                peers.insert(resolved.fullname.clone(), peer_from_resolved(&resolved));
            }
        }
        Ok(())
    }
}

/// Convert a resolved mDNS service into a [`DiscoveredPeer`], reading the TXT
/// record for the protocol version, fingerprint, and public key.
fn peer_from_resolved(resolved: &mdns_sd::ResolvedService) -> DiscoveredPeer {
    let addresses: Vec<String> = resolved
        .addresses
        .iter()
        .map(|ip| format!("{}:{}", ip.to_ip_addr(), resolved.port))
        .collect();

    let txt = resolved.get_properties();
    let protocol_version = txt
        .get_property_val_str(TXT_KEY_VERSION)
        .and_then(|s| s.parse::<u16>().ok());
    let fingerprint = txt
        .get_property_val_str(TXT_KEY_FINGERPRINT)
        .map(str::to_string);
    let public_key = txt.get_property_val_str(TXT_KEY_PUBKEY).map(str::to_string);

    DiscoveredPeer {
        name: instance_label(&resolved.fullname),
        host: Some(resolved.host.clone()),
        addresses,
        port: resolved.port,
        fingerprint,
        public_key,
        protocol_version,
    }
}

/// Extract the human-readable instance label from a full mDNS name
/// (`instance._fauxx-sync._tcp.local.` -> `instance`).
fn instance_label(fullname: &str) -> String {
    fullname
        .strip_suffix(&format!(".{SERVICE_TYPE}"))
        .unwrap_or(fullname)
        .to_string()
}

#[async_trait]
impl Discovery for MdnsDiscovery {
    async fn advertise(&self) -> Result<()> {
        let info = self.service_info()?;
        self.daemon
            .register(info)
            .map_err(|e| CoreError::Sync(format!("mDNS register: {e}")))?;
        // Begin browsing alongside advertising so the peer list fills in.
        self.start_browsing()?;
        Ok(())
    }

    async fn stop_advertising(&self) -> Result<()> {
        let fullname = format!("{}.{SERVICE_TYPE}", self.device.instance_name);
        self.daemon
            .unregister(&fullname)
            .map_err(|e| CoreError::Sync(format!("mDNS unregister: {e}")))?;
        // Drop the browse receiver to stop browsing (the daemon stops delivering
        // once no receiver remains).
        if let Ok(mut guard) = self.browse.lock() {
            *guard = None;
        }
        Ok(())
    }

    async fn discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
        // Drain any browse events that arrived since the last poll, then snapshot.
        self.drain_browse_events()?;
        let guard = self
            .peers
            .lock()
            .map_err(|_| CoreError::Sync("peer map lock poisoned".to_string()))?;
        Ok(guard.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> AdvertisedDevice {
        AdvertisedDevice {
            instance_name: "Fauxx-Desktop".to_string(),
            host_name: "desktop.local.".to_string(),
            port: 45_999,
            public_key_b64: "AAAA".to_string(),
            fingerprint: "1a2b:3c4d:5e6f:7081".to_string(),
        }
    }

    #[test]
    fn instance_label_strips_service_suffix() {
        assert_eq!(
            instance_label("My-Phone._fauxx-sync._tcp.local."),
            "My-Phone"
        );
        assert_eq!(instance_label("bare"), "bare");
    }

    // Constructing the daemon and ServiceInfo touches no multicast until
    // register/browse, so this stays hermetic. It exercises the TXT-record
    // assembly path that the phone reads.
    #[test]
    fn service_info_carries_txt_record() -> Result<()> {
        let discovery = match MdnsDiscovery::new(device()) {
            Ok(d) => d,
            // Some sandboxed CI hosts cannot open the mDNS sockets the daemon
            // creates eagerly; skip rather than fail in that case.
            Err(_) => return Ok(()),
        };
        let info = discovery.service_info()?;
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(
            info.get_property_val_str(TXT_KEY_VERSION),
            Some(SYNC_PROTOCOL_VERSION.to_string().as_str())
        );
        assert_eq!(
            info.get_property_val_str(TXT_KEY_FINGERPRINT),
            Some("1a2b:3c4d:5e6f:7081")
        );
        assert_eq!(info.get_property_val_str(TXT_KEY_PUBKEY), Some("AAAA"));
        Ok(())
    }
}
