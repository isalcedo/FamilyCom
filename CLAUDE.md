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

## Build & Install
```bash
make              # debug build
make release      # optimized release build
make test         # run all 46 tests
make clippy       # lint (strict: warnings = errors)
make install      # release build + install to ~/.local/bin/ + autostart
make uninstall    # remove binaries + autostart config
```

Or directly with cargo:
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings -A dead_code
```

### Daemon subcommands
```bash
familycomd                    # start daemon (with tray)
familycomd --no-tray          # start headless
familycomd install            # set up autostart on login
familycomd install --dry-run  # preview without changes
familycomd uninstall          # remove autostart
```

### Logging
- Daemon logs to stderr + `~/.local/share/familycom/daemon.log`
- TUI logs to `~/.local/share/familycom/tui.log` (only when `FAMILYCOM_LOG` is set)
- Control log level: `FAMILYCOM_LOG=debug familycomd`

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
