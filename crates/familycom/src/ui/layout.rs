//! Main screen layout.
//!
//! Divides the terminal into three areas:
//!
//! ```text
//! +-- Peers --------+-- Messages ----------------------+
//! | * PC-Sala       | [10:30] PC-Sala:                 |
//! |   Laptop        | Hola, como estas?                |
//! |                 |                                  |
//! |                 | [10:31] Yo:                      |
//! |                 | Bien! Aqui trabajando            |
//! +-----------------+----------------------------------+
//! | > escribe un mensaje...                            |
//! +----------------------------------------------------+
//! | FamilyCom v0.1.0 | 2 peers | Connected            |
//! +----------------------------------------------------+
//! ```
//!
//! Uses ratatui's `Layout` with `Constraint`s to define proportional
//! and fixed-size regions.

use crate::app::TuiApp;
use crate::ui::{input, messages, peer_list};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Renders the complete TUI to the given frame.
///
/// This is the top-level render function called on every frame.
/// It divides the screen into regions and delegates to sub-renderers.
pub fn render(frame: &mut Frame, app: &mut TuiApp) {
    let size = frame.area();

    // Main vertical layout: content area + input + status bar
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Content (peers + messages)
            Constraint::Length(3), // Input box
            Constraint::Length(1), // Status bar
        ])
        .split(size);

    let content_area = vertical[0];
    let input_area = vertical[1];
    let status_area = vertical[2];

    // Horizontal split for content: peers list | messages
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25), // Peer list (25% width)
            Constraint::Percentage(75), // Messages (75% width)
        ])
        .split(content_area);

    let peers_area = horizontal[0];
    let messages_area = horizontal[1];

    // Save panel rectangles for mouse hit-testing
    app.panel_rects.peers = peers_area;
    app.panel_rects.messages = messages_area;
    app.panel_rects.input = input_area;

    // Render each panel
    peer_list::render(frame, app, peers_area);
    messages::render(frame, app, messages_area);
    input::render(frame, app, input_area);
    render_status_bar(frame, app, status_area);
}

/// Renders the status bar at the bottom of the screen.
fn render_status_bar(frame: &mut Frame, app: &TuiApp, area: Rect) {
    let online_count = app.peers.iter().filter(|p| p.online).count();
    let total_count = app.peers.len();

    let status_text = Line::from(vec![
        Span::styled(
            " FamilyCom v0.1.0 ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("| "),
        Span::styled(
            format!("{online_count}/{total_count} peers online"),
            Style::default().fg(if online_count > 0 {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw(" | "),
        Span::styled(&app.status, Style::default().fg(Color::DarkGray)),
        Span::raw(" | "),
        Span::styled(
            app.our_name.to_string(),
            Style::default().fg(Color::Yellow),
        ),
    ]);

    let status_bar = Paragraph::new(status_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(status_bar, area);
}
