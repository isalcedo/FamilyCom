//! TCP message client.
//!
//! Sends messages to other FamilyCom daemons over TCP. Each send operation
//! establishes a new TCP connection, sends the message, waits for an ACK,
//! and closes the connection.
//!
//! # Why connect-per-message?
//!
//! For a home LAN chat app with low message volume, the simplicity of
//! connect-per-message outweighs the overhead. Each send is:
//! 1. TCP connect (< 1ms on LAN)
//! 2. Send message frame
//! 3. Read ACK frame
//! 4. Close connection
//!
//! If performance becomes an issue, we can add connection pooling later.
//!
//! # Timeout
//!
//! All operations have a timeout to handle unreachable peers gracefully.
//! If a peer's mDNS entry is stale (they crashed without unregistering),
//! the timeout prevents us from blocking forever.

use familycom_core::protocol::{self, PeerMessage, ProtocolError};
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, warn};

/// How long to wait for a TCP connection to be established.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for an ACK after sending a message.
const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors that can occur when sending a message to a peer.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("connection to {addr} timed out after {timeout:?}")]
    ConnectTimeout { addr: String, timeout: Duration },

    #[error("failed to connect to {addr}: {source}")]
    Connect { addr: String, source: std::io::Error },

    #[error("timed out waiting for ACK from {addr}")]
    AckTimeout { addr: String },

    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    #[error("peer at {addr} did not acknowledge message (got unexpected response)")]
    UnexpectedResponse { addr: String },

    #[error("no reachable address for peer")]
    NoAddress,
}

/// Sends a `PeerMessage` to a peer at the given address and waits for an ACK.
///
/// This is the main function used by the daemon to send chat messages.
///
/// # Arguments
///
/// * `addr` - The peer's address as "ip:port" string (e.g., "192.168.1.10:9876")
/// * `message` - The message to send (usually a `PeerMessage::Chat`)
///
/// # Returns
///
/// `Ok(())` if the message was sent and acknowledged.
/// `Err(...)` if the connection failed, timed out, or the peer didn't ACK.
pub async fn send_message(addr: &str, message: &PeerMessage) -> Result<(), ClientError> {
    // Step 1: Establish TCP connection with timeout
    debug!(addr, "connecting to peer");
    let mut stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            return Err(ClientError::Connect {
                addr: addr.to_string(),
                source: e,
            });
        }
        Err(_) => {
            return Err(ClientError::ConnectTimeout {
                addr: addr.to_string(),
                timeout: CONNECT_TIMEOUT,
            });
        }
    };

    // Step 2: Send the message
    let (mut reader, mut writer) = stream.split();
    protocol::write_message(&mut writer, message).await?;
    debug!(addr, "message sent, waiting for ACK");

    // Step 3: Wait for ACK with timeout
    let response = match timeout(ACK_TIMEOUT, protocol::read_message(&mut reader)).await {
        Ok(Ok(msg)) => msg,
        Ok(Err(e)) => return Err(ClientError::Protocol(e)),
        Err(_) => {
            return Err(ClientError::AckTimeout {
                addr: addr.to_string(),
            });
        }
    };

    // Step 4: Verify we got an ACK (not some other message type)
    match &response {
        PeerMessage::Ack { message_id } => {
            debug!(message_id = %message_id, addr, "received ACK");
            Ok(())
        }
        _ => {
            warn!(addr, ?response, "expected ACK but got different message");
            Err(ClientError::UnexpectedResponse {
                addr: addr.to_string(),
            })
        }
    }
}

/// Tries to send a message to a peer using any of their known addresses.
///
/// Iterates through the peer's address list and tries each one until
/// one succeeds. This handles cases where a peer has multiple network
/// interfaces (e.g., WiFi and Ethernet) and one is unreachable.
///
/// # Arguments
///
/// * `addresses` - List of "ip:port" strings for the peer
/// * `message` - The message to send
///
/// # Returns
///
/// `Ok(())` if the message was delivered via any address.
/// `Err(...)` if all addresses failed.
pub async fn send_to_any(
    addresses: &[String],
    message: &PeerMessage,
) -> Result<(), ClientError> {
    if addresses.is_empty() {
        return Err(ClientError::NoAddress);
    }

    let mut last_error = None;

    for addr in addresses {
        match send_message(addr, message).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(addr, error = %e, "failed to send to this address, trying next");
                last_error = Some(e);
            }
        }
    }

    // All addresses failed — return the last error
    Err(last_error.unwrap_or(ClientError::NoAddress))
}
