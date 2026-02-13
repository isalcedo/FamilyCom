//! SQLite database layer for FamilyCom.
//!
//! All persistent data is stored in a single SQLite database file per machine.
//! This includes messages, known peers, and local configuration key-value pairs.
//!
//! # Thread Safety
//!
//! `rusqlite::Connection` is `!Send`, meaning it cannot be moved between threads.
//! In the daemon, we wrap `Database` in a `std::sync::Mutex` and access it from
//! the tokio runtime using `tokio::task::spawn_blocking`. This is the recommended
//! pattern for synchronous database access in async Rust.
//!
//! # Why SQLite?
//!
//! SQLite is perfect for this use case:
//! - Zero configuration (no server to run)
//! - Single-file database (easy to backup)
//! - Excellent read performance for small datasets
//! - Built-in full UTF-8 support
//! - With the `bundled` feature, rusqlite compiles SQLite from source,
//!   so no system library is needed.

use crate::types::{Direction, Message, MessageId, PeerId, PeerInfo, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during database operations.
#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("invalid data in database: {0}")]
    InvalidData(String),
}

/// The database handle wrapping a SQLite connection.
///
/// Provides typed methods for all CRUD operations on messages, peers,
/// and configuration. All SQL uses parameterized queries to prevent
/// SQL injection.
pub struct Database {
    /// The underlying SQLite connection.
    /// We keep this private to enforce using our typed methods.
    conn: Connection,
}

impl Database {
    /// Opens (or creates) a database at the given path and runs migrations.
    ///
    /// If the file doesn't exist, SQLite creates it automatically.
    /// After opening, we run `migrate()` to ensure all tables exist.
    ///
    /// # WAL Mode
    ///
    /// We enable WAL (Write-Ahead Logging) mode for better concurrent read
    /// performance. This is especially useful when the daemon is writing
    /// messages while the TUI is reading them (though they go through IPC,
    /// not direct DB access).
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        let conn = Connection::open(path)?;

        // WAL mode: better performance for concurrent reads and writes.
        // Once set, it persists in the database file.
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // Foreign keys are off by default in SQLite — we need to enable them
        // for each connection so our FOREIGN KEY constraints are enforced.
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Opens an in-memory database (useful for tests).
    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Creates all tables if they don't already exist.
    ///
    /// This is idempotent — safe to call every time the app starts.
    /// Uses `CREATE TABLE IF NOT EXISTS` so it won't fail if tables
    /// already exist from a previous run.
    fn migrate(&self) -> Result<(), DatabaseError> {
        self.conn.execute_batch(
            "
            -- Key-value store for local configuration (peer_id, display_name, etc.)
            CREATE TABLE IF NOT EXISTS config (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Peers we've discovered on the network
            CREATE TABLE IF NOT EXISTS peers (
                id            TEXT PRIMARY KEY,
                display_name  TEXT NOT NULL,
                last_seen_at  INTEGER NOT NULL,
                addresses     TEXT NOT NULL  -- JSON array of 'ip:port' strings
            );

            -- Chat messages (both sent and received)
            CREATE TABLE IF NOT EXISTS messages (
                id        TEXT PRIMARY KEY,
                peer_id   TEXT NOT NULL,
                direction TEXT NOT NULL CHECK(direction IN ('sent', 'received')),
                content   TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                delivered INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (peer_id) REFERENCES peers(id)
            );

            -- Index for fetching messages with a specific peer, newest first
            CREATE INDEX IF NOT EXISTS idx_messages_peer_time
                ON messages(peer_id, timestamp DESC);

            -- Index for fetching all recent messages across all peers
            CREATE INDEX IF NOT EXISTS idx_messages_timestamp
                ON messages(timestamp DESC);
            ",
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Config operations
    // -----------------------------------------------------------------------

    /// Gets a configuration value by key.
    ///
    /// Returns `None` if the key doesn't exist.
    pub fn get_config(&self, key: &str) -> Result<Option<String>, DatabaseError> {
        let value = self
            .conn
            .query_row("SELECT value FROM config WHERE key = ?1", params![key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(value)
    }

    /// Sets a configuration value (insert or update).
    ///
    /// Uses SQLite's `INSERT OR REPLACE` which is atomic — it either
    /// inserts a new row or replaces the existing one with the same key.
    pub fn set_config(&self, key: &str, value: &str) -> Result<(), DatabaseError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Peer operations
    // -----------------------------------------------------------------------

    /// Inserts a new peer or updates an existing one.
    ///
    /// Uses `INSERT OR REPLACE` to handle both cases atomically.
    /// The `addresses` field is stored as a JSON array string.
    pub fn upsert_peer(&self, peer: &PeerInfo) -> Result<(), DatabaseError> {
        let addresses_json = serde_json::to_string(&peer.addresses)
            .map_err(|e| DatabaseError::InvalidData(format!("failed to serialize addresses: {e}")))?;

        self.conn.execute(
            "INSERT OR REPLACE INTO peers (id, display_name, last_seen_at, addresses)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                peer.id.as_str(),
                peer.display_name,
                peer.last_seen_at.as_millis(),
                addresses_json,
            ],
        )?;
        Ok(())
    }

    /// Returns all known peers.
    ///
    /// The `online` field is always set to `false` here — the daemon
    /// maintains online status in memory based on mDNS events, not in the DB.
    pub fn get_peers(&self) -> Result<Vec<PeerInfo>, DatabaseError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, display_name, last_seen_at, addresses FROM peers ORDER BY display_name")?;

        let peers = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let display_name: String = row.get(1)?;
                let last_seen_at: i64 = row.get(2)?;
                let addresses_json: String = row.get(3)?;
                Ok((id, display_name, last_seen_at, addresses_json))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        peers
            .into_iter()
            .map(|(id, display_name, last_seen_at, addresses_json)| {
                let addresses: Vec<String> =
                    serde_json::from_str(&addresses_json).map_err(|e| {
                        DatabaseError::InvalidData(format!("bad addresses JSON: {e}"))
                    })?;
                Ok(PeerInfo {
                    id: PeerId::new(id),
                    display_name,
                    addresses,
                    last_seen_at: Timestamp::from_millis(last_seen_at),
                    online: false, // Caller (daemon) sets this from mDNS state
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Message operations
    // -----------------------------------------------------------------------

    /// Saves a message to the database.
    ///
    /// The message must have a unique `id`. If a message with the same ID
    /// already exists, this will return an error (duplicate primary key).
    pub fn save_message(&self, msg: &Message) -> Result<(), DatabaseError> {
        self.conn.execute(
            "INSERT INTO messages (id, peer_id, direction, content, timestamp, delivered)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                msg.id.as_str(),
                msg.peer_id.as_str(),
                msg.direction.as_db_str(),
                msg.content,
                msg.timestamp.as_millis(),
                msg.delivered as i32,
            ],
        )?;
        Ok(())
    }

    /// Retrieves messages exchanged with a specific peer.
    ///
    /// Returns up to `limit` messages, ordered newest-first.
    /// If `before` is provided, only returns messages with a timestamp
    /// strictly less than that value (for pagination / infinite scroll).
    pub fn get_messages(
        &self,
        peer_id: &PeerId,
        limit: u32,
        before: Option<Timestamp>,
    ) -> Result<Vec<Message>, DatabaseError> {
        let messages = if let Some(before_ts) = before {
            // Fetch messages older than the given timestamp
            let mut stmt = self.conn.prepare(
                "SELECT id, peer_id, direction, content, timestamp, delivered
                 FROM messages
                 WHERE peer_id = ?1 AND timestamp < ?2
                 ORDER BY timestamp DESC
                 LIMIT ?3",
            )?;
            Self::collect_messages(&mut stmt, params![peer_id.as_str(), before_ts.as_millis(), limit])?
        } else {
            // Fetch the most recent messages
            let mut stmt = self.conn.prepare(
                "SELECT id, peer_id, direction, content, timestamp, delivered
                 FROM messages
                 WHERE peer_id = ?1
                 ORDER BY timestamp DESC
                 LIMIT ?2",
            )?;
            Self::collect_messages(&mut stmt, params![peer_id.as_str(), limit])?
        };

        Ok(messages)
    }

    /// Helper: collects message rows from a prepared statement into a Vec.
    ///
    /// This avoids duplicating the row-mapping logic between the two
    /// branches of `get_messages`.
    fn collect_messages(
        stmt: &mut rusqlite::Statement,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Message>, DatabaseError> {
        let rows = stmt
            .query_map(params, |row| {
                let id: String = row.get(0)?;
                let peer_id: String = row.get(1)?;
                let direction: String = row.get(2)?;
                let content: String = row.get(3)?;
                let timestamp: i64 = row.get(4)?;
                let delivered: i32 = row.get(5)?;
                Ok((id, peer_id, direction, content, timestamp, delivered))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, peer_id, direction, content, timestamp, delivered)| {
                let direction = Direction::from_db_str(&direction)
                    .map_err(DatabaseError::InvalidData)?;
                Ok(Message {
                    id: MessageId::new(id),
                    peer_id: PeerId::new(peer_id),
                    direction,
                    content,
                    timestamp: Timestamp::from_millis(timestamp),
                    delivered: delivered != 0,
                })
            })
            .collect()
    }

    /// Marks a message as delivered (ACK received or sent).
    ///
    /// Returns `Ok(true)` if a message was updated, `Ok(false)` if no
    /// message with that ID exists.
    pub fn mark_delivered(&self, message_id: &MessageId) -> Result<bool, DatabaseError> {
        let rows_affected = self.conn.execute(
            "UPDATE messages SET delivered = 1 WHERE id = ?1",
            params![message_id.as_str()],
        )?;
        Ok(rows_affected > 0)
    }

    /// Returns the count of unread (undelivered received) messages from a peer.
    ///
    /// Useful for showing unread badges in the TUI peer list.
    pub fn unread_count(&self, peer_id: &PeerId) -> Result<u32, DatabaseError> {
        let count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE peer_id = ?1 AND direction = 'received' AND delivered = 0",
            params![peer_id.as_str()],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: creates an in-memory database for testing.
    fn test_db() -> Database {
        Database::open_in_memory().expect("failed to create test database")
    }

    /// Helper: creates a test peer and inserts it into the database.
    fn insert_test_peer(db: &Database, id: &str, name: &str) {
        let peer = PeerInfo {
            id: PeerId::new(id),
            display_name: name.to_string(),
            addresses: vec!["192.168.1.10:9876".to_string()],
            last_seen_at: Timestamp::now(),
            online: true,
        };
        db.upsert_peer(&peer).unwrap();
    }

    #[test]
    fn config_set_and_get() {
        let db = test_db();
        db.set_config("peer_id", "abc-123").unwrap();
        assert_eq!(db.get_config("peer_id").unwrap(), Some("abc-123".to_string()));
    }

    #[test]
    fn config_get_missing_key() {
        let db = test_db();
        assert_eq!(db.get_config("nonexistent").unwrap(), None);
    }

    #[test]
    fn config_update_existing() {
        let db = test_db();
        db.set_config("name", "old").unwrap();
        db.set_config("name", "new").unwrap();
        assert_eq!(db.get_config("name").unwrap(), Some("new".to_string()));
    }

    #[test]
    fn peer_upsert_and_get() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC-Sala");

        let peers = db.get_peers().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].id.as_str(), "peer-1");
        assert_eq!(peers[0].display_name, "PC-Sala");
        assert_eq!(peers[0].addresses, vec!["192.168.1.10:9876"]);
        assert!(!peers[0].online); // DB always returns online=false
    }

    #[test]
    fn peer_upsert_updates_existing() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "Old Name");
        insert_test_peer(&db, "peer-1", "New Name");

        let peers = db.get_peers().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].display_name, "New Name");
    }

    #[test]
    fn message_save_and_get() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC-Sala");

        let msg = Message {
            id: MessageId::new("msg-1"),
            peer_id: PeerId::new("peer-1"),
            direction: Direction::Sent,
            content: "Hola, qué tal?".to_string(),
            timestamp: Timestamp::from_millis(1000),
            delivered: false,
        };
        db.save_message(&msg).unwrap();

        let messages = db.get_messages(&PeerId::new("peer-1"), 10, None).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hola, qué tal?");
        assert_eq!(messages[0].direction, Direction::Sent);
        assert!(!messages[0].delivered);
    }

    #[test]
    fn message_ordering_newest_first() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC");

        for i in 1..=5 {
            let msg = Message {
                id: MessageId::new(format!("msg-{i}")),
                peer_id: PeerId::new("peer-1"),
                direction: Direction::Sent,
                content: format!("Message {i}"),
                timestamp: Timestamp::from_millis(i * 1000),
                delivered: false,
            };
            db.save_message(&msg).unwrap();
        }

        let messages = db.get_messages(&PeerId::new("peer-1"), 10, None).unwrap();
        assert_eq!(messages.len(), 5);
        // Newest first
        assert_eq!(messages[0].content, "Message 5");
        assert_eq!(messages[4].content, "Message 1");
    }

    #[test]
    fn message_pagination_with_before() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC");

        for i in 1..=10 {
            let msg = Message {
                id: MessageId::new(format!("msg-{i}")),
                peer_id: PeerId::new("peer-1"),
                direction: Direction::Sent,
                content: format!("Message {i}"),
                timestamp: Timestamp::from_millis(i * 1000),
                delivered: false,
            };
            db.save_message(&msg).unwrap();
        }

        // Get messages before timestamp 6000 (messages 1-5), limit 3
        let messages = db
            .get_messages(&PeerId::new("peer-1"), 3, Some(Timestamp::from_millis(6000)))
            .unwrap();
        assert_eq!(messages.len(), 3);
        // Newest of the older ones first
        assert_eq!(messages[0].content, "Message 5");
        assert_eq!(messages[1].content, "Message 4");
        assert_eq!(messages[2].content, "Message 3");
    }

    #[test]
    fn message_mark_delivered() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC");

        let msg = Message {
            id: MessageId::new("msg-1"),
            peer_id: PeerId::new("peer-1"),
            direction: Direction::Sent,
            content: "Hello".to_string(),
            timestamp: Timestamp::now(),
            delivered: false,
        };
        db.save_message(&msg).unwrap();

        // Mark as delivered
        assert!(db.mark_delivered(&MessageId::new("msg-1")).unwrap());

        // Verify it's delivered now
        let messages = db.get_messages(&PeerId::new("peer-1"), 1, None).unwrap();
        assert!(messages[0].delivered);
    }

    #[test]
    fn message_mark_delivered_nonexistent() {
        let db = test_db();
        assert!(!db.mark_delivered(&MessageId::new("nonexistent")).unwrap());
    }

    #[test]
    fn unread_count() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "PC");

        // Insert 3 received undelivered messages
        for i in 1..=3 {
            let msg = Message {
                id: MessageId::new(format!("msg-{i}")),
                peer_id: PeerId::new("peer-1"),
                direction: Direction::Received,
                content: format!("Incoming {i}"),
                timestamp: Timestamp::from_millis(i * 1000),
                delivered: false,
            };
            db.save_message(&msg).unwrap();
        }

        // Insert 1 sent message (should not count as unread)
        let sent = Message {
            id: MessageId::new("msg-sent"),
            peer_id: PeerId::new("peer-1"),
            direction: Direction::Sent,
            content: "Outgoing".to_string(),
            timestamp: Timestamp::now(),
            delivered: false,
        };
        db.save_message(&sent).unwrap();

        assert_eq!(db.unread_count(&PeerId::new("peer-1")).unwrap(), 3);

        // Mark one as delivered
        db.mark_delivered(&MessageId::new("msg-1")).unwrap();
        assert_eq!(db.unread_count(&PeerId::new("peer-1")).unwrap(), 2);
    }

    #[test]
    fn spanish_characters_in_messages() {
        let db = test_db();
        insert_test_peer(&db, "peer-1", "Habitación");

        let msg = Message {
            id: MessageId::new("msg-1"),
            peer_id: PeerId::new("peer-1"),
            direction: Direction::Received,
            content: "¡Hola! ¿Cómo está la niña? Está jugando en el salón.".to_string(),
            timestamp: Timestamp::now(),
            delivered: false,
        };
        db.save_message(&msg).unwrap();

        let messages = db.get_messages(&PeerId::new("peer-1"), 1, None).unwrap();
        assert_eq!(
            messages[0].content,
            "¡Hola! ¿Cómo está la niña? Está jugando en el salón."
        );
    }
}
