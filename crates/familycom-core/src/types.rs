//! Domain types for FamilyCom.
//!
//! All core types are defined here as **newtypes** — thin wrappers around
//! primitive types that give them distinct identities in the type system.
//! This prevents accidentally passing a `MessageId` where a `PeerId` is
//! expected, which would compile fine if both were plain `String`s.
//!
//! # Design Pattern: Newtype
//!
//! In Rust, a "newtype" is a single-field tuple struct like `PeerId(String)`.
//! It has zero runtime cost (same memory layout as the inner type) but gives
//! us compile-time type safety. We derive `Serialize`/`Deserialize` so these
//! types work seamlessly with both MessagePack (wire protocol) and JSON (IPC).

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// PeerId — uniquely identifies a machine running FamilyCom
// ---------------------------------------------------------------------------

/// A unique identifier for a peer on the network.
///
/// Generated once on first run (UUID v4) and stored in the local config.
/// Two different machines will always have different `PeerId`s, even if
/// they have the same display name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(String);

impl PeerId {
    /// Creates a new `PeerId` from a string.
    ///
    /// In production this will be a UUID, but we accept any string
    /// to keep tests simple.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Generates a new random `PeerId` using UUID v4.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Returns the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Display a `PeerId` by showing its inner string.
/// This makes it easy to use in log messages and formatted strings.
impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// MessageId — uniquely identifies a single message
// ---------------------------------------------------------------------------

/// A unique identifier for a message.
///
/// Each message gets a UUID v4 assigned by the sender. This lets the
/// receiver send back an `Ack` referencing which message was delivered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(String);

impl MessageId {
    /// Creates a `MessageId` from an existing string (e.g., loaded from DB).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Generates a new random `MessageId` using UUID v4.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Returns the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// DisplayName — a human-readable name for a peer
// ---------------------------------------------------------------------------

/// A human-readable name chosen by the user for their machine.
///
/// Examples: "PC-Sala", "Laptop-Ignacio", "Servidor".
///
/// Validated on creation:
/// - Must not be empty
/// - Maximum 50 characters
/// - Leading/trailing whitespace is trimmed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayName(String);

/// Errors that can occur when creating a `DisplayName`.
#[derive(Debug, thiserror::Error)]
pub enum DisplayNameError {
    #[error("display name cannot be empty")]
    Empty,
    #[error("display name cannot exceed {max} characters (got {got})")]
    TooLong { max: usize, got: usize },
}

impl DisplayName {
    /// Maximum allowed length for a display name.
    pub const MAX_LENGTH: usize = 50;

    /// Creates a new `DisplayName`, validating the input.
    ///
    /// The name is trimmed of leading/trailing whitespace before validation.
    ///
    /// # Errors
    ///
    /// Returns `DisplayNameError::Empty` if the trimmed name is empty.
    /// Returns `DisplayNameError::TooLong` if it exceeds 50 characters.
    pub fn new(name: impl Into<String>) -> Result<Self, DisplayNameError> {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return Err(DisplayNameError::Empty);
        }
        if name.len() > Self::MAX_LENGTH {
            return Err(DisplayNameError::TooLong {
                max: Self::MAX_LENGTH,
                got: name.len(),
            });
        }
        Ok(Self(name))
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// MessageContent — the text body of a message
// ---------------------------------------------------------------------------

/// The text content of a chat message.
///
/// Validated on creation:
/// - Must not be empty (after trimming)
/// - Maximum 10,000 characters
///
/// Supports full UTF-8 including Spanish characters (ñ, á, é, í, ó, ú).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent(String);

/// Errors that can occur when creating a `MessageContent`.
#[derive(Debug, thiserror::Error)]
pub enum MessageContentError {
    #[error("message content cannot be empty")]
    Empty,
    #[error("message content cannot exceed {max} characters (got {got})")]
    TooLong { max: usize, got: usize },
}

impl MessageContent {
    /// Maximum allowed length for a message.
    pub const MAX_LENGTH: usize = 10_000;

    /// Creates a new `MessageContent`, validating the input.
    ///
    /// Unlike `DisplayName`, we do NOT trim the content — the user may
    /// intentionally include leading/trailing whitespace. We only check
    /// that it's not entirely whitespace.
    ///
    /// # Errors
    ///
    /// Returns `MessageContentError::Empty` if the content is empty or all whitespace.
    /// Returns `MessageContentError::TooLong` if it exceeds 10,000 characters.
    pub fn new(content: impl Into<String>) -> Result<Self, MessageContentError> {
        let content = content.into();
        if content.trim().is_empty() {
            return Err(MessageContentError::Empty);
        }
        if content.len() > Self::MAX_LENGTH {
            return Err(MessageContentError::TooLong {
                max: Self::MAX_LENGTH,
                got: content.len(),
            });
        }
        Ok(Self(content))
    }

    /// Returns the content as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Timestamp — Unix milliseconds since epoch
// ---------------------------------------------------------------------------

/// A point in time represented as milliseconds since the Unix epoch.
///
/// We use milliseconds (not seconds) for sub-second precision in message
/// ordering. This is a common choice for chat applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Creates a `Timestamp` from raw Unix milliseconds.
    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Returns the current time as a `Timestamp`.
    pub fn now() -> Self {
        Self(chrono::Utc::now().timestamp_millis())
    }

    /// Returns the raw milliseconds value.
    pub fn as_millis(&self) -> i64 {
        self.0
    }

    /// Formats this timestamp as a local time string like "10:30" or "10:30:45".
    ///
    /// Uses the system's local timezone. Returns "??:??" if the timestamp
    /// can't be converted (e.g., out-of-range values).
    pub fn format_local_time(&self) -> String {
        use chrono::{Local, TimeZone};
        match Local.timestamp_millis_opt(self.0) {
            chrono::LocalResult::Single(dt) => dt.format("%H:%M").to_string(),
            _ => "??:??".to_string(),
        }
    }

    /// Formats this timestamp as a local date+time string like "2026-02-13 10:30".
    pub fn format_local_datetime(&self) -> String {
        use chrono::{Local, TimeZone};
        match Local.timestamp_millis_opt(self.0) {
            chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
            _ => "????-??-?? ??:??".to_string(),
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_local_time())
    }
}

// ---------------------------------------------------------------------------
// Direction — whether a message was sent or received
// ---------------------------------------------------------------------------

/// Indicates whether a message was sent by us or received from a peer.
///
/// Stored in the database and used by the UI to decide how to render
/// each message (e.g., "Yo:" vs the peer's name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Sent,
    Received,
}

impl Direction {
    /// Returns the string representation used in the database.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Direction::Sent => "sent",
            Direction::Received => "received",
        }
    }

    /// Parses a direction from its database string representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is neither "sent" nor "received".
    pub fn from_db_str(s: &str) -> Result<Self, String> {
        match s {
            "sent" => Ok(Direction::Sent),
            "received" => Ok(Direction::Received),
            other => Err(format!("invalid direction: '{other}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// PeerInfo — information about a discovered peer
// ---------------------------------------------------------------------------

/// Complete information about a peer discovered on the network.
///
/// This struct is used both in the daemon (updated from mDNS events)
/// and sent to the TUI via IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Unique identifier (UUID) of the peer.
    pub id: PeerId,
    /// Human-readable name chosen by the peer's user.
    pub display_name: String,
    /// Network addresses where this peer can be reached (e.g., "192.168.1.10:9876").
    pub addresses: Vec<String>,
    /// When we last saw this peer on the network.
    pub last_seen_at: Timestamp,
    /// Whether the peer is currently reachable (based on mDNS presence).
    pub online: bool,
}

// ---------------------------------------------------------------------------
// Message — a chat message (sent or received)
// ---------------------------------------------------------------------------

/// A complete chat message with all metadata.
///
/// This is the main data type stored in SQLite and displayed in the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique identifier for this message (UUID v4).
    pub id: MessageId,
    /// The other party: sender (if received) or recipient (if sent).
    pub peer_id: PeerId,
    /// Whether this message was sent by us or received from the peer.
    pub direction: Direction,
    /// The text content of the message (UTF-8).
    pub content: String,
    /// When the message was created (Unix millis).
    pub timestamp: Timestamp,
    /// Whether delivery was confirmed:
    /// - For sent messages: true if we received an ACK from the peer
    /// - For received messages: true if we sent an ACK back
    pub delivered: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_id_generate_is_unique() {
        let a = PeerId::generate();
        let b = PeerId::generate();
        assert_ne!(a, b, "two generated PeerIds should be different");
    }

    #[test]
    fn peer_id_display() {
        let id = PeerId::new("abc-123");
        assert_eq!(id.to_string(), "abc-123");
    }

    #[test]
    fn display_name_valid() {
        let name = DisplayName::new("PC-Sala").unwrap();
        assert_eq!(name.as_str(), "PC-Sala");
    }

    #[test]
    fn display_name_trimmed() {
        let name = DisplayName::new("  Laptop  ").unwrap();
        assert_eq!(name.as_str(), "Laptop");
    }

    #[test]
    fn display_name_empty_rejected() {
        assert!(DisplayName::new("").is_err());
        assert!(DisplayName::new("   ").is_err());
    }

    #[test]
    fn display_name_too_long_rejected() {
        let long = "a".repeat(51);
        assert!(DisplayName::new(long).is_err());
    }

    #[test]
    fn display_name_spanish_chars() {
        // Spanish characters should work fine — they're valid UTF-8
        let name = DisplayName::new("Salón de Mamá").unwrap();
        assert_eq!(name.as_str(), "Salón de Mamá");
    }

    #[test]
    fn message_content_valid() {
        let content = MessageContent::new("Hola, cómo estás?").unwrap();
        assert_eq!(content.as_str(), "Hola, cómo estás?");
    }

    #[test]
    fn message_content_empty_rejected() {
        assert!(MessageContent::new("").is_err());
        assert!(MessageContent::new("   ").is_err());
    }

    #[test]
    fn message_content_too_long_rejected() {
        let long = "a".repeat(10_001);
        assert!(MessageContent::new(long).is_err());
    }

    #[test]
    fn timestamp_now_is_positive() {
        let ts = Timestamp::now();
        assert!(ts.as_millis() > 0);
    }

    #[test]
    fn timestamp_ordering() {
        let earlier = Timestamp::from_millis(1000);
        let later = Timestamp::from_millis(2000);
        assert!(earlier < later);
    }

    #[test]
    fn direction_db_roundtrip() {
        assert_eq!(
            Direction::from_db_str(Direction::Sent.as_db_str()).unwrap(),
            Direction::Sent
        );
        assert_eq!(
            Direction::from_db_str(Direction::Received.as_db_str()).unwrap(),
            Direction::Received
        );
    }

    #[test]
    fn direction_invalid_db_str() {
        assert!(Direction::from_db_str("invalid").is_err());
    }

    #[test]
    fn peer_id_serde_json_roundtrip() {
        let id = PeerId::new("test-peer-123");
        let json = serde_json::to_string(&id).unwrap();
        let parsed: PeerId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn message_serde_json_roundtrip() {
        let msg = Message {
            id: MessageId::generate(),
            peer_id: PeerId::new("peer-1"),
            direction: Direction::Sent,
            content: "Hola desde la cocina!".to_string(),
            timestamp: Timestamp::now(),
            delivered: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg.id, parsed.id);
        assert_eq!(msg.content, parsed.content);
        assert_eq!(msg.direction, parsed.direction);
    }
}
