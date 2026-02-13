# FamilyCom — LAN Messenger Implementation Plan

## Overview

FamilyCom is a peer-to-peer LAN messaging application for home use. It auto-discovers other instances on the local network via mDNS, allows sending UTF-8 text messages (including Spanish characters like ñ, á, é), and persists messages locally in SQLite. Built in Rust with a TUI interface, system tray integration, and native desktop notifications.

## Current State

Empty project directory. Starting from scratch.

## Desired End State

A fully functional LAN messaging app consisting of:
- **`familycomd`**: Background daemon with system tray icon, mDNS discovery, TCP messaging, SQLite storage, and desktop notifications
- **`familycom`**: TUI client that connects to the local daemon to view peers, read message history, and send messages
- Autostart configuration for both macOS and Linux
- Well-commented, strongly-typed Rust code suitable for learning

## Technology Stack

| Component | Crate(s) | Purpose |
|-----------|----------|---------|
| mDNS Discovery | `mdns-sd 0.17` | Peer discovery & registration on LAN |
| TUI | `ratatui 0.30` + `crossterm 0.29` | Terminal user interface |
| System Tray | `tray-icon 0.21` + `muda 0.16` | System tray icon with menu |
| Notifications | `notify-rust 4` | Cross-platform desktop notifications |
| SQLite | `rusqlite 0.38` (bundled) | Local message persistence |
| Async Runtime | `tokio 1` (multi-thread) | Networking and async I/O |
| Serialization | `serde 1` + `rmp-serde 1` | MessagePack wire protocol |
| CLI Args | `clap 4` | Command-line argument parsing |
| Logging | `tracing` + `tracing-subscriber` | Structured logging |
| IPC | Unix domain sockets (tokio) | Daemon <-> TUI communication |
| GTK (Linux) | `gtk 0.18` | Event loop for system tray on Linux |

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    LAN (WiFi/Ethernet)              │
│                                                     │
│  ┌──────────┐    mDNS     ┌──────────┐             │
│  │ Machine A│<----------->│ Machine B│             │
│  │          │    TCP      │          │             │
│  │ daemon A │<----------->│ daemon B │             │
│  └────┬─────┘             └────┬─────┘             │
│       │ Unix Socket            │ Unix Socket       │
│  ┌────┴─────┐             ┌────┴─────┐             │
│  │  TUI A   │             │  TUI B   │             │
│  └──────────┘             └──────────┘             │
└─────────────────────────────────────────────────────┘
```

### Daemon (`familycomd`) Internal Architecture

```
Main Thread (platform event loop)
|- System tray icon (GTK on Linux, NSApplication on macOS)
|- Tray menu events -> channel -> tokio runtime

Background Thread (tokio multi-thread runtime)
|- mDNS Service
|  |- Register this peer (_familycom._tcp.local)
|  |- Browse for other peers
|- TCP Server (port from config)
|  |- Accept incoming messages -> SQLite -> notify TUI
|- TCP Client
|  |- Send outgoing messages to peer TCP servers
|- SQLite Manager (dedicated thread via tokio::spawn_blocking)
|  |- Read/write messages, peers, config
|- IPC Server (Unix socket: /tmp/familycom-{uid}.sock)
|  |- Handle TUI client connections (JSON-based protocol)
|- Notification Dispatcher
   |- Show desktop notification on incoming message
```

### TUI (`familycom`) Internal Architecture

```
Tokio Runtime
|- IPC Client (connects to daemon Unix socket)
|  |- Request peer list
|  |- Request message history
|  |- Send message command
|  |- Receive real-time updates (new messages, peer changes)
|- Terminal Event Stream (crossterm async)
|  |- Keyboard/mouse input
|- Ratatui Render Loop
   |- Left panel: Peer list (online/offline)
   |- Right panel: Message history with selected peer
   |- Bottom: Input text box + status bar
```

## IPC Protocol (Daemon <-> TUI)

JSON-based protocol over Unix socket. Each message is a JSON object terminated by newline (`\n`).

### Request types (TUI -> Daemon):

```rust
enum ClientRequest {
    /// Get list of discovered peers
    ListPeers,
    /// Get message history with a specific peer
    GetMessages { peer_id: PeerId, limit: u32, before: Option<Timestamp> },
    /// Send a message to a peer
    SendMessage { peer_id: PeerId, content: String },
    /// Get this machine's configuration
    GetConfig,
    /// Update this machine's display name
    SetDisplayName { name: String },
    /// Subscribe to real-time events
    Subscribe,
}
```

### Response/Event types (Daemon -> TUI):

```rust
enum ServerMessage {
    /// Response to ListPeers
    PeerList { peers: Vec<PeerInfo> },
    /// Response to GetMessages
    Messages { messages: Vec<Message> },
    /// Acknowledgment of sent message
    MessageSent { message_id: MessageId },
    /// Real-time: new message received
    NewMessage { message: Message },
    /// Real-time: peer came online
    PeerOnline { peer: PeerInfo },
    /// Real-time: peer went offline
    PeerOffline { peer_id: PeerId },
    /// Current config
    Config { display_name: String, peer_id: PeerId },
    /// Error response
    Error { code: String, message: String },
}
```

## Peer-to-Peer Protocol (Daemon <-> Daemon over TCP)

MessagePack-encoded frames over TCP. Each frame is length-prefixed (4 bytes big-endian length + MessagePack payload).

```rust
enum PeerMessage {
    /// Send a chat message
    Chat {
        id: MessageId,
        sender_id: PeerId,
        sender_name: String,
        content: String,     // UTF-8 text (Spanish chars: n, a, e, etc.)
        timestamp: Timestamp,
    },
    /// Acknowledge receipt of a message
    Ack { message_id: MessageId },
    /// Ping to check if peer is alive
    Ping,
    /// Response to Ping
    Pong,
}
```

## SQLite Schema

```sql
-- Unique identity of this machine
CREATE TABLE config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Keys: 'peer_id' (UUID), 'display_name' (user-chosen name), 'tcp_port'

-- Known peers discovered on the network
CREATE TABLE peers (
    id TEXT PRIMARY KEY,           -- UUID assigned by each peer
    display_name TEXT NOT NULL,    -- Human-readable name
    last_seen_at INTEGER NOT NULL, -- Unix timestamp millis
    addresses TEXT NOT NULL        -- JSON array of "ip:port" strings
);

-- All messages (sent and received)
CREATE TABLE messages (
    id TEXT PRIMARY KEY,            -- UUID
    peer_id TEXT NOT NULL,          -- The other party
    direction TEXT NOT NULL CHECK(direction IN ('sent', 'received')),
    content TEXT NOT NULL,          -- UTF-8 message body
    timestamp INTEGER NOT NULL,    -- Unix timestamp millis
    delivered INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (peer_id) REFERENCES peers(id)
);

CREATE INDEX idx_messages_peer_time ON messages(peer_id, timestamp DESC);
CREATE INDEX idx_messages_timestamp ON messages(timestamp DESC);
```

## Cargo Workspace Structure

```
FamilyCom/
├── Cargo.toml                    # Workspace root
├── CLAUDE.md                     # Project instructions
├── .env.example                  # Template for env vars
├── .gitignore
├── crates/
│   ├── familycom-core/           # Shared library crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            # Re-exports
│   │       ├── types.rs          # Strong-typed domain types
│   │       ├── protocol.rs       # PeerMessage enum + MessagePack serialization
│   │       ├── ipc.rs            # IPC protocol types (ClientRequest, ServerMessage)
│   │       ├── db.rs             # SQLite schema, migrations, queries
│   │       └── config.rs         # Configuration management
│   ├── familycomd/               # Daemon binary crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs           # Entry point, platform event loop
│   │       ├── discovery.rs      # mDNS registration and browsing
│   │       ├── server.rs         # TCP server for incoming peer messages
│   │       ├── client.rs         # TCP client for sending to peers
│   │       ├── ipc_server.rs     # Unix socket server for TUI clients
│   │       ├── tray.rs           # System tray icon and menu
│   │       ├── notifications.rs  # Desktop notifications
│   │       └── app.rs            # Central app state + coordination
│   └── familycom/                # TUI binary crate
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs           # Entry point
│           ├── app.rs            # TUI app state
│           ├── ipc_client.rs     # Unix socket client to daemon
│           ├── ui/
│           │   ├── mod.rs        # UI module root
│           │   ├── layout.rs     # Main layout (panels)
│           │   ├── peer_list.rs  # Peer list widget
│           │   ├── messages.rs   # Message history widget
│           │   └── input.rs      # Text input widget
│           └── event.rs          # Event handling (keyboard, IPC)
├── assets/
│   └── icon.png                  # Tray icon
└── config/
    ├── familycom.desktop         # Linux autostart
    └── com.familycom.daemon.plist # macOS LaunchAgent
```

---

## Phase 1: Project Scaffold

**Goal:** Initialize Cargo workspace, set up project structure, dependencies, git, and configuration files.

### Tasks:
1. `git init` the repository
2. Create `Cargo.toml` workspace with three members: `familycom-core`, `familycomd`, `familycom`
3. Create all crate directories with initial `Cargo.toml` files and placeholder `src/` files
4. Add all dependencies to appropriate `Cargo.toml` files
5. Create `.gitignore` (Rust standard + .env + target/ + *.db)
6. Create `.env.example` with placeholder config
7. Create `CLAUDE.md` with project overview
8. Verify the workspace compiles with `cargo check`

### Success Criteria:

#### Automated Verification:
- [ ] Workspace compiles: `cargo check --workspace`
- [ ] Project structure exists: `ls crates/familycom-core/src/lib.rs crates/familycomd/src/main.rs crates/familycom/src/main.rs`
- [ ] Git initialized: `git status`
- [ ] .gitignore working: `git status` should not show target/

#### Manual Verification:
- [ ] Review `Cargo.toml` files for correct dependencies
- [ ] Verify `.gitignore` contains: .env, target/, *.db

**Implementation Note**: Pause for verification before proceeding to Phase 2.

---

## Phase 2: Core Types and SQLite Layer

**Goal:** Define all strong-typed domain types, the SQLite schema, and the data access layer in `familycom-core`.

### Tasks:
1. **`types.rs`**: Define newtype wrappers and domain types:
   - `PeerId(String)` — UUID wrapper with Display, FromStr, Serialize/Deserialize
   - `MessageId(String)` — UUID wrapper
   - `DisplayName(String)` — validated display name (non-empty, max 50 chars)
   - `MessageContent(String)` — validated UTF-8 message (non-empty, max 10,000 chars)
   - `Timestamp(i64)` — Unix millis wrapper with helper methods
   - `Direction` enum: `Sent`, `Received`
   - `PeerInfo` struct: id, display_name, addresses, last_seen_at, online status
   - `Message` struct: id, peer_id, direction, content, timestamp, delivered

2. **`config.rs`**: Configuration management:
   - `AppConfig` struct with peer_id, display_name, tcp_port, db_path, socket_path
   - Load from TOML config file / environment / defaults
   - First-run detection (no config file yet)
   - Config file path: `~/.config/familycom/config.toml`

3. **`db.rs`**: SQLite data access layer:
   - `Database` struct wrapping `rusqlite::Connection`
   - `Database::open(path)` — opens DB and runs migrations
   - `Database::migrate()` — creates tables if not exist
   - `Database::save_message(msg: &Message) -> Result<()>`
   - `Database::get_messages(peer_id: &PeerId, limit: u32, before: Option<Timestamp>) -> Result<Vec<Message>>`
   - `Database::mark_delivered(message_id: &MessageId) -> Result<()>`
   - `Database::upsert_peer(peer: &PeerInfo) -> Result<()>`
   - `Database::get_peers() -> Result<Vec<PeerInfo>>`
   - `Database::get_config(key: &str) -> Result<Option<String>>`
   - `Database::set_config(key: &str, value: &str) -> Result<()>`
   - All methods return `Result<T, DatabaseError>` with custom error type via `thiserror`

4. **`protocol.rs`**: Peer-to-peer wire protocol:
   - `PeerMessage` enum with Chat, Ack, Ping, Pong variants
   - `encode(msg: &PeerMessage) -> Result<Vec<u8>>` — length-prefixed MessagePack
   - `decode(bytes: &[u8]) -> Result<PeerMessage>` — parse from bytes
   - Frame reading/writing async helpers for tokio TCP streams

5. **`ipc.rs`**: IPC protocol types:
   - `ClientRequest` enum (ListPeers, GetMessages, SendMessage, etc.)
   - `ServerMessage` enum (PeerList, Messages, NewMessage, etc.)
   - JSON serialization for IPC (human-readable for debugging)

6. **`lib.rs`**: Re-export all public types

7. **Tests**: Unit tests for each module (types validation, DB CRUD, protocol encode/decode)

### Success Criteria:

#### Automated Verification:
- [ ] Core crate compiles: `cargo check -p familycom-core`
- [ ] Tests pass: `cargo test -p familycom-core`
- [ ] No warnings: `cargo clippy -p familycom-core -- -D warnings`

#### Manual Verification:
- [ ] Review types.rs: all domain types are newtypes with proper derives
- [ ] Review db.rs: SQL queries use parameterized statements (no injection)
- [ ] Review protocol.rs: MessagePack encode/decode roundtrips correctly in tests
- [ ] Code comments explain "why" not just "what"

**Implementation Note**: Pause for verification before proceeding to Phase 3.

---

## Phase 3: Networking and Discovery

**Goal:** Implement mDNS peer discovery and TCP messaging between daemons (testable standalone).

### Tasks:
1. **`discovery.rs`** (in familycomd):
   - `DiscoveryService` struct managing mDNS daemon
   - `DiscoveryService::new(peer_id, display_name, tcp_port) -> Result<Self>`
   - Registers service: `_familycom._tcp.local.` with TXT records (peer_id, display_name)
   - Browses for other `_familycom._tcp` services
   - Emits events via channel: `DiscoveryEvent::PeerFound(PeerInfo)`, `PeerLost(PeerId)`
   - Filters out self (by peer_id)

2. **`server.rs`** (in familycomd):
   - `MessageServer` struct wrapping tokio `TcpListener`
   - `MessageServer::bind(addr) -> Result<Self>`
   - `MessageServer::accept_loop()` — accepts connections, reads length-prefixed frames
   - Per-connection handler: reads `PeerMessage` frames, responds with Ack
   - Emits received messages via channel

3. **`client.rs`** (in familycomd):
   - `MessageClient` — sends messages to peer TCP servers
   - `MessageClient::send(addr, message) -> Result<()>`
   - Connect-per-message for simplicity (connection pooling later if needed)
   - Handles Ack reception
   - Timeout on send to handle unreachable peers

4. **Integration test**: Two discovery services + TCP server/client communicating on localhost

### Success Criteria:

#### Automated Verification:
- [ ] Daemon crate compiles: `cargo check -p familycomd`
- [ ] Integration tests pass: `cargo test -p familycomd`
- [ ] Clippy clean: `cargo clippy -p familycomd -- -D warnings`

#### Manual Verification:
- [ ] Run two instances on the same machine (different ports) and verify they discover each other via mDNS
- [ ] Send a message from instance A to instance B over TCP and verify it arrives
- [ ] Verify MessagePack framing works correctly (length prefix + payload)

**Implementation Note**: Pause for verification before proceeding to Phase 4.

---

## Phase 4: Daemon Core

**Goal:** Build the complete daemon binary integrating discovery, messaging, SQLite, and IPC (without system tray and notifications yet).

### Tasks:
1. **`app.rs`** (in familycomd):
   - `DaemonApp` struct: central coordinator
   - Owns: Database (behind Mutex), DiscoveryService, MessageServer, connected TUI clients list
   - Event loop using `tokio::select!`:
     - mDNS discovery events -> update DB, notify TUI clients
     - Incoming TCP messages -> save to DB, notify TUI clients
     - IPC requests from TUI -> handle and respond
   - `DaemonApp::run()` — main async loop

2. **`ipc_server.rs`** (in familycomd):
   - `IpcServer` struct wrapping tokio `UnixListener`
   - Socket path: `$XDG_RUNTIME_DIR/familycom.sock` or `/tmp/familycom-{uid}.sock`
   - Accepts TUI client connections
   - Per-client handler: reads `ClientRequest` (JSON lines), sends `ServerMessage`
   - Subscription: clients that send `Subscribe` get real-time events pushed
   - Supports multiple concurrent TUI clients

3. **`main.rs`** (in familycomd):
   - Parse CLI args with clap (--port, --db-path, --name, --config)
   - First-run setup: generate peer_id, prompt for display_name if interactive terminal
   - Initialize tracing logging
   - Start tokio runtime, create and run DaemonApp
   - Graceful shutdown on SIGINT/SIGTERM (clean up socket file, unregister mDNS)
   - (System tray comes in Phase 6 — runs headless in terminal for now)

4. **Config file**: `~/.config/familycom/config.toml`
   - Stores peer_id, display_name, tcp_port
   - Created on first run

### Success Criteria:

#### Automated Verification:
- [ ] Daemon compiles and shows help: `cargo run -p familycomd -- --help`
- [ ] Tests pass: `cargo test -p familycomd`
- [ ] Clippy clean: `cargo clippy -p familycomd -- -D warnings`

#### Manual Verification:
- [ ] Start daemon: `cargo run -p familycomd`
- [ ] First run prompts for display name and saves config
- [ ] Daemon logs show mDNS registration and browsing activity
- [ ] Unix socket is created at expected path
- [ ] Connect to socket with `socat` and send a `{"ListPeers":{}}` JSON request — get a response
- [ ] Ctrl+C cleanly shuts down (removes socket, logs shutdown)

**Implementation Note**: Pause for verification before proceeding to Phase 5.

---

## Phase 5: TUI Client

**Goal:** Build the terminal user interface that connects to the daemon and provides full chat functionality.

### Tasks:
1. **`ipc_client.rs`** (in familycom):
   - `IpcClient` struct wrapping tokio `UnixStream`
   - `IpcClient::connect() -> Result<Self>` — finds and connects to daemon socket
   - `IpcClient::send(request: ClientRequest) -> Result<()>`
   - `IpcClient::recv() -> Result<ServerMessage>`
   - Auto-subscribes to real-time events on connect
   - If daemon not running: shows clear error message with instructions

2. **`app.rs`** (in familycom):
   - `TuiApp` struct: manages application state
   - Fields: peers list, selected_peer index, messages for selected peer, input buffer, scroll position
   - `TuiApp::handle_event(event) -> Option<Action>` — processes events
   - State machine: NoPeers -> PeerSelected -> Chatting

3. **`ui/layout.rs`**:
   - Three-panel layout:
     ```
     +-------------+--------------------------+
     |  Peers (20%)|  Messages (80%)          |
     |             |                          |
     | * PC-Sala   |  [10:30] PC-Sala:        |
     |   Laptop    |  Hola! Como estas?       |
     |             |                          |
     |             |  [10:31] Yo:             |
     |             |  Bien! Aqui trabajando   |
     |             |                          |
     +-------------+--------------------------+
     | > Escribe un mensaje...                |
     +-----------+----------------------------+
     | FamilyCom v0.1 | 2 peers | Connected  |
     +----------------------------------------+
     ```

4. **`ui/peer_list.rs`**:
   - List widget showing discovered peers
   - Online indicator (asterisk * for online, dash - for offline)
   - Highlight selected peer
   - Navigate with Up/Down or j/k

5. **`ui/messages.rs`**:
   - Scrollable message history for selected peer
   - Timestamps formatted as local time
   - Sent messages marked "Yo:" / Received marked with peer name
   - Full UTF-8 rendering (Spanish characters)
   - Scroll with PageUp/PageDown

6. **`ui/input.rs`**:
   - Text input widget with cursor
   - Enter sends message
   - Full UTF-8 input support (n, a, e, etc.)

7. **`event.rs`**:
   - Event handling combining terminal events + IPC events
   - `tokio::select!` over crossterm EventStream and IPC messages
   - Key bindings:
     - `Tab`: switch focus between panels
     - `Esc` or `q` (when not in input): quit TUI
     - `Enter` (in input): send message
     - `Up/Down` or `j/k` (in peer list): navigate peers
     - `PageUp/PageDown` (in messages): scroll history

8. **`main.rs`** (in familycom):
   - Parse CLI args
   - Connect to daemon (error if not running)
   - Initialize terminal (crossterm raw mode, alternate screen)
   - Run TUI event loop
   - Restore terminal on exit (including on panic via panic hook)

### Success Criteria:

#### Automated Verification:
- [ ] TUI compiles: `cargo build -p familycom`
- [ ] Tests pass: `cargo test -p familycom`
- [ ] Clippy clean: `cargo clippy -p familycom -- -D warnings`

#### Manual Verification:
- [ ] Start daemon, then TUI: `cargo run -p familycomd` then `cargo run -p familycom`
- [ ] TUI shows list of discovered peers (test with two daemons on different ports)
- [ ] Select a peer and see message history
- [ ] Type a message with Spanish characters (n, a, e, i, o, u) — sends correctly
- [ ] Receive a message from another peer — appears in real-time
- [ ] Keyboard navigation works (Tab, arrows, Enter, Esc)
- [ ] TUI restores terminal cleanly on exit (no garbled terminal)

**Implementation Note**: Pause for verification before proceeding to Phase 6.

---

## Phase 6: System Tray and Notifications

**Goal:** Add system tray icon to the daemon and native desktop notifications for incoming messages.

### Tasks:
1. **`tray.rs`** (in familycomd):
   - `TrayManager` struct
   - Create tray icon with FamilyCom icon (embedded in binary via `include_bytes!`)
   - Menu items:
     - "Open Chat" -> launches terminal with `familycom`
     - "Status: Online (N peers)" -> informational, updated dynamically
     - separator
     - "Quit" -> sends shutdown signal
   - Platform-specific event loop:
     - Linux: GTK `gtk::main()` on dedicated thread
     - macOS: NSApplication run loop on main thread
   - Communication with tokio runtime via channels

2. **`notifications.rs`** (in familycomd):
   - `NotificationManager` struct
   - `notify_new_message(sender_name: &str, preview: &str)`
   - Uses `notify-rust` for cross-platform notifications
   - Rate limiting: max 1 notification per second to avoid spam
   - Don't notify for messages sent by self

3. **`main.rs`** update:
   - Platform-conditional main function:
     - Linux: spawn GTK thread for tray, tokio on main thread
     - macOS: NSApplication on main thread, tokio on background thread
   - Wire tray events to daemon (Open Chat, Quit)
   - Wire incoming messages to notification manager

4. **`assets/icon.png`**: Simple tray icon (speech bubble or "FC" letters, 32x32)

5. **"Open Chat" action**:
   - Detect available terminal emulator and launch `familycom` in it
   - Configurable via `terminal_command` in config.toml
   - Defaults: Linux -> x-terminal-emulator, macOS -> open -a Terminal

### Success Criteria:

#### Automated Verification:
- [ ] Full workspace compiles: `cargo build --workspace`
- [ ] Tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings`

#### Manual Verification:
- [ ] Start daemon — tray icon appears in system tray
- [ ] Right-click tray icon — menu shows "Open Chat", status, "Quit"
- [ ] Click "Open Chat" — terminal opens with TUI
- [ ] Receive message when TUI is closed — desktop notification appears
- [ ] Click "Quit" in tray menu — daemon shuts down cleanly
- [ ] On Linux: verify GTK thread doesn't block networking
- [ ] On macOS: verify tray works on main thread while tokio runs on background

**Implementation Note**: Pause for verification. This phase has the most platform-specific code and may need iteration.

---

## Phase 7: Autostart, Polish, and First-Run Experience

**Goal:** Configure autostart, improve first-run experience, error handling, and polish.

### Tasks:
1. **Autostart configuration**:
   - **Linux**: `~/.config/autostart/familycom.desktop` file
   - **macOS**: `~/Library/LaunchAgents/com.familycom.daemon.plist`
   - `familycomd install` subcommand: creates autostart config with correct binary path
   - `familycomd uninstall` subcommand: removes autostart config

2. **First-run experience**:
   - Daemon starts with no config:
     1. Generate random UUID for peer_id
     2. If interactive terminal: prompt for display name
     3. If non-interactive (autostart): use hostname
     4. Save to `~/.config/familycom/config.toml`
   - TUI: if no daemon running, show helpful error with command to start it

3. **Display name change**:
   - `familycom --set-name "Nuevo Nombre"` CLI option
   - Propagates to mDNS re-registration and notifies connected peers

4. **Error handling polish**:
   - Daemon: handle network interface changes (WiFi reconnect)
   - TUI: handle daemon disconnection (show "Reconnecting..." status)
   - TUI: handle terminal resize events
   - Both: structured error types with `thiserror`

5. **Logging**:
   - Daemon: log to file `~/.local/share/familycom/daemon.log`
   - TUI: log to file (not terminal, would break TUI)
   - Log level via `FAMILYCOM_LOG` env var or config

6. **Build and install**:
   - `Makefile` with targets: build, install, uninstall
   - `install`: copies binaries to `~/.local/bin/` + runs autostart setup

### Success Criteria:

#### Automated Verification:
- [ ] Release build: `cargo build --workspace --release`
- [ ] All tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings`
- [ ] Install dry-run: `cargo run -p familycomd -- install --dry-run`

#### Manual Verification:
- [ ] Fresh start: delete config, run daemon — prompts for name, creates config
- [ ] Autostart: install, logout/login, verify daemon starts automatically
- [ ] Change display name: other peers see updated name
- [ ] Kill WiFi, reconnect — daemon recovers and re-discovers peers
- [ ] TUI handles daemon restart: shows "Reconnecting..." then reconnects
- [ ] Run `familycom` when daemon is not running — shows helpful error
- [ ] Send messages with Spanish characters end-to-end between two machines
- [ ] Check logs at ~/.local/share/familycom/daemon.log

**Implementation Note**: Final phase. Full end-to-end testing on both macOS and Linux recommended.

---

## Build Dependencies (system packages)

**Linux (Arch):**
```bash
sudo pacman -S gtk3 libappindicator-gtk3 xdotool pkg-config
```

**Linux (Debian/Ubuntu):**
```bash
sudo apt install libgtk-3-dev libappindicator3-dev libxdo-dev pkg-config
```

**macOS:**
```bash
xcode-select --install  # C compiler for rusqlite bundled SQLite
```

---

## Risk Assessment

| Risk | Mitigation |
|------|------------|
| System tray event loop conflicts with tokio | Platform-conditional threading (GTK thread on Linux, NSApp main thread on macOS) |
| mDNS not working across VLANs/subnets | Document: app is for single LAN segment (home use) |
| SQLite concurrent access | Single connection behind Mutex, or dedicated DB thread |
| macOS notification bundle ID | Include Info.plist if needed; test on actual macOS |
| GTK dependency on Linux | Document required packages; consider optional feature flag |
