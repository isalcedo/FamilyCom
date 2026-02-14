//! Central daemon application — coordinates all subsystems.
//!
//! `DaemonApp` is the heart of the daemon. It owns all the shared state
//! and runs the main event loop that ties together:
//!
//! - **mDNS Discovery**: peer found/lost events
//! - **TCP Server**: incoming messages from peers
//! - **IPC Server**: requests from TUI clients
//! - **SQLite Database**: persistent storage
//! - **Broadcast channel**: real-time events to subscribed TUI clients
//!
//! # Event Loop Architecture
//!
//! The main loop uses `tokio::select!` to multiplex over all event sources.
//! This is a common pattern in async Rust for handling multiple concurrent
//! streams of events in a single task:
//!
//! ```text
//! loop {
//!     select! {
//!         discovery_event => update peers, notify TUI clients
//!         incoming_message => save to DB, notify TUI clients
//!         ipc_request => handle and respond
//!     }
//! }
//! ```

use crate::client;
use crate::discovery::DiscoveryEvent;
use crate::ipc_server::IpcRequest;
use crate::server::IncomingMessage;
use familycom_core::config::AppConfig;
use familycom_core::db::Database;
use familycom_core::ipc::{ClientRequest, ServerMessage};
use familycom_core::protocol::PeerMessage;
use familycom_core::types::{Direction, Message, MessageContent, MessageId, PeerId, PeerInfo, Timestamp};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// The main daemon application.
///
/// Holds all shared state and coordinates the subsystems. The `Database`
/// is behind a `Mutex` because rusqlite connections are `!Send` — we
/// access it via `tokio::task::spawn_blocking` when needed from async code,
/// but the simpler approach (since we're single-tasked in the main loop)
/// is to keep it in a Mutex and access it synchronously from the event loop.
pub struct DaemonApp {
    /// SQLite database for persisting messages and peers.
    db: Mutex<Database>,
    /// Our configuration (peer_id, display_name, etc.).
    config: AppConfig,
    /// Currently known online peers (keyed by PeerId).
    /// This is the authoritative source for online status — the DB
    /// stores all known peers, but online status is managed here.
    online_peers: HashMap<PeerId, PeerInfo>,
    /// Broadcast channel for pushing events to subscribed TUI clients.
    event_tx: broadcast::Sender<ServerMessage>,
}

impl DaemonApp {
    /// Creates a new daemon app with the given database and config.
    pub fn new(db: Database, config: AppConfig) -> Self {
        // Broadcast channel with a buffer of 256 events.
        // If a TUI client falls behind by more than 256 events,
        // it will receive a Lagged error and miss some events.
        let (event_tx, _) = broadcast::channel(256);

        Self {
            db: Mutex::new(db),
            config,
            online_peers: HashMap::new(),
            event_tx,
        }
    }

    /// Returns a clone of the broadcast sender (for the IPC server to use).
    pub fn event_sender(&self) -> broadcast::Sender<ServerMessage> {
        self.event_tx.clone()
    }

    /// Runs the main event loop.
    ///
    /// This is the daemon's core — it processes events from all subsystems
    /// until a shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `discovery_rx` - Channel receiving mDNS discovery events
    /// * `message_rx` - Channel receiving incoming TCP messages
    /// * `ipc_rx` - Channel receiving IPC requests from TUI clients
    /// * `shutdown_rx` - Signal to stop the daemon
    pub async fn run(
        &mut self,
        mut discovery_rx: mpsc::Receiver<DiscoveryEvent>,
        mut message_rx: mpsc::Receiver<IncomingMessage>,
        mut ipc_rx: mpsc::Receiver<IpcRequest>,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) {
        info!(
            peer_id = %self.config.peer_id,
            display_name = %self.config.display_name,
            "daemon main loop started"
        );

        loop {
            tokio::select! {
                // Handle mDNS discovery events
                Some(event) = discovery_rx.recv() => {
                    self.handle_discovery_event(event);
                }

                // Handle incoming TCP messages from peers
                Some(incoming) = message_rx.recv() => {
                    self.handle_incoming_message(incoming);
                }

                // Handle IPC requests from TUI clients
                Some(ipc_req) = ipc_rx.recv() => {
                    self.handle_ipc_request(ipc_req).await;
                }

                // Shutdown signal
                _ = shutdown_rx.recv() => {
                    info!("shutdown signal received, stopping daemon");
                    break;
                }
            }
        }
    }

    /// Processes an mDNS discovery event (peer found or lost).
    fn handle_discovery_event(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::PeerFound(peer_info) => {
                info!(
                    peer_id = %peer_info.id,
                    name = %peer_info.display_name,
                    addresses = ?peer_info.addresses,
                    "peer came online"
                );

                // Update our in-memory peer list
                self.online_peers
                    .insert(peer_info.id.clone(), peer_info.clone());

                // Persist to database
                if let Ok(db) = self.db.lock() {
                    if let Err(e) = db.upsert_peer(&peer_info) {
                        error!(error = %e, "failed to save peer to database");
                    }
                }

                // Notify subscribed TUI clients
                let _ = self.event_tx.send(ServerMessage::PeerOnline {
                    peer: peer_info,
                });
            }

            DiscoveryEvent::PeerLost(peer_id) => {
                // The discovery module now maps mDNS fullnames to UUID-based
                // PeerIds, so we can look up directly by key.
                if self.online_peers.remove(&peer_id).is_some() {
                    info!(peer_id = %peer_id, "peer went offline");
                    let _ = self.event_tx.send(ServerMessage::PeerOffline {
                        peer_id,
                    });
                } else {
                    debug!(peer_id = %peer_id, "received PeerLost for unknown peer");
                }
            }
        }
    }

    /// Processes an incoming message received over TCP from a peer.
    fn handle_incoming_message(&mut self, incoming: IncomingMessage) {
        match incoming.message {
            PeerMessage::Chat {
                id,
                sender_id,
                sender_name,
                content,
                timestamp,
            } => {
                info!(
                    message_id = %id,
                    from = %sender_name,
                    "received chat message"
                );

                // Build the message struct
                let message = Message {
                    id: id.clone(),
                    peer_id: sender_id.clone(),
                    direction: Direction::Received,
                    content: content.clone(),
                    timestamp,
                    delivered: true, // We already sent an ACK in the TCP handler
                };

                // Save to database
                if let Ok(db) = self.db.lock() {
                    // Ensure the peer exists in our DB
                    // (they should from mDNS, but just in case)
                    let peer_exists = db.get_peers().ok()
                        .map(|peers| peers.iter().any(|p| p.id == sender_id))
                        .unwrap_or(false);

                    if !peer_exists {
                        let peer_info = PeerInfo {
                            id: sender_id.clone(),
                            display_name: sender_name.clone(),
                            addresses: vec![incoming.from_addr.to_string()],
                            last_seen_at: Timestamp::now(),
                            online: true,
                        };
                        if let Err(e) = db.upsert_peer(&peer_info) {
                            error!(error = %e, "failed to save peer");
                        }
                    }

                    if let Err(e) = db.save_message(&message) {
                        error!(error = %e, "failed to save message to database");
                    }
                }

                // Notify subscribed TUI clients about the new message
                let _ = self.event_tx.send(ServerMessage::NewMessage { message });
            }

            PeerMessage::Ack { message_id } => {
                debug!(message_id = %message_id, "received delivery ACK");

                // Mark the message as delivered in our database
                if let Ok(db) = self.db.lock() {
                    if let Err(e) = db.mark_delivered(&message_id) {
                        error!(error = %e, "failed to mark message as delivered");
                    }
                }

                // Notify TUI clients
                let _ = self.event_tx.send(ServerMessage::MessageDelivered { message_id });
            }

            // Ping/Pong are handled at the TCP connection level, not here
            PeerMessage::Ping | PeerMessage::Pong => {}
        }
    }

    /// Processes an IPC request from a TUI client.
    async fn handle_ipc_request(&mut self, ipc_req: IpcRequest) {
        let IpcRequest {
            request,
            response_tx,
        } = ipc_req;

        let response = match request {
            ClientRequest::ListPeers => self.handle_list_peers(),

            ClientRequest::GetMessages {
                peer_id,
                limit,
                before,
            } => self.handle_get_messages(&peer_id, limit, before),

            ClientRequest::SendMessage { peer_id, content } => {
                self.handle_send_message(&peer_id, &content).await
            }

            ClientRequest::GetConfig => self.handle_get_config(),

            ClientRequest::SetDisplayName { name } => self.handle_set_display_name(&name),

            // Subscribe is handled in the IPC server itself
            ClientRequest::Subscribe => ServerMessage::Ok,
        };

        if response_tx.send(response).await.is_err() {
            debug!("IPC client disconnected before receiving response");
        }
    }

    /// Handles ListPeers: returns all known peers with their online status.
    fn handle_list_peers(&self) -> ServerMessage {
        match self.db.lock() {
            Ok(db) => match db.get_peers() {
                Ok(mut peers) => {
                    // Update online status from our in-memory state
                    for peer in &mut peers {
                        peer.online = self.online_peers.contains_key(&peer.id);
                    }
                    ServerMessage::PeerList { peers }
                }
                Err(e) => ServerMessage::Error {
                    code: "db_error".to_string(),
                    message: format!("failed to fetch peers: {e}"),
                },
            },
            Err(e) => ServerMessage::Error {
                code: "internal_error".to_string(),
                message: format!("database lock poisoned: {e}"),
            },
        }
    }

    /// Handles GetMessages: returns message history with a peer.
    fn handle_get_messages(
        &self,
        peer_id: &PeerId,
        limit: u32,
        before: Option<Timestamp>,
    ) -> ServerMessage {
        match self.db.lock() {
            Ok(db) => match db.get_messages(peer_id, limit, before) {
                Ok(messages) => ServerMessage::Messages { messages },
                Err(e) => ServerMessage::Error {
                    code: "db_error".to_string(),
                    message: format!("failed to fetch messages: {e}"),
                },
            },
            Err(e) => ServerMessage::Error {
                code: "internal_error".to_string(),
                message: format!("database lock poisoned: {e}"),
            },
        }
    }

    /// Handles SendMessage: saves the message locally and sends it to the peer via TCP.
    async fn handle_send_message(&mut self, peer_id: &PeerId, content: &str) -> ServerMessage {
        // Validate the message content
        if let Err(e) = MessageContent::new(content) {
            return ServerMessage::Error {
                code: "invalid_content".to_string(),
                message: e.to_string(),
            };
        }

        // Find the peer's addresses
        let peer_info = self.online_peers.get(peer_id).cloned();
        let addresses = match &peer_info {
            Some(info) => info.addresses.clone(),
            None => {
                // Peer might be offline — try to get their last known addresses from DB
                match self.db.lock() {
                    Ok(db) => match db.get_peers() {
                        Ok(peers) => peers
                            .into_iter()
                            .find(|p| p.id == *peer_id)
                            .map(|p| p.addresses)
                            .unwrap_or_default(),
                        Err(_) => vec![],
                    },
                    Err(_) => vec![],
                }
            }
        };

        if addresses.is_empty() {
            return ServerMessage::Error {
                code: "peer_not_found".to_string(),
                message: format!("no known addresses for peer {peer_id}"),
            };
        }

        // Create the message
        let message_id = MessageId::generate();
        let timestamp = Timestamp::now();

        let peer_message = PeerMessage::Chat {
            id: message_id.clone(),
            sender_id: PeerId::new(&self.config.peer_id),
            sender_name: self.config.display_name.clone(),
            content: content.to_string(),
            timestamp,
        };

        // Save to our local database first
        let message = Message {
            id: message_id.clone(),
            peer_id: peer_id.clone(),
            direction: Direction::Sent,
            content: content.to_string(),
            timestamp,
            delivered: false,
        };

        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.save_message(&message) {
                error!(error = %e, "failed to save outgoing message");
                return ServerMessage::Error {
                    code: "db_error".to_string(),
                    message: format!("failed to save message: {e}"),
                };
            }
        }

        // Send the message to the peer via TCP
        match client::send_to_any(&addresses, &peer_message).await {
            Ok(()) => {
                info!(
                    message_id = %message_id,
                    peer_id = %peer_id,
                    "message sent and acknowledged"
                );

                // Mark as delivered since we got an ACK
                if let Ok(db) = self.db.lock() {
                    let _ = db.mark_delivered(&message_id);
                }

                ServerMessage::MessageSent { message_id }
            }
            Err(e) => {
                warn!(
                    message_id = %message_id,
                    peer_id = %peer_id,
                    error = %e,
                    "failed to deliver message"
                );

                // Message is saved locally but not delivered.
                // We still return MessageSent so the TUI shows it,
                // but with delivered=false.
                ServerMessage::MessageSent { message_id }
            }
        }
    }

    /// Handles GetConfig: returns the current configuration.
    fn handle_get_config(&self) -> ServerMessage {
        ServerMessage::Config {
            display_name: self.config.display_name.clone(),
            peer_id: PeerId::new(&self.config.peer_id),
        }
    }

    /// Handles SetDisplayName: updates the display name.
    fn handle_set_display_name(&mut self, name: &str) -> ServerMessage {
        // Validate
        if name.trim().is_empty() || name.len() > 50 {
            return ServerMessage::Error {
                code: "invalid_name".to_string(),
                message: "display name must be 1-50 characters".to_string(),
            };
        }

        self.config.display_name = name.trim().to_string();

        // Save to config file
        if let Err(e) = self.config.save() {
            error!(error = %e, "failed to save config");
            return ServerMessage::Error {
                code: "config_error".to_string(),
                message: format!("failed to save config: {e}"),
            };
        }

        info!(new_name = %self.config.display_name, "display name updated");
        ServerMessage::Ok
    }
}
