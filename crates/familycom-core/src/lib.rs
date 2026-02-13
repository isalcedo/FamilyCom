//! # familycom-core
//!
//! Shared library for the FamilyCom LAN messenger.
//! Contains domain types, wire protocol, IPC protocol, database layer, and configuration.
//!
//! This crate is used by both the daemon (`familycomd`) and the TUI client (`familycom`).

pub mod config;
pub mod db;
pub mod ipc;
pub mod protocol;
pub mod types;
