//! IPC protocol between the daemon and TUI clients.
//!
//! Communication happens over a Unix domain socket using JSON lines
//! (one JSON object per line, terminated by '\n'). JSON is chosen over
//! MessagePack here for easier debugging with tools like `socat`.

// Placeholder — full implementation in Phase 2
