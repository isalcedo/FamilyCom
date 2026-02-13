//! Event handling for the TUI.
//!
//! Converts raw terminal events (from crossterm) into `Action`s that
//! the `TuiApp` can process. This module is the bridge between the
//! physical keyboard and the application logic.
//!
//! # Key Bindings
//!
//! | Key          | Context     | Action                    |
//! |--------------|-------------|---------------------------|
//! | Tab          | Any         | Switch focus to next panel |
//! | Esc / q      | Not input   | Quit the TUI              |
//! | Up / k       | Peer list   | Select previous peer      |
//! | Down / j     | Peer list   | Select next peer          |
//! | PageUp       | Messages    | Scroll up (older)         |
//! | PageDown     | Messages    | Scroll down (newer)       |
//! | Enter        | Input       | Send message              |
//! | Backspace    | Input       | Delete char before cursor |
//! | Delete       | Input       | Delete char after cursor  |
//! | Left/Right   | Input       | Move cursor               |
//! | Home/End     | Input       | Jump to start/end         |
//! | Any char     | Input       | Type that character       |

use crate::app::{Action, FocusedPanel, TuiApp};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/// Converts a crossterm `Event` into an optional `Action`.
///
/// Returns `None` if the event doesn't map to any action (e.g., mouse
/// events, resize events, or keys that aren't bound to anything).
pub fn handle_event(event: &Event, app: &TuiApp) -> Option<Action> {
    match event {
        Event::Key(key_event) => handle_key_event(key_event, app),
        Event::Mouse(mouse_event) => handle_mouse_event(mouse_event, app),
        // ratatui handles resize automatically in its render loop.
        _ => None,
    }
}

/// Converts a key event into an action based on the current focus.
fn handle_key_event(key: &KeyEvent, app: &TuiApp) -> Option<Action> {
    // Ctrl+C always quits, regardless of focus
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }

    // Tab always switches focus
    if key.code == KeyCode::Tab {
        return Some(Action::NextFocus);
    }

    // Backtab (Shift+Tab) switches focus backwards
    if key.code == KeyCode::BackTab {
        // We reuse NextFocus but could add PrevFocus for reverse cycling
        return Some(Action::NextFocus);
    }

    match app.focused {
        FocusedPanel::PeerList => handle_peer_list_key(key),
        FocusedPanel::Messages => handle_messages_key(key),
        FocusedPanel::Input => handle_input_key(key),
    }
}

/// Key handling when the peer list panel is focused.
fn handle_peer_list_key(key: &KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::PrevPeer),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::NextPeer),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Key handling when the messages panel is focused.
fn handle_messages_key(key: &KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::PageUp | KeyCode::Up | KeyCode::Char('k') => Some(Action::ScrollUp),
        KeyCode::PageDown | KeyCode::Down | KeyCode::Char('j') => Some(Action::ScrollDown),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Key handling when the text input is focused.
///
/// In input mode, most keys produce text input rather than navigation.
/// Esc defocuses the input (moves focus to peer list).
fn handle_input_key(key: &KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Enter => Some(Action::SendMessage),
        KeyCode::Backspace => Some(Action::InputBackspace),
        KeyCode::Delete => Some(Action::InputDelete),
        KeyCode::Left => Some(Action::InputLeft),
        KeyCode::Right => Some(Action::InputRight),
        KeyCode::Home => Some(Action::InputHome),
        KeyCode::End => Some(Action::InputEnd),
        KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char(c) => Some(Action::InputChar(c)),
        _ => None,
    }
}

/// Converts a mouse event into an action using the saved panel rectangles.
///
/// Supports:
/// - Left click on peer list → select that peer row
/// - Left click on messages/input → focus that panel
/// - Scroll wheel in messages → scroll up/down
fn handle_mouse_event(
    mouse: &crossterm::event::MouseEvent,
    app: &TuiApp,
) -> Option<Action> {
    let col = mouse.column;
    let row = mouse.row;
    let rects = &app.panel_rects;

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(rects.peers, col, row) {
                // Clicked inside the peers panel. Compute which row was clicked.
                // Subtract 1 for the top border of the block.
                let inner_y = row.saturating_sub(rects.peers.y + 1);
                Some(Action::SelectPeer(inner_y as usize))
            } else if rect_contains(rects.messages, col, row) {
                Some(Action::FocusPanel(FocusedPanel::Messages))
            } else if rect_contains(rects.input, col, row) {
                Some(Action::FocusPanel(FocusedPanel::Input))
            } else {
                None
            }
        }

        MouseEventKind::ScrollUp if rect_contains(rects.messages, col, row) => {
            Some(Action::ScrollUp)
        }

        MouseEventKind::ScrollDown if rect_contains(rects.messages, col, row) => {
            Some(Action::ScrollDown)
        }

        _ => None,
    }
}

/// Returns `true` if the given (column, row) is inside the rectangle.
fn rect_contains(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}
