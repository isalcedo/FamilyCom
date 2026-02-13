# FamilyCom — LAN Messenger

## Project Overview
Peer-to-peer LAN messaging app for home use. Auto-discovers peers via mDNS, sends UTF-8 text messages over TCP, persists messages in SQLite.

## Architecture
- **Cargo workspace** with 3 crates:
  - `familycom-core` — shared types, protocol, DB, config
  - `familycomd` — background daemon (mDNS, TCP, SQLite, IPC, tray, notifications)
  - `familycom` — TUI client (ratatui, connects to daemon via Unix socket)

## Key Design Decisions
- **Strong typing**: all IDs are newtypes (PeerId, MessageId), not raw strings
- **Two binaries**: daemon runs in background with tray; TUI opens/closes independently
- **MessagePack** for peer-to-peer wire protocol (compact, self-describing)
- **JSON lines** for daemon<->TUI IPC (debuggable with socat)
- **Platform-conditional threading**: GTK tray thread on Linux, NSApp main thread on macOS

## Build
```bash
cargo build --workspace          # debug build
cargo build --workspace --release # release build
cargo test --workspace           # run all tests
cargo clippy --workspace -- -D warnings  # lint
```

## System Dependencies
### Linux (Arch)
```bash
sudo pacman -S gtk3 libappindicator-gtk3 xdotool pkg-config
```
### Linux (Debian/Ubuntu)
```bash
sudo apt install libgtk-3-dev libappindicator3-dev libxdo-dev pkg-config
```
### macOS
```bash
xcode-select --install
```

## Code Style
- Well-commented code (suitable for learning Rust)
- Comments explain "why" not "what"
- All domain types are newtypes with proper trait impls
- Use `thiserror` for library errors, `anyhow` for binary errors
- Prefer `Result<T, E>` over unwrap/expect in library code
