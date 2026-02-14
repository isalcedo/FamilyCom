//! mDNS peer discovery service.
//!
//! Uses the `mdns-sd` crate to both **advertise** this FamilyCom instance
//! and **discover** other instances on the local network.
//!
//! # How mDNS Works (simplified)
//!
//! mDNS (Multicast DNS) lets devices on a local network find each other
//! without a central server. When our daemon starts, it:
//!
//! 1. **Registers** a service: `{display_name}._familycom._tcp.local.`
//!    with TXT records containing our `peer_id` and `display_name`.
//! 2. **Browses** for other `_familycom._tcp.local.` services on the network.
//!
//! When another FamilyCom instance starts (or stops), we get notified
//! through the mDNS browsing mechanism.
//!
//! # Service Type
//!
//! We use `_familycom._tcp.local.` as our service type. The underscore
//! prefix is an mDNS convention for service types. The `._tcp` suffix
//! indicates we use TCP for the actual communication.

use familycom_core::types::{PeerId, PeerInfo, Timestamp};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// The mDNS service type we register and browse for.
/// All FamilyCom instances on the LAN use this same service type.
const SERVICE_TYPE: &str = "_familycom._tcp.local.";

/// Events emitted by the discovery service.
///
/// The daemon's main loop receives these via a channel and updates
/// its internal state (peer list, database, UI notifications).
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// A new peer was found on the network (or an existing peer updated its info).
    PeerFound(PeerInfo),
    /// A peer left the network (mDNS goodbye or timeout).
    PeerLost(PeerId),
}

/// Errors that can occur in the discovery service.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mDNS daemon error: {0}")]
    Mdns(String),
    #[error("failed to register service: {0}")]
    Registration(String),
}

/// Manages mDNS service registration and peer discovery.
///
/// Internally, `mdns-sd` runs its own background thread for multicast
/// networking. This struct provides an async-friendly interface by
/// bridging mDNS events into a tokio mpsc channel.
pub struct DiscoveryService {
    /// The mdns-sd daemon handle. Dropping this stops the background thread.
    daemon: ServiceDaemon,
    /// Our own peer ID, used to filter out self-discovery.
    #[allow(dead_code)]
    our_peer_id: PeerId,
    /// The full service name we registered (needed for unregistration).
    our_service_fullname: String,
}

impl DiscoveryService {
    /// Creates and starts a new discovery service.
    ///
    /// This immediately:
    /// 1. Registers our service on the network
    /// 2. Starts browsing for other FamilyCom services
    /// 3. Spawns a background task that forwards discovery events to the returned channel
    ///
    /// # Arguments
    ///
    /// * `peer_id` - Our unique peer identifier
    /// * `display_name` - Our human-readable name (shown to other peers)
    /// * `tcp_port` - The TCP port our message server is listening on
    /// * `network_interface` - Optional interface name override (e.g. "enp5s0").
    ///   If `None`, auto-detects the default-route interface via `netdev`.
    ///
    /// # Returns
    ///
    /// A tuple of `(DiscoveryService, mpsc::Receiver<DiscoveryEvent>)`.
    /// The receiver emits events whenever peers appear or disappear.
    pub fn new(
        peer_id: PeerId,
        display_name: &str,
        tcp_port: u16,
        network_interface: Option<&str>,
    ) -> Result<(Self, mpsc::Receiver<DiscoveryEvent>), DiscoveryError> {
        // Create the mDNS daemon. This starts a background thread that
        // handles all multicast networking.
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Mdns(e.to_string()))?;

        // Determine which network interface to use for mDNS.
        // Without filtering, mDNS probes on ALL interfaces (including Docker
        // bridges, VPNs, etc.) which causes conflicts and unreachable addresses.
        let iface_name = match network_interface {
            Some(name) => name.to_string(),
            None => {
                // Auto-detect: use the interface that holds the default route
                netdev::get_default_interface()
                    .map(|iface| iface.name)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "could not detect default network interface, using all");
                        String::new()
                    })
            }
        };

        if !iface_name.is_empty() {
            info!(interface = %iface_name, "restricting mDNS to interface");
            daemon
                .disable_interface(IfKind::All)
                .map_err(|e| DiscoveryError::Mdns(e.to_string()))?;
            daemon
                .enable_interface(IfKind::Name(iface_name))
                .map_err(|e| DiscoveryError::Mdns(e.to_string()))?;
            // Disable IPv6 AFTER enabling the named interface. The mdns-sd
            // crate processes interface selections as an ordered list where
            // the last matching rule wins. If we disable IPv6 before the
            // named enable, the enable overrides it (Name matches both v4
            // and v6 addresses). Placing the IPv6 disable last ensures it
            // takes precedence for any IPv6 address on the interface.
            // This is needed because our TCP server binds to 0.0.0.0 (IPv4
            // only), and dual-stack mDNS causes resolution failures between
            // peers (IPv6 link-local addresses lack zone IDs in std::net).
            daemon
                .disable_interface(IfKind::IPv6)
                .map_err(|e| DiscoveryError::Mdns(e.to_string()))?;
        }

        // Build our service info. The service name is a human-readable label
        // (display_name), but the actual identification happens via the TXT
        // records where we store our peer_id.
        //
        // TXT records are key-value pairs attached to an mDNS service.
        // We use them to transmit our peer_id without relying on the
        // service instance name (which may not be unique if two people
        // choose the same display name).
        let mut properties = HashMap::new();
        properties.insert("peer_id".to_string(), peer_id.to_string());
        properties.insert("display_name".to_string(), display_name.to_string());

        // The hostname for our service. We use "_" as placeholder since
        // mdns-sd will use the actual local hostname.
        let host = format!("{}.local.", hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "familycom".to_string()));

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            display_name,   // Instance name (human-readable)
            &host,
            "",             // No explicit addrs — addr_auto lets the lib find them
            tcp_port,
            properties,
        )
        .map_err(|e| DiscoveryError::Registration(e.to_string()))?
        .enable_addr_auto();

        // Save the full service name for later unregistration
        let fullname = service_info.get_fullname().to_string();

        // Register our service on the network.
        // Other FamilyCom instances browsing for _familycom._tcp will discover us.
        daemon
            .register(service_info)
            .map_err(|e| DiscoveryError::Registration(e.to_string()))?;

        info!(
            peer_id = %peer_id,
            display_name,
            tcp_port,
            "registered mDNS service"
        );

        // Start browsing for other FamilyCom services
        let browse_receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| DiscoveryError::Mdns(e.to_string()))?;

        // Create a channel for forwarding discovery events to the daemon's main loop
        let (event_tx, event_rx) = mpsc::channel::<DiscoveryEvent>(64);

        // Clone the peer_id for the background task
        let our_peer_id = peer_id.clone();

        // Spawn a background task that converts mdns-sd events into our DiscoveryEvents.
        // We use tokio::task::spawn_blocking because mdns-sd's receiver uses
        // blocking recv(), not async.
        let our_peer_id_clone = our_peer_id.clone();
        tokio::task::spawn_blocking(move || {
            Self::browse_loop(browse_receiver, event_tx, &our_peer_id_clone);
        });

        let service = Self {
            daemon,
            our_peer_id: peer_id,
            our_service_fullname: fullname,
        };

        Ok((service, event_rx))
    }

    /// Background loop that receives mDNS browse events and forwards them
    /// as `DiscoveryEvent`s through the channel.
    ///
    /// This runs on a blocking thread because `mdns-sd` uses synchronous channels.
    /// It will exit when either the mdns-sd browse receiver is closed (daemon shutdown)
    /// or the event sender is closed (main loop dropped the receiver).
    fn browse_loop(
        browse_receiver: mdns_sd::Receiver<ServiceEvent>,
        event_tx: mpsc::Sender<DiscoveryEvent>,
        our_peer_id: &PeerId,
    ) {
        // Track mDNS fullname → PeerId so we can emit correct PeerLost events.
        // ServiceRemoved only gives us the fullname (e.g. "ChuiMachine._familycom._tcp.local."),
        // not the TXT records with the UUID peer_id. This map lets us look it up.
        let mut fullname_to_peer_id: HashMap<String, PeerId> = HashMap::new();

        // recv() blocks until an event is available or the channel is closed
        while let Ok(event) = browse_receiver.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    // A service was fully resolved — we know its name, IP, port, TXT records.
                    // Extract the peer_id from the TXT records.
                    let properties = info.get_properties();
                    let peer_id_str = match properties.get_property_val_str("peer_id") {
                        Some(id) => id.to_string(),
                        None => {
                            warn!(
                                service = info.get_fullname(),
                                "discovered service without peer_id TXT record, ignoring"
                            );
                            continue;
                        }
                    };

                    let peer_id = PeerId::new(&peer_id_str);

                    // Skip ourselves — we don't want to show up in our own peer list
                    if peer_id == *our_peer_id {
                        debug!("discovered ourselves, skipping");
                        continue;
                    }

                    let display_name = properties
                        .get_property_val_str("display_name")
                        .unwrap_or("Unknown")
                        .to_string();

                    // Build the list of reachable addresses (IP:port).
                    // Filter out IPv6 link-local addresses (fe80::/10) because
                    // std::net doesn't support zone IDs (%iface) and our TCP
                    // server only binds IPv4 anyway.
                    let port = info.get_port();
                    let addresses: Vec<String> = info
                        .get_addresses()
                        .iter()
                        .filter(|addr| !is_ipv6_link_local(addr))
                        .map(|addr| format!("{addr}:{port}"))
                        .collect();

                    if addresses.is_empty() {
                        warn!(peer_id = %peer_id, "peer has no addresses, skipping");
                        continue;
                    }

                    // Remember the fullname → peer_id mapping for ServiceRemoved
                    fullname_to_peer_id.insert(
                        info.get_fullname().to_string(),
                        peer_id.clone(),
                    );

                    let peer_info = PeerInfo {
                        id: peer_id.clone(),
                        display_name: display_name.clone(),
                        addresses: addresses.clone(),
                        last_seen_at: Timestamp::now(),
                        online: true,
                    };

                    info!(
                        peer_id = %peer_id,
                        display_name,
                        ?addresses,
                        "peer found"
                    );

                    // Send the event. If the receiver is dropped, we exit the loop.
                    if event_tx.blocking_send(DiscoveryEvent::PeerFound(peer_info)).is_err() {
                        debug!("event channel closed, stopping browse loop");
                        break;
                    }
                }

                ServiceEvent::ServiceRemoved(_, fullname) => {
                    // A service was removed (peer went offline or unregistered).
                    // Look up the real PeerId from our fullname map so the daemon
                    // can correctly remove the peer from its online_peers.
                    if let Some(peer_id) = fullname_to_peer_id.remove(&fullname) {
                        info!(
                            peer_id = %peer_id,
                            service = fullname,
                            "peer service removed"
                        );
                        if event_tx.blocking_send(DiscoveryEvent::PeerLost(peer_id)).is_err() {
                            break;
                        }
                    } else {
                        debug!(
                            service = fullname,
                            "service removed for unknown fullname, ignoring"
                        );
                    }
                }

                ServiceEvent::ServiceFound(service_type, fullname) => {
                    // Intermediate step: the library found a PTR record pointing
                    // to this service. Resolution (SRV/TXT/A queries) follows.
                    info!(
                        service_type,
                        fullname,
                        "mDNS service found (pending resolution)"
                    );
                }

                ServiceEvent::SearchStarted(service_type) => {
                    debug!(service_type, "mDNS search started");
                }

                ServiceEvent::SearchStopped(service_type) => {
                    debug!(service_type, "mDNS search stopped");
                }
            }
        }

        debug!("browse loop exited");
    }

    /// Unregisters our service from the network and shuts down the mDNS daemon.
    ///
    /// Call this during graceful shutdown so other peers know we're going offline
    /// immediately, rather than waiting for the mDNS TTL to expire.
    pub fn shutdown(self) {
        info!("unregistering mDNS service");

        // unregister() and shutdown() both return Receivers for the operation
        // status. We must .recv() on them to wait for completion — dropping
        // the receiver immediately would cause mdns-sd to log "failed to send
        // response: sending on a closed channel" errors.
        match self.daemon.unregister(&self.our_service_fullname) {
            Ok(receiver) => {
                if let Err(e) = receiver.recv() {
                    debug!(error = %e, "did not receive unregister confirmation");
                }
            }
            Err(e) => {
                error!(error = %e, "failed to unregister mDNS service");
            }
        }

        match self.daemon.shutdown() {
            Ok(receiver) => {
                if let Err(e) = receiver.recv() {
                    debug!(error = %e, "did not receive shutdown confirmation");
                }
            }
            Err(e) => {
                error!(error = %e, "failed to shutdown mDNS daemon");
            }
        }
    }

    /// Returns our peer ID.
    #[allow(dead_code)]
    pub fn peer_id(&self) -> &PeerId {
        &self.our_peer_id
    }
}

/// Returns `true` if the address is an IPv6 link-local address (fe80::/10).
///
/// These addresses require a zone ID (`%iface`) that `std::net` doesn't support,
/// so they always fail when used for TCP connections.
fn is_ipv6_link_local(addr: &std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        std::net::IpAddr::V4(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn test_is_ipv6_link_local() {
        // IPv6 link-local addresses (fe80::/10) — should be filtered
        let ll: IpAddr = "fe80::1".parse().unwrap();
        assert!(is_ipv6_link_local(&ll));

        let ll2: IpAddr = "fe80::abcd:ef01:2345:6789".parse().unwrap();
        assert!(is_ipv6_link_local(&ll2));

        // IPv6 non-link-local — should NOT be filtered
        let global: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(!is_ipv6_link_local(&global));

        let loopback: IpAddr = "::1".parse().unwrap();
        assert!(!is_ipv6_link_local(&loopback));

        // IPv4 — should never be filtered
        let v4: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(!is_ipv6_link_local(&v4));

        let v4_ll: IpAddr = "169.254.1.1".parse().unwrap();
        assert!(!is_ipv6_link_local(&v4_ll));
    }
}
