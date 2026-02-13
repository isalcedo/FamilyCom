//! FamilyCom Daemon — the background service that powers LAN messaging.
//!
//! This binary handles:
//! - mDNS service registration and peer discovery
//! - TCP server/client for receiving and sending messages
//! - SQLite persistence for messages and peer data
//! - IPC server (Unix socket) for TUI client connections
//! - System tray icon and desktop notifications (Phase 6)

mod client;
mod discovery;
mod server;

fn main() {
    println!("familycomd — FamilyCom daemon (placeholder, full implementation in Phase 4)");
}
