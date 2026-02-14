# Fix mDNS Peer Discovery Between Two Machines

## Overview

Two FamilyCom instances on the same LAN cannot discover each other during normal
operation. Peers only briefly appear when the other daemon shuts down (goodbye
packets). Root cause: IPv6 is not actually being disabled in the mDNS library due
to incorrect rule ordering, and a secondary bug prevents peers from being marked
offline correctly.

## Current State

- **discovery.rs:117-133**: The `disable_interface(IfKind::IPv6)` call is placed
  BEFORE `enable_interface(IfKind::Name(...))`. Since mdns-sd processes selections
  as an ordered list where the **last matching rule wins**, the named enable
  overrides the IPv6 disable. Logs confirm: both IPv4 and IPv6 addresses are active.

- **app.rs:289-303**: `ServiceRemoved` provides the mDNS fullname (e.g.
  `"ChuiMachine._familycom._tcp.local."`), but the code creates a `PeerId` from it
  and compares against UUID-based PeerIds. They never match, so peers are never
  correctly removed from `online_peers`.

- **discovery.rs:314-316**: `ServiceFound` events (intermediate step before
  `ServiceResolved`) are silently ignored with no logging, making debugging
  difficult.

## Desired End State

- Two FamilyCom instances on the same LAN discover each other within seconds of
  starting.
- When one instance shuts down, the other correctly marks it as offline.
- Diagnostic logging helps debug discovery issues in the field.

## Implementation Phases

### Phase 1: Fix IPv6 Disable Ordering (Root Cause)

**Files**: `crates/familycomd/src/discovery.rs`

The mdns-sd crate's `if_selections` is a `Vec<IfSelection>` processed sequentially —
last matching rule wins. The fix is to call `disable_interface(IfKind::IPv6)` AFTER
`enable_interface(IfKind::Name(...))`.

**Current order** (broken):
```rust
daemon.disable_interface(IfKind::All)?;        // All → false
daemon.disable_interface(IfKind::IPv6)?;        // IPv6 → false (redundant)
daemon.enable_interface(IfKind::Name(iface))?;  // Name → true (OVERRIDES IPv6!)
```

**Fixed order**:
```rust
daemon.disable_interface(IfKind::All)?;        // All → false
daemon.enable_interface(IfKind::Name(iface))?;  // Name → true
daemon.disable_interface(IfKind::IPv6)?;        // IPv6 → false (WINS for IPv6 addrs)
```

For `fe80::...` on `wlp0s20f3`: All→false, Name→true, IPv6→false. **Result: disabled** (correct).
For `192.168.68.70` on `wlp0s20f3`: All→false, Name→true, IPv6 doesn't match. **Result: enabled** (correct).

Update the comment to explain the "last rule wins" semantics.

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] Tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start `familycomd` on one machine — logs should show ONLY IPv4 address, NOT `fe80::` IPv6
- [ ] Start `familycomd` on second machine — both should discover each other within ~5 seconds
- [ ] The "conflict handler" warnings about AAAA records should be gone from the logs

**Implementation Note**: This is the critical fix. Test on two machines before proceeding.

---

### Phase 2: Fix PeerLost Matching by mDNS Fullname

**Files**: `crates/familycomd/src/discovery.rs`, `crates/familycomd/src/app.rs`

Currently, `ServiceRemoved` provides the mDNS fullname (e.g.
`"ChuiMachine._familycom._tcp.local."`) but the code creates a `PeerId::new(fullname)`
and tries to match it against UUID-based peer IDs. This never matches.

**Fix in discovery.rs**: Change `ServiceRemoved` handler to emit the raw fullname as a
string, not wrapped in `PeerId`. Add a new variant to `DiscoveryEvent`:

```rust
pub enum DiscoveryEvent {
    PeerFound(PeerInfo),
    PeerLost(PeerId),
    /// Service removed by mDNS fullname (used when we don't have the UUID)
    ServiceRemoved(String),
}
```

**Fix in app.rs**: Handle `ServiceRemoved` by searching `online_peers` for a peer whose
display name matches the instance name portion of the mDNS fullname. Extract the
display name by stripping the `._familycom._tcp.local.` suffix.

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] Tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start both daemons — they discover each other
- [ ] Stop one daemon (Ctrl+C) — the other logs "peer went offline" with the correct peer_id
- [ ] The TUI (if connected) shows the peer going offline

**Implementation Note**: Test both directions (stop A → B sees it, stop B → A sees it).

---

### Phase 3: Add Diagnostic Logging for Browse Events

**Files**: `crates/familycomd/src/discovery.rs`

Add `info!` logging for `ServiceFound` events in the browse_loop. This is the
intermediate step before `ServiceResolved` and helps diagnose whether the library is
finding services but failing to resolve them.

```rust
ServiceEvent::ServiceFound(service_type, fullname) => {
    info!(service_type, fullname, "mDNS service found (pending resolution)");
}
```

### Success Criteria:

#### Automated Verification:
- [ ] Build succeeds: `cargo build --workspace`
- [ ] Tests pass: `cargo test --workspace`
- [ ] Clippy clean: `cargo clippy --workspace -- -D warnings -A dead_code`

#### Manual Verification:
- [ ] Start both daemons — logs show "mDNS service found" followed by "peer found" for each peer
- [ ] The ServiceFound log appears before ServiceResolved, confirming the browse→resolve pipeline works

**Implementation Note**: This is diagnostic improvement only. No behavior change.
