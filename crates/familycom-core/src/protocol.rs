//! Peer-to-peer wire protocol for FamilyCom.
//!
//! This module defines the messages exchanged between FamilyCom daemons
//! over TCP connections on the local network.
//!
//! # Wire Format
//!
//! Each message is transmitted as a **length-prefixed frame**:
//!
//! ```text
//! +-------------------+------------------------------+
//! | Length (4 bytes)   | MessagePack Payload          |
//! | big-endian u32     | (variable length)            |
//! +-------------------+------------------------------+
//! ```
//!
//! The length prefix tells the receiver how many bytes to read for the
//! payload. This is a simple and efficient framing strategy that avoids
//! the need for delimiters (which would require escaping in the payload).
//!
//! # Why MessagePack?
//!
//! - **Compact**: significantly smaller than JSON (no field name repetition)
//! - **Self-describing**: unlike protobuf, you can decode without a schema
//! - **Fast**: near-zero overhead for encoding/decoding
//! - **Compatible**: if we add new fields, old clients can still decode
//!
//! # Message Types
//!
//! - `Chat`: a text message from one peer to another
//! - `Ack`: confirms receipt of a `Chat` message
//! - `Ping` / `Pong`: keepalive to detect disconnected peers

use crate::types::{MessageId, PeerId, Timestamp};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum frame size: 1 MB. Any frame larger than this is rejected
/// to prevent memory exhaustion from malformed data.
const MAX_FRAME_SIZE: u32 = 1_048_576;

/// Errors that can occur during protocol encoding/decoding.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("MessagePack encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("MessagePack decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("frame too large: {size} bytes (max {MAX_FRAME_SIZE})")]
    FrameTooLarge { size: u32 },

    #[error("connection closed by peer")]
    ConnectionClosed,
}

/// A message exchanged between two FamilyCom daemons over TCP.
///
/// Each variant represents a different type of peer-to-peer interaction.
/// The `#[serde(tag = "type")]` attribute adds a `"type"` field to the
/// serialized form, making it easy to distinguish variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PeerMessage {
    /// A chat message from one peer to another.
    Chat {
        /// Unique message ID (UUID v4), assigned by the sender.
        id: MessageId,
        /// Who sent this message.
        sender_id: PeerId,
        /// Display name of the sender (so receiver can show it immediately
        /// without needing to look up the peer in their DB).
        sender_name: String,
        /// The message text (UTF-8, supports Spanish characters).
        content: String,
        /// When the message was created (Unix millis).
        timestamp: Timestamp,
    },

    /// Acknowledgment that a message was received and stored.
    ///
    /// Sent back to the original sender so they can mark the message
    /// as "delivered" in their local DB.
    Ack {
        /// The ID of the message being acknowledged.
        message_id: MessageId,
    },

    /// Keepalive ping. The receiver should respond with `Pong`.
    ///
    /// Used to detect if a TCP connection is still alive when there's
    /// no chat traffic. The daemon can send periodic pings and consider
    /// a peer offline if no pong comes back within a timeout.
    Ping,

    /// Response to a `Ping`.
    Pong,
}

/// Encodes a `PeerMessage` into a length-prefixed byte buffer.
///
/// The returned buffer contains:
/// - 4 bytes: payload length as big-endian u32
/// - N bytes: MessagePack-encoded payload
///
/// This is the format written to TCP streams.
pub fn encode(msg: &PeerMessage) -> Result<Vec<u8>, ProtocolError> {
    // First, serialize the message to MessagePack bytes
    let payload = rmp_serde::to_vec_named(msg)?;

    // Build the frame: 4-byte length prefix + payload
    let length = payload.len() as u32;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);

    Ok(frame)
}

/// Decodes a `PeerMessage` from a MessagePack payload (without length prefix).
///
/// This is used after reading the length prefix and payload bytes separately.
pub fn decode(payload: &[u8]) -> Result<PeerMessage, ProtocolError> {
    let msg = rmp_serde::from_slice(payload)?;
    Ok(msg)
}

/// Writes a `PeerMessage` to an async writer (e.g., a TCP stream).
///
/// This is the main function used by the daemon to send messages over the network.
/// It handles the full process: serialize → length-prefix → write to stream.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &PeerMessage,
) -> Result<(), ProtocolError> {
    let frame = encode(msg)?;
    writer.write_all(&frame).await?;
    // Flush to ensure the data is sent immediately, not buffered.
    // This is important for chat apps where latency matters.
    writer.flush().await?;
    Ok(())
}

/// Reads a `PeerMessage` from an async reader (e.g., a TCP stream).
///
/// This is the main function used by the daemon to receive messages.
/// It handles: read length prefix → validate size → read payload → deserialize.
///
/// Returns `ProtocolError::ConnectionClosed` if the peer closes the connection
/// (indicated by reading 0 bytes when expecting the length prefix).
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<PeerMessage, ProtocolError> {
    // Step 1: Read the 4-byte length prefix
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            // The other side closed the connection cleanly
            return Err(ProtocolError::ConnectionClosed);
        }
        Err(e) => return Err(ProtocolError::Io(e)),
    }
    let length = u32::from_be_bytes(len_buf);

    // Step 2: Validate the frame size to prevent memory exhaustion
    if length > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge { size: length });
    }

    // Step 3: Read exactly `length` bytes of payload
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload).await?;

    // Step 4: Deserialize from MessagePack
    decode(&payload)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_chat_roundtrip() {
        let msg = PeerMessage::Chat {
            id: MessageId::new("msg-123"),
            sender_id: PeerId::new("peer-abc"),
            sender_name: "PC-Sala".to_string(),
            content: "¡Hola! ¿Qué tal están?".to_string(),
            timestamp: Timestamp::from_millis(1707849600000),
        };

        // Encode to bytes
        let frame = encode(&msg).unwrap();

        // The first 4 bytes are the length prefix
        let length = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(length as usize, frame.len() - 4);

        // Decode the payload (skip the 4-byte length prefix)
        let decoded = decode(&frame[4..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn encode_decode_ack_roundtrip() {
        let msg = PeerMessage::Ack {
            message_id: MessageId::new("msg-456"),
        };
        let frame = encode(&msg).unwrap();
        let decoded = decode(&frame[4..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn encode_decode_ping_pong() {
        for msg in [PeerMessage::Ping, PeerMessage::Pong] {
            let frame = encode(&msg).unwrap();
            let decoded = decode(&frame[4..]).unwrap();
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn chat_message_is_compact() {
        // MessagePack should be significantly smaller than JSON
        let msg = PeerMessage::Chat {
            id: MessageId::new("550e8400-e29b-41d4-a716-446655440000"),
            sender_id: PeerId::new("660e8400-e29b-41d4-a716-446655440000"),
            sender_name: "PC-Sala".to_string(),
            content: "Hola mundo!".to_string(),
            timestamp: Timestamp::from_millis(1707849600000),
        };

        let msgpack_frame = encode(&msg).unwrap();
        let json_bytes = serde_json::to_vec(&msg).unwrap();

        // MessagePack should be notably smaller than JSON
        assert!(
            msgpack_frame.len() < json_bytes.len(),
            "MessagePack ({} bytes) should be smaller than JSON ({} bytes)",
            msgpack_frame.len(),
            json_bytes.len()
        );
    }

    /// Tests the async read/write functions using an in-memory pipe.
    #[tokio::test]
    async fn async_write_read_roundtrip() {
        // tokio::io::duplex creates a pair of connected streams,
        // like a pipe. What you write to one end can be read from the other.
        let (mut writer, mut reader) = tokio::io::duplex(1024);

        let original = PeerMessage::Chat {
            id: MessageId::new("msg-async"),
            sender_id: PeerId::new("peer-1"),
            sender_name: "Test".to_string(),
            content: "Mensaje asíncrono!".to_string(),
            timestamp: Timestamp::now(),
        };

        // Write the message on one end
        write_message(&mut writer, &original).await.unwrap();

        // Read it back from the other end
        let received = read_message(&mut reader).await.unwrap();
        assert_eq!(received, original);
    }

    /// Tests that multiple messages can be sent and received in sequence.
    #[tokio::test]
    async fn multiple_messages_in_sequence() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);

        let messages = vec![
            PeerMessage::Ping,
            PeerMessage::Pong,
            PeerMessage::Chat {
                id: MessageId::new("m1"),
                sender_id: PeerId::new("p1"),
                sender_name: "A".to_string(),
                content: "First".to_string(),
                timestamp: Timestamp::from_millis(1000),
            },
            PeerMessage::Ack {
                message_id: MessageId::new("m1"),
            },
        ];

        // Write all messages
        for msg in &messages {
            write_message(&mut writer, msg).await.unwrap();
        }

        // Read them all back in order
        for expected in &messages {
            let received = read_message(&mut reader).await.unwrap();
            assert_eq!(&received, expected);
        }
    }
}
