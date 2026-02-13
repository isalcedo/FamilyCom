//! IPC protocol between the daemon and TUI clients.
//!
//! The daemon exposes a Unix domain socket that TUI clients connect to.
//! Communication uses **JSON lines**: each message is a single JSON object
//! followed by a newline character (`\n`).
//!
//! # Why JSON (not MessagePack)?
//!
//! The IPC protocol uses JSON instead of MessagePack for two reasons:
//! 1. **Debuggability**: you can test the daemon with `socat` and see
//!    human-readable requests/responses
//! 2. **Performance is irrelevant**: IPC is localhost-only and the data
//!    volume is tiny compared to the peer-to-peer protocol
//!
//! # Request-Response Pattern
//!
//! The TUI sends a `ClientRequest` and the daemon responds with a
//! `ServerMessage`. Some requests (like `Subscribe`) cause the daemon
//! to push additional `ServerMessage`s whenever events occur (new messages,
//! peer changes).
//!
//! # Example Session
//!
//! ```text
//! TUI → Daemon:  {"Subscribe":{}}
//! Daemon → TUI:  {"type":"Ok"}
//! TUI → Daemon:  {"ListPeers":{}}
//! Daemon → TUI:  {"type":"PeerList","peers":[...]}
//! ... later, when a message arrives ...
//! Daemon → TUI:  {"type":"NewMessage","message":{...}}
//! ```

use crate::types::{Message, MessageId, PeerId, PeerInfo, Timestamp};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during IPC communication.
#[derive(Debug, Error)]
pub enum IpcError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IPC line too long: {size} bytes (max {max})")]
    LineTooLong { size: usize, max: usize },
}

/// Maximum IPC line length: 1 MB (same limit as the wire protocol).
pub const MAX_IPC_LINE_LENGTH: usize = 1_048_576;

// ---------------------------------------------------------------------------
// Client → Daemon requests
// ---------------------------------------------------------------------------

/// A request sent from the TUI client to the daemon.
///
/// Each variant maps to a specific action the daemon should perform.
/// The daemon always responds with a `ServerMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    /// Request the list of all known peers (online and offline).
    ListPeers,

    /// Request message history with a specific peer.
    GetMessages {
        /// Which peer's messages to fetch.
        peer_id: PeerId,
        /// Maximum number of messages to return.
        limit: u32,
        /// If provided, only return messages older than this timestamp.
        /// Used for pagination (loading older messages).
        #[serde(default)]
        before: Option<Timestamp>,
    },

    /// Send a text message to a peer.
    SendMessage {
        /// The recipient peer.
        peer_id: PeerId,
        /// The message text.
        content: String,
    },

    /// Get the current configuration (display name, peer ID).
    GetConfig,

    /// Change this machine's display name.
    SetDisplayName {
        /// The new display name.
        name: String,
    },

    /// Subscribe to real-time events (new messages, peer online/offline).
    ///
    /// After subscribing, the daemon will push `ServerMessage` events
    /// to this client whenever something happens, without the client
    /// needing to poll.
    Subscribe,
}

// ---------------------------------------------------------------------------
// Daemon → Client responses and events
// ---------------------------------------------------------------------------

/// A message sent from the daemon to a TUI client.
///
/// This can be either a direct response to a `ClientRequest`, or a
/// pushed event (if the client has subscribed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Simple acknowledgment (e.g., for Subscribe, SetDisplayName).
    Ok,

    /// Response to `ListPeers`: the full list of known peers.
    PeerList {
        peers: Vec<PeerInfo>,
    },

    /// Response to `GetMessages`: a page of message history.
    Messages {
        messages: Vec<Message>,
    },

    /// Acknowledgment that a message was sent (and its assigned ID).
    MessageSent {
        message_id: MessageId,
    },

    /// Pushed event: a new message was received from a peer.
    NewMessage {
        message: Message,
    },

    /// Pushed event: a peer came online (discovered via mDNS).
    PeerOnline {
        peer: PeerInfo,
    },

    /// Pushed event: a peer went offline (mDNS goodbye or timeout).
    PeerOffline {
        peer_id: PeerId,
    },

    /// Pushed event: a previously sent message was delivered (ACK received).
    MessageDelivered {
        message_id: MessageId,
    },

    /// Response to `GetConfig`: the current local configuration.
    Config {
        /// This machine's display name.
        display_name: String,
        /// This machine's unique peer ID.
        peer_id: PeerId,
    },

    /// Error response when a request fails.
    Error {
        /// Machine-readable error code (e.g., "peer_not_found", "db_error").
        code: String,
        /// Human-readable error description.
        message: String,
    },
}

/// Serializes a `ClientRequest` to a JSON line (with trailing newline).
pub fn encode_request(request: &ClientRequest) -> Result<String, IpcError> {
    let mut json = serde_json::to_string(request)?;
    json.push('\n');
    Ok(json)
}

/// Deserializes a `ClientRequest` from a JSON line.
pub fn decode_request(line: &str) -> Result<ClientRequest, IpcError> {
    let request = serde_json::from_str(line.trim())?;
    Ok(request)
}

/// Serializes a `ServerMessage` to a JSON line (with trailing newline).
pub fn encode_response(response: &ServerMessage) -> Result<String, IpcError> {
    let mut json = serde_json::to_string(response)?;
    json.push('\n');
    Ok(json)
}

/// Deserializes a `ServerMessage` from a JSON line.
pub fn decode_response(line: &str) -> Result<ServerMessage, IpcError> {
    let response = serde_json::from_str(line.trim())?;
    Ok(response)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Timestamp;

    #[test]
    fn request_list_peers_roundtrip() {
        let req = ClientRequest::ListPeers;
        let json = encode_request(&req).unwrap();
        let decoded = decode_request(&json).unwrap();
        // Verify it's the right variant
        assert!(matches!(decoded, ClientRequest::ListPeers));
    }

    #[test]
    fn request_send_message_roundtrip() {
        let req = ClientRequest::SendMessage {
            peer_id: PeerId::new("peer-1"),
            content: "¡Hola desde la sala!".to_string(),
        };
        let json = encode_request(&req).unwrap();
        let decoded = decode_request(&json).unwrap();
        match decoded {
            ClientRequest::SendMessage { peer_id, content } => {
                assert_eq!(peer_id.as_str(), "peer-1");
                assert_eq!(content, "¡Hola desde la sala!");
            }
            _ => panic!("expected SendMessage"),
        }
    }

    #[test]
    fn request_get_messages_with_pagination() {
        let req = ClientRequest::GetMessages {
            peer_id: PeerId::new("peer-1"),
            limit: 50,
            before: Some(Timestamp::from_millis(1707849600000)),
        };
        let json = encode_request(&req).unwrap();
        let decoded = decode_request(&json).unwrap();
        match decoded {
            ClientRequest::GetMessages {
                peer_id,
                limit,
                before,
            } => {
                assert_eq!(peer_id.as_str(), "peer-1");
                assert_eq!(limit, 50);
                assert_eq!(before.unwrap().as_millis(), 1707849600000);
            }
            _ => panic!("expected GetMessages"),
        }
    }

    #[test]
    fn response_peer_list_roundtrip() {
        let resp = ServerMessage::PeerList {
            peers: vec![PeerInfo {
                id: PeerId::new("p1"),
                display_name: "Computador de Mamá".to_string(),
                addresses: vec!["192.168.1.5:9876".to_string()],
                last_seen_at: Timestamp::now(),
                online: true,
            }],
        };
        let json = encode_response(&resp).unwrap();
        let decoded = decode_response(&json).unwrap();
        match decoded {
            ServerMessage::PeerList { peers } => {
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].display_name, "Computador de Mamá");
            }
            _ => panic!("expected PeerList"),
        }
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = ServerMessage::Error {
            code: "peer_not_found".to_string(),
            message: "No peer with ID 'abc' exists".to_string(),
        };
        let json = encode_response(&resp).unwrap();
        let decoded = decode_response(&json).unwrap();
        match decoded {
            ServerMessage::Error { code, message } => {
                assert_eq!(code, "peer_not_found");
                assert_eq!(message, "No peer with ID 'abc' exists");
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn json_lines_are_single_line() {
        // Each encoded message should be exactly one line (no embedded newlines)
        let req = ClientRequest::SendMessage {
            peer_id: PeerId::new("peer-1"),
            content: "This is a\nmultiline message".to_string(),
        };
        let json = encode_request(&req).unwrap();
        // The JSON itself shouldn't contain raw newlines (they're escaped as \n)
        // Only the trailing newline we added should be there
        let lines: Vec<&str> = json.trim().split('\n').collect();
        assert_eq!(lines.len(), 1, "JSON line should not contain embedded newlines");
    }

    #[test]
    fn all_request_variants_serialize() {
        // Verify that every ClientRequest variant can be serialized without error
        let requests = vec![
            ClientRequest::ListPeers,
            ClientRequest::GetMessages {
                peer_id: PeerId::new("p"),
                limit: 10,
                before: None,
            },
            ClientRequest::SendMessage {
                peer_id: PeerId::new("p"),
                content: "hi".to_string(),
            },
            ClientRequest::GetConfig,
            ClientRequest::SetDisplayName {
                name: "New Name".to_string(),
            },
            ClientRequest::Subscribe,
        ];
        for req in requests {
            let json = encode_request(&req).unwrap();
            assert!(!json.is_empty());
        }
    }
}
