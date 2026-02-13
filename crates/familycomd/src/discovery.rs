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
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
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
    ///
    /// # Returns
    ///
    /// A tuple of `(DiscoveryService, mpsc::Receiver<DiscoveryEvent>)`.
    /// The receiver emits events whenever peers appear or disappear.
    pub fn new(
        peer_id: PeerId,
        display_name: &str,
        tcp_port: u16,
    ) -> Result<(Self, mpsc::Receiver<DiscoveryEvent>), DiscoveryError> {
        // Create the mDNS daemon. This starts a background thread that
        // handles all multicast networking.
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Mdns(e.to_string()))?;

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
            "",             // Empty = use all available interfaces
            tcp_port,
            properties,
        )
        .map_err(|e| DiscoveryError::Registration(e.to_string()))?;

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

                    // Build the list of reachable addresses (IP:port)
                    let port = info.get_port();
                    let addresses: Vec<String> = info
                        .get_addresses()
                        .iter()
                        .map(|addr| format!("{addr}:{port}"))
                        .collect();

                    if addresses.is_empty() {
                        warn!(peer_id = %peer_id, "peer has no addresses, skipping");
                        continue;
                    }

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
                    // Unfortunately, the removed event doesn't include TXT records,
                    // so we extract what we can from the service name.
                    info!(service = fullname, "peer service removed");

                    // We can't reliably extract the peer_id from just the fullname.
                    // The daemon will need to match this by service name or handle
                    // it via periodic peer health checks.
                    // For now, we emit PeerLost with a PeerId derived from the fullname.
                    // The daemon can cross-reference with its known peers.
                    let peer_id = PeerId::new(fullname);
                    if event_tx.blocking_send(DiscoveryEvent::PeerLost(peer_id)).is_err() {
                        break;
                    }
                }

                ServiceEvent::SearchStarted(service_type) => {
                    debug!(service_type, "mDNS search started");
                }

                ServiceEvent::SearchStopped(service_type) => {
                    debug!(service_type, "mDNS search stopped");
                }

                // Other events (ServiceFound is an intermediate step before ServiceResolved)
                _ => {}
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
        if let Err(e) = self.daemon.unregister(&self.our_service_fullname) {
            error!(error = %e, "failed to unregister mDNS service");
        }
        if let Err(e) = self.daemon.shutdown() {
            error!(error = %e, "failed to shutdown mDNS daemon");
        }
    }

    /// Returns our peer ID.
    pub fn peer_id(&self) -> &PeerId {
        &self.our_peer_id
    }
}
