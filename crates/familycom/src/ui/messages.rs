//! Message history panel (right side).
//!
//! Shows the conversation with the currently selected peer.
//! Messages are displayed chronologically (oldest at top, newest at bottom).
//!
//! ```text
//! +-- Messages (PC-Sala) -------------------------+
//! | [10:30] PC-Sala:                               |
//! | Hola, como estas?                              |
//! |                                                |
//! | [10:31] Yo:                                    |
//! | Bien! Aqui trabajando en algo chevere          |
//! +------------------------------------------------+
//! ```

use crate::app::{FocusedPanel, TuiApp};
use familycom_core::types::Direction;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Renders the message history panel.
pub fn render(frame: &mut Frame, app: &TuiApp, area: Rect) {
    let is_focused = app.focused == FocusedPanel::Messages;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Panel title includes the selected peer's name
    let title = match app.selected_peer() {
        Some(peer) => format!(" Mensajes - {} ", peer.display_name),
        None => " Mensajes ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let messages = app.current_messages();

    if messages.is_empty() {
        let empty_text = if app.selected_peer().is_some() {
            "No hay mensajes aun. Escribe algo!"
        } else {
            "Selecciona un peer para ver mensajes"
        };
        let empty_msg = Paragraph::new(empty_text)
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        frame.render_widget(empty_msg, area);
        return;
    }

    // Build the message lines.
    // Each message becomes 2+ lines: header (time + name) + content.
    let mut lines: Vec<Line> = Vec::new();

    for msg in messages {
        let time = msg.timestamp.format_local_time();

        let (name, name_color) = match msg.direction {
            Direction::Sent => ("Yo".to_string(), Color::Cyan),
            Direction::Received => {
                let peer_name = app
                    .selected_peer()
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| "???".to_string());
                (peer_name, Color::Yellow)
            }
        };

        // Delivery indicator for sent messages
        let delivery_indicator = match msg.direction {
            Direction::Sent if msg.delivered => " [ok]",
            Direction::Sent => " [...]",
            Direction::Received => "",
        };

        // Header line: [HH:MM] Name: [delivery]
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{time}] "),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{name}:"),
                Style::default()
                    .fg(name_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                delivery_indicator,
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Content line(s)
        for content_line in msg.content.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {content_line}"),
                Style::default().fg(Color::White),
            )));
        }

        // Empty line between messages for readability
        lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.messages_scroll, 0));

    frame.render_widget(paragraph, area);
}
