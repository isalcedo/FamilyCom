# Fix: Message Persistence in TUI & Notification Click-to-Open

**Date**: 2026-02-14
**Status**: Draft

## Overview

Two bugs and one minor issue to fix:
1. Messages appear empty when switching peers or reopening the TUI — they're stored in SQLite correctly but the TUI never requests them from the daemon when navigating between peers.
2. Clicking a desktop notification does nothing — no click handler is configured in notify-rust.
3. Notifications always show "Peer" as the sender name instead of the actual display name.

## Current State

### Message Loading (Bug #1)
- Messages ARE being saved to SQLite correctly (both sent and received) in `familycomd/src/app.rs:203-225`.
- The TUI only fetches messages in ONE place: after receiving a `PeerList` response (`familycom/src/main.rs:181-188`), via `fetch_selected_peer_messages()`.
- When the user navigates peers with `NextPeer`/`PrevPeer`/`SelectPeer` actions (`familycom/src/app.rs:165-271`), no message fetch is triggered.
- On TUI restart, `PeerList` triggers a fetch for only the first selected peer; other peers' histories stay empty until the user interacts.

### Notification Click (Bug #2)
- Notifications use `notify-rust` v4 (`familycomd/Cargo.toml:43`).
- In `familycomd/src/notifications.rs:68-72`, the notification is built with only `.summary()`, `.body()`, `.timeout()` — no `.action()` or click handler.
- The tray icon's "Abrir Chat" correctly launches the TUI via `tray::open_chat_in_terminal()` (`familycomd/src/tray.rs:174`), so the launch infrastructure exists.

### Sender Name (Bug #3)
- In `familycomd/src/main.rs:269`, notifications always pass `"Peer"` as the sender name.
- The notification task subscribes to the broadcast channel and receives `ServerMessage::NewMessage { message }`, but `Message` only has `peer_id`, not `display_name`.
- The daemon broadcasts `PeerOnline { peer }` events that include `display_name`, but the notification task ignores them.

## Desired End State

1. When switching peers in the TUI (via keyboard or mouse), the selected peer's message history loads from the daemon (and thus from SQLite).
2. Clicking a desktop notification opens the TUI in a terminal window (same behavior as tray icon's "Abrir Chat").
3. Notifications show the sender's display name (e.g., "FamilyCom - PC-Sala") instead of "FamilyCom - Peer".

## Implementation Phases

### Phase 1: Fix TUI Message Fetching on Peer Switch

**Files to modify:**
- `crates/familycom/src/main.rs` — trigger message fetch after peer-switching actions

**Approach:**

In the terminal event handler (main.rs:157-173), when processing actions that aren't `SendMessage`, we need to detect if the selected peer changed and fetch messages for the new peer.

The cleanest way is to compare the selected peer ID before and after `handle_action`:

```rust
other => {
    let prev_peer = app.selected_peer_id().cloned();
    app.handle_action(other);
    let new_peer = app.selected_peer_id().cloned();
    // If the selected peer changed, fetch its message history
    if new_peer != prev_peer {
        fetch_selected_peer_messages(&app, &mut client).await;
    }
}
```

This approach:
- Catches all peer-switching actions (NextPeer, PrevPeer, SelectPeer) without enumerating them
- Also handles edge cases like clicking a peer in the list (SelectPeer)
- Always fetches fresh history from the daemon (which reads from SQLite)
- Is minimal: only changes the `other =>` branch in the match

**Note:** We always fetch even if messages are already in the HashMap — this ensures freshness after TUI restart. The cost is negligible (one IPC round-trip of ~100 messages in JSON).

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] All tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start daemon and two TUI instances on different machines (or one daemon + socat to send a test message)
- [ ] Send messages to a peer, close TUI, reopen — messages should appear
- [ ] Navigate between peers with arrow keys — each peer's messages should load
- [ ] Click on a peer in the list — its messages should load

**Implementation Note**: Pause after this phase for manual verification before proceeding.

---

### Phase 2: Add Notification Click-to-Open-TUI

**Files to modify:**
- `crates/familycomd/src/notifications.rs` — add action to notification, spawn click handler thread

**Approach:**

1. Add `.action("default", "Abrir Chat")` to the notification builder. On Linux/D-Bus, the "default" action fires when the notification body is clicked.
2. After `.show()` returns a `NotificationHandle`, spawn a std::thread to call `handle.wait_for_action()`. This method blocks until the notification is clicked, dismissed, or times out (5s).
3. If the action is "default", call `crate::tray::open_chat_in_terminal()` to launch the TUI.

```rust
let result = notify_rust::Notification::new()
    .summary(&format!("FamilyCom - {sender_name}"))
    .body(&truncated_preview)
    .action("default", "Abrir Chat")
    .timeout(notify_rust::Timeout::Milliseconds(5000))
    .show();

match result {
    Ok(handle) => {
        debug!(sender = sender_name, "notification sent");
        self.last_notification = Some(Instant::now());
        // Spawn a thread to handle the click — wait_for_action() blocks
        std::thread::spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    crate::tray::open_chat_in_terminal();
                }
            });
        });
    }
    Err(e) => {
        error!(error = %e, "failed to send notification");
    }
}
```

**Thread safety notes:**
- Each notification spawns at most one short-lived thread (blocks for ≤5s timeout)
- Rate limiting (1 notification/second) means at most ~5 concurrent threads
- `open_chat_in_terminal()` is already designed to be called from any thread (it spawns a subprocess)

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] All tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start daemon, ensure TUI is closed
- [ ] Send a message from another peer
- [ ] Notification appears with "Abrir Chat" action (or just clicking works depending on notification server)
- [ ] Click the notification — a terminal window opens with the TUI
- [ ] Verify notification timeout still works (disappears after 5 seconds if not clicked)

**Implementation Note**: Pause after this phase for manual verification before proceeding.

---

### Phase 3: Show Actual Peer Name in Notifications

**Files to modify:**
- `crates/familycomd/src/main.rs` — maintain a PeerId→display_name map in the notification task

**Approach:**

The notification task already subscribes to the daemon's broadcast channel, which sends `PeerOnline { peer }` events containing both `peer.id` and `peer.display_name`. We can build a local name lookup map within the task:

```rust
tokio::spawn(async move {
    // Local map of peer IDs to display names, built from PeerOnline events
    let mut peer_names: HashMap<PeerId, String> = HashMap::new();

    loop {
        match notification_rx.recv().await {
            Ok(ServerMessage::PeerOnline { ref peer }) => {
                // Track peer names for use in notifications
                peer_names.insert(peer.id.clone(), peer.display_name.clone());
            }
            Ok(ServerMessage::NewMessage { ref message }) => {
                if message.direction == Direction::Received {
                    let sender_name = peer_names
                        .get(&message.peer_id)
                        .map(|s| s.as_str())
                        .unwrap_or("Peer");

                    let preview = if message.content.len() > 100 {
                        format!("{}...", &message.content[..message.content.floor_char_boundary(97)])
                    } else {
                        message.content.clone()
                    };
                    notification_mgr.notify_new_message(sender_name, &preview);
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(missed = n, "notification handler lagged");
            }
            Err(broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
});
```

This approach:
- No IPC protocol changes needed
- No DB access needed from the notification task
- Peer names are available before messages arrive (mDNS discovery happens first)
- Falls back to "Peer" if the name is unknown (shouldn't happen in practice)
- Requires adding `use std::collections::HashMap` and the necessary type imports

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] All tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start daemon, check daemon.log for PeerOnline events with display names
- [ ] Send a message from a peer with a known display name (e.g., "PC-Sala")
- [ ] Notification should show "FamilyCom - PC-Sala" instead of "FamilyCom - Peer"

**Implementation Note**: Pause after this phase for manual verification before proceeding.

## Risk Assessment

- **Low risk**: All changes are small and localized. No protocol changes, no DB schema changes.
- **Phase 1**: Pure TUI-side change, daemon untouched. Easy to verify.
- **Phase 2**: The `std::thread::spawn` for each notification is the riskiest part, but rate limiting caps it at ~1 thread/second and each thread lives at most 5 seconds.
- **Phase 3**: Simple data flow change within a single tokio task in main.rs.
