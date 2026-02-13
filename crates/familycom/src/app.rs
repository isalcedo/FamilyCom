//! TUI application state management.
//!
//! `TuiApp` holds all the state needed to render the terminal UI and
//! process user input. It follows the **Elm Architecture** pattern
//! (also known as TEA or Model-View-Update):
//!
//! 1. **Model**: `TuiApp` struct holds the state
//! 2. **Update**: `handle_action()` modifies state based on events
//! 3. **View**: the `ui/` modules render the state to the terminal
//!
//! This separation makes the app easy to test and reason about.

use familycom_core::ipc::ServerMessage;
use familycom_core::types::{Message, PeerId, PeerInfo};
use std::collections::HashMap;

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPanel {
    /// The peer list (left panel). Arrow keys navigate peers.
    PeerList,
    /// The message history (right panel). PageUp/PageDown scrolls.
    Messages,
    /// The text input (bottom). Typing composes a message.
    Input,
}

/// Actions that modify the application state.
///
/// These are produced by the event handler and consumed by the app.
/// This indirection keeps input handling separate from state mutation.
#[derive(Debug)]
pub enum Action {
    /// User wants to quit the TUI.
    Quit,
    /// Switch focus to the next panel (Tab).
    NextFocus,
    /// Select the next peer in the list (Down / j).
    NextPeer,
    /// Select the previous peer in the list (Up / k).
    PrevPeer,
    /// Scroll messages up (older).
    ScrollUp,
    /// Scroll messages down (newer).
    ScrollDown,
    /// Append a character to the input buffer.
    InputChar(char),
    /// Delete the character before the cursor.
    InputBackspace,
    /// Delete the character after the cursor.
    InputDelete,
    /// Move cursor left.
    InputLeft,
    /// Move cursor right.
    InputRight,
    /// Move cursor to start of input.
    InputHome,
    /// Move cursor to end of input.
    InputEnd,
    /// Send the current input as a message.
    SendMessage,
    /// A server message was received from the daemon.
    ServerMessage(ServerMessage),
}

/// The main TUI application state.
pub struct TuiApp {
    /// All known peers (from daemon).
    pub peers: Vec<PeerInfo>,
    /// Index of the currently selected peer in the `peers` list.
    pub selected_peer_idx: Option<usize>,
    /// Message history per peer (keyed by PeerId).
    /// Messages are stored oldest-first for display.
    pub messages: HashMap<PeerId, Vec<Message>>,
    /// The text input buffer (what the user is currently typing).
    pub input: String,
    /// Cursor position within the input string (byte offset).
    pub input_cursor: usize,
    /// Which panel currently has focus.
    pub focused: FocusedPanel,
    /// Scroll offset for the messages panel (0 = bottom / newest).
    pub messages_scroll: u16,
    /// Our display name (from daemon config).
    pub our_name: String,
    /// Our peer ID (from daemon config).
    pub our_peer_id: Option<PeerId>,
    /// Status message shown in the bottom bar.
    pub status: String,
    /// Whether the app should exit.
    pub should_quit: bool,
}

impl TuiApp {
    /// Creates a new TUI app with empty state.
    pub fn new() -> Self {
        Self {
            peers: Vec::new(),
            selected_peer_idx: None,
            messages: HashMap::new(),
            input: String::new(),
            input_cursor: 0,
            focused: FocusedPanel::PeerList,
            messages_scroll: 0,
            our_name: String::new(),
            our_peer_id: None,
            status: "Connecting...".to_string(),
            should_quit: false,
        }
    }

    /// Returns the currently selected peer, if any.
    pub fn selected_peer(&self) -> Option<&PeerInfo> {
        self.selected_peer_idx
            .and_then(|idx| self.peers.get(idx))
    }

    /// Returns the PeerId of the currently selected peer, if any.
    pub fn selected_peer_id(&self) -> Option<&PeerId> {
        self.selected_peer().map(|p| &p.id)
    }

    /// Returns the messages for the currently selected peer.
    pub fn current_messages(&self) -> &[Message] {
        self.selected_peer_id()
            .and_then(|id| self.messages.get(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Processes an action and updates the state accordingly.
    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.should_quit = true;
            }

            Action::NextFocus => {
                // Cycle through: PeerList -> Messages -> Input -> PeerList
                self.focused = match self.focused {
                    FocusedPanel::PeerList => FocusedPanel::Messages,
                    FocusedPanel::Messages => FocusedPanel::Input,
                    FocusedPanel::Input => FocusedPanel::PeerList,
                };
            }

            Action::NextPeer => {
                if self.peers.is_empty() {
                    return;
                }
                self.selected_peer_idx = Some(match self.selected_peer_idx {
                    Some(idx) => (idx + 1).min(self.peers.len() - 1),
                    None => 0,
                });
                // Reset scroll when switching peers
                self.messages_scroll = 0;
            }

            Action::PrevPeer => {
                if self.peers.is_empty() {
                    return;
                }
                self.selected_peer_idx = Some(match self.selected_peer_idx {
                    Some(idx) => idx.saturating_sub(1),
                    None => 0,
                });
                self.messages_scroll = 0;
            }

            Action::ScrollUp => {
                self.messages_scroll = self.messages_scroll.saturating_add(3);
            }

            Action::ScrollDown => {
                self.messages_scroll = self.messages_scroll.saturating_sub(3);
            }

            Action::InputChar(ch) => {
                self.input.insert(self.input_cursor, ch);
                self.input_cursor += ch.len_utf8();
            }

            Action::InputBackspace => {
                if self.input_cursor > 0 {
                    // Find the previous character boundary
                    let prev = self.input[..self.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    self.input.drain(prev..self.input_cursor);
                    self.input_cursor = prev;
                }
            }

            Action::InputDelete => {
                if self.input_cursor < self.input.len() {
                    // Find the next character boundary
                    let next_char_len = self.input[self.input_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.input
                        .drain(self.input_cursor..self.input_cursor + next_char_len);
                }
            }

            Action::InputLeft => {
                if self.input_cursor > 0 {
                    self.input_cursor = self.input[..self.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                }
            }

            Action::InputRight => {
                if self.input_cursor < self.input.len() {
                    let next_char_len = self.input[self.input_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.input_cursor += next_char_len;
                }
            }

            Action::InputHome => {
                self.input_cursor = 0;
            }

            Action::InputEnd => {
                self.input_cursor = self.input.len();
            }

            Action::SendMessage => {
                // Handled externally (needs IPC client) — just clear input
                // The caller checks this action and sends via IPC before clearing.
            }

            Action::ServerMessage(msg) => {
                self.handle_server_message(msg);
            }
        }
    }

    /// Processes a message from the daemon.
    fn handle_server_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::PeerList { peers } => {
                self.peers = peers;
                // Ensure selected index is still valid
                if let Some(idx) = self.selected_peer_idx {
                    if idx >= self.peers.len() {
                        self.selected_peer_idx = if self.peers.is_empty() {
                            None
                        } else {
                            Some(self.peers.len() - 1)
                        };
                    }
                } else if !self.peers.is_empty() {
                    self.selected_peer_idx = Some(0);
                }
                let n = self.peers.len();
                self.status = format!("{n} peer{}", if n == 1 { "" } else { "s" });
            }

            ServerMessage::Messages { messages } => {
                // Messages come newest-first from the DB. Reverse them
                // for display (oldest-first, chronological order).
                if let Some(peer_id) = messages.first().map(|m| m.peer_id.clone()) {
                    let mut msgs = messages;
                    msgs.reverse();
                    self.messages.insert(peer_id, msgs);
                }
            }

            ServerMessage::NewMessage { message } => {
                // Add the new message to the correct peer's history
                let peer_id = message.peer_id.clone();
                self.messages
                    .entry(peer_id)
                    .or_default()
                    .push(message);
                // Reset scroll to show the newest message
                self.messages_scroll = 0;
            }

            ServerMessage::MessageSent { message_id: _ } => {
                // The message was already added to our local messages
                // when we sent it. Nothing to do here.
            }

            ServerMessage::PeerOnline { peer } => {
                // Update or add the peer in our list
                if let Some(existing) = self.peers.iter_mut().find(|p| p.id == peer.id) {
                    existing.online = true;
                    existing.display_name = peer.display_name;
                    existing.addresses = peer.addresses;
                } else {
                    self.peers.push(peer);
                }
                let n = self.peers.len();
                self.status = format!("{n} peer{}", if n == 1 { "" } else { "s" });
            }

            ServerMessage::PeerOffline { peer_id } => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.id == peer_id) {
                    peer.online = false;
                }
            }

            ServerMessage::MessageDelivered { message_id } => {
                // Mark the message as delivered in our local state
                for messages in self.messages.values_mut() {
                    if let Some(msg) = messages.iter_mut().find(|m| m.id == message_id) {
                        msg.delivered = true;
                        break;
                    }
                }
            }

            ServerMessage::Config {
                display_name,
                peer_id,
            } => {
                self.our_name = display_name;
                self.our_peer_id = Some(peer_id);
            }

            ServerMessage::Error { code, message } => {
                self.status = format!("Error [{code}]: {message}");
            }

            ServerMessage::Ok => {}
        }
    }

    /// Takes the current input content and clears the input buffer.
    /// Returns the content that was in the buffer.
    pub fn take_input(&mut self) -> String {
        let content = self.input.clone();
        self.input.clear();
        self.input_cursor = 0;
        content
    }
}
