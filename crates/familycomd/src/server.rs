//! TCP message server.
//!
//! Listens for incoming TCP connections from other FamilyCom daemons
//! on the local network. When a peer connects, it reads length-prefixed
//! MessagePack frames (see `familycom_core::protocol`) and processes them.
//!
//! # Connection Flow
//!
//! 1. Peer connects via TCP
//! 2. Peer sends a `PeerMessage::Chat` frame
//! 3. We respond with a `PeerMessage::Ack` frame
//! 4. Connection may stay open for more messages or be closed
//!
//! Each incoming connection is handled in its own tokio task, so multiple
//! peers can send messages simultaneously without blocking each other.

use familycom_core::protocol::{self, PeerMessage, ProtocolError};
use std::net::SocketAddr;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Errors that can occur in the message server.
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("failed to bind TCP listener: {0}")]
    Bind(std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

/// An incoming message received from a peer over TCP.
///
/// This is what the server sends through its channel to the daemon's
/// main loop for processing (saving to DB, notifying TUI, etc.).
#[derive(Debug)]
pub struct IncomingMessage {
    /// The peer message that was received.
    pub message: PeerMessage,
    /// The remote address of the peer who sent it.
    pub from_addr: SocketAddr,
}

/// TCP server that accepts connections from other FamilyCom peers.
pub struct MessageServer {
    /// The underlying TCP listener.
    listener: TcpListener,
    /// The local address we're bound to (useful for logging and mDNS registration).
    local_addr: SocketAddr,
}

impl MessageServer {
    /// Binds a new TCP server to the given address.
    ///
    /// Use port `0` to let the OS assign a random available port.
    /// After binding, call `local_addr()` to find out which port was assigned.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() {
    /// let server = MessageServer::bind("0.0.0.0:0").await.unwrap();
    /// println!("Listening on {}", server.local_addr());
    /// # }
    /// ```
    pub async fn bind(addr: &str) -> Result<Self, ServerError> {
        let listener = TcpListener::bind(addr).await.map_err(ServerError::Bind)?;
        let local_addr = listener.local_addr().map_err(ServerError::Bind)?;
        info!(addr = %local_addr, "TCP message server listening");
        Ok(Self {
            listener,
            local_addr,
        })
    }

    /// Returns the local address this server is bound to.
    ///
    /// Particularly useful when binding to port 0 (auto-assign) — this
    /// tells you which port the OS chose, so you can register it via mDNS.
    #[allow(dead_code)]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns just the port number.
    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    /// Runs the accept loop, spawning a handler task for each incoming connection.
    ///
    /// Received messages are sent through the returned channel. This method
    /// runs forever (until the server is dropped or an unrecoverable error occurs).
    ///
    /// # Arguments
    ///
    /// * `message_tx` - Channel sender for forwarding received messages to the daemon.
    pub async fn accept_loop(self, message_tx: mpsc::Sender<IncomingMessage>) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer_addr)) => {
                    debug!(peer = %peer_addr, "accepted TCP connection");

                    // Handle each connection in its own task so one slow peer
                    // doesn't block others.
                    let tx = message_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, peer_addr, tx).await {
                            // ConnectionClosed is normal — peer just disconnected
                            match &e {
                                ProtocolError::ConnectionClosed => {
                                    debug!(peer = %peer_addr, "peer disconnected");
                                }
                                _ => {
                                    warn!(peer = %peer_addr, error = %e, "connection error");
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    // Accept errors are usually transient (too many open files, etc.)
                    // Log and continue rather than crashing.
                    error!(error = %e, "failed to accept TCP connection");
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Handles a single TCP connection from a peer.
///
/// Reads messages in a loop until the peer disconnects or an error occurs.
/// For each `Chat` message received, sends back an `Ack`.
async fn handle_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    message_tx: mpsc::Sender<IncomingMessage>,
) -> Result<(), ProtocolError> {
    // Split the stream so we can read and write independently.
    // This is important because we need to send Acks while potentially
    // receiving more messages.
    let (mut reader, mut writer) = stream.split();

    loop {
        // Read the next message from the peer
        let msg = protocol::read_message(&mut reader).await?;

        match &msg {
            PeerMessage::Chat { id, sender_name, .. } => {
                debug!(
                    message_id = %id,
                    sender = sender_name,
                    peer = %peer_addr,
                    "received chat message"
                );

                // Send acknowledgment back to the sender
                let ack = PeerMessage::Ack {
                    message_id: id.clone(),
                };
                if let Err(e) = protocol::write_message(&mut writer, &ack).await {
                    warn!(peer = %peer_addr, error = %e, "failed to send ACK");
                }
            }

            PeerMessage::Ping => {
                debug!(peer = %peer_addr, "received ping, sending pong");
                if let Err(e) = protocol::write_message(&mut writer, &PeerMessage::Pong).await {
                    warn!(peer = %peer_addr, error = %e, "failed to send pong");
                }
                // Don't forward pings to the daemon — they're just keepalive
                continue;
            }

            PeerMessage::Pong => {
                debug!(peer = %peer_addr, "received pong");
                continue;
            }

            PeerMessage::Ack { message_id } => {
                debug!(message_id = %message_id, peer = %peer_addr, "received ack");
            }
        }

        // Forward the message to the daemon's main loop for processing
        let incoming = IncomingMessage {
            message: msg,
            from_addr: peer_addr,
        };
        if message_tx.send(incoming).await.is_err() {
            debug!("message channel closed, stopping connection handler");
            break;
        }
    }

    Ok(())
}
