//! Peer list panel (left side).
//!
//! Shows all discovered peers with their online status.
//! The selected peer is highlighted, and arrow keys navigate the list.
//!
//! ```text
//! +-- Peers --------+
//! | * PC-Sala       |  <- * = online, selected (highlighted)
//! |   Laptop-Ign    |  <- no *, offline
//! |                 |
//! +-----------------+
//! ```

use crate::app::{FocusedPanel, TuiApp};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

/// Renders the peer list panel.
pub fn render(frame: &mut Frame, app: &TuiApp, area: Rect) {
    let is_focused = app.focused == FocusedPanel::PeerList;

    // Build the border style — highlighted when focused
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Peers ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if app.peers.is_empty() {
        // Show a helpful message when no peers are found
        let empty_msg = ratatui::widgets::Paragraph::new("Buscando peers...")
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        frame.render_widget(empty_msg, area);
        return;
    }

    // Build list items from peers
    let items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|peer| {
            // Online indicator: green * for online, dim - for offline
            let (indicator, indicator_color) = if peer.online {
                ("*", Color::Green)
            } else {
                ("-", Color::DarkGray)
            };

            let name_color = if peer.online {
                Color::White
            } else {
                Color::DarkGray
            };

            let line = Line::from(vec![
                Span::styled(format!(" {indicator} "), Style::default().fg(indicator_color)),
                Span::styled(&peer.display_name, Style::default().fg(name_color)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // ListState tracks the selected index for the List widget.
    // We create it fresh each frame because ratatui is immediate-mode.
    let mut list_state = ListState::default();
    list_state.select(app.selected_peer_idx);

    frame.render_stateful_widget(list, area, &mut list_state);
}
