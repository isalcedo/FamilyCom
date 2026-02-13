//! Text input panel (bottom).
//!
//! Shows a text box where the user types messages. Supports full UTF-8
//! input including Spanish characters (ñ, á, é, í, ó, ú).
//!
//! ```text
//! +-- Escribe un mensaje... -----------------------+
//! | > Hola, como estas?|                           |
//! +----------------------------------------------------+
//! ```
//!
//! The cursor is shown as a blinking block when the input is focused.

use crate::app::{FocusedPanel, TuiApp};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Renders the text input panel.
pub fn render(frame: &mut Frame, app: &TuiApp, area: Rect) {
    let is_focused = app.focused == FocusedPanel::Input;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_focused {
        " Escribe un mensaje (Enter para enviar) "
    } else {
        " Escribe un mensaje... "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    // Display the input text with a ">" prompt
    let display_text = if app.input.is_empty() && !is_focused {
        String::new()
    } else {
        format!("> {}", app.input)
    };

    let input_widget = Paragraph::new(display_text)
        .style(Style::default().fg(Color::White))
        .block(block);

    frame.render_widget(input_widget, area);

    // Position the cursor when the input is focused.
    // ratatui doesn't render a cursor by default — we need to
    // explicitly tell the terminal where to place it.
    if is_focused {
        // +2 for the border (1) and "> " prefix (2), -1 for 0-indexing
        // The cursor_x offset accounts for the "> " prefix (2 chars)
        // plus the current cursor position in the input text.
        let cursor_x = area.x + 1 + 2 + visual_cursor_offset(&app.input, app.input_cursor) as u16;
        let cursor_y = area.y + 1; // +1 for the top border
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Calculates the visual column offset for the cursor.
///
/// Because we're dealing with UTF-8 strings, the byte offset (input_cursor)
/// may not equal the visual column position. Each character contributes
/// one column regardless of its byte length. This is a simplification
/// that works well for Western scripts and Spanish characters.
fn visual_cursor_offset(input: &str, byte_cursor: usize) -> usize {
    // Count the number of characters before the cursor position
    input[..byte_cursor].chars().count()
}
