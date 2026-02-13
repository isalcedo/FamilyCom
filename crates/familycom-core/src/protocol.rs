//! Peer-to-peer wire protocol.
//!
//! Defines the messages exchanged between FamilyCom daemons over TCP.
//! Messages are serialized as MessagePack and transmitted as length-prefixed frames:
//! [4 bytes big-endian length][MessagePack payload]

// Placeholder — full implementation in Phase 2
