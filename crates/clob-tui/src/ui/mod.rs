//! Drawing.
//!
//! Split by panel rather than by widget, so each file answers one question: what does the
//! book look like, what does the tape look like. Every panel is a function of state and
//! draws nothing it was not given — there is no fetching in here, and no state either.

mod book;
mod header;
mod position;
mod tape;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::app::App;

/// Colours, in one place.
pub mod theme {
    use ratatui::style::Color;

    /// Bids and asks, as every trader reading a ladder expects them. A venue is the wrong
    /// place to be inventive about which colour means somebody is buying.
    pub const BID: Color = Color::Rgb(74, 222, 155);
    pub const ASK: Color = Color::Rgb(239, 125, 99);

    pub const TEXT: Color = Color::Rgb(228, 231, 232);
    pub const DIM: Color = Color::Rgb(108, 114, 117);
    pub const FAINT: Color = Color::Rgb(71, 75, 77);
    pub const LINE: Color = Color::Rgb(48, 52, 54);
    pub const WARN: Color = Color::Rgb(232, 201, 160);
}

/// Lays out the whole screen.
///
/// Header across the top, then the book beside the tape and the position. The book is
/// widest because it is the one read down; the others are glanced at.
pub fn draw(frame: &mut Frame, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header::draw(frame, rows[0], app);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ])
        .split(rows[1]);

    book::draw(frame, columns[0], app);
    tape::draw(frame, columns[1], app);
    position::draw(frame, columns[2], app);
    header::status(frame, rows[2], app);
}
