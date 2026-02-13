//! UI rendering modules for the FamilyCom TUI.
//!
//! Each module corresponds to a visual component:
//! - `layout`: The overall screen layout (three panels)
//! - `peer_list`: Left panel showing discovered peers
//! - `messages`: Right panel showing message history
//! - `input`: Bottom panel for text input

pub mod input;
pub mod layout;
pub mod messages;
pub mod peer_list;
