//! The order book.
//!
//! Asks descend to the spread and bids run down from it — the arrangement every trader
//! already reads, with the best of both sides meeting in the middle so the spread is where
//! the eye lands rather than something to hunt for.
//!
//! Depth bars are scaled to the largest level *shown*, not the largest in the book. A bar
//! scaled to something off-screen conveys nothing.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::theme;
use crate::app::App;
use crate::feed::Level;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE))
        .title(Span::styled(" book ", Style::default().fg(theme::DIM)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Two sides plus the spread row, split evenly, so the ladder stays centred as the
    // terminal is resized rather than pinning to the top.
    let rows = inner.height.saturating_sub(2) as usize;
    let per_side = (rows / 2).max(1);

    let asks: Vec<&Level> = app.feed.asks.iter().take(per_side).collect();
    let bids: Vec<&Level> = app.feed.bids.iter().take(per_side).collect();

    let widest = asks
        .iter()
        .chain(bids.iter())
        .map(|level| level.base_lots)
        .max()
        .unwrap_or(1)
        .max(1);

    let width = inner.width as usize;
    let mut lines = Vec::with_capacity(rows + 2);

    let (price_col, size_col, _) = columns(width);
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<price_col$}", "price"),
            Style::default().fg(theme::FAINT),
        ),
        Span::styled(
            format!("{:>size_col$}", "size"),
            Style::default().fg(theme::FAINT),
        ),
    ]));

    // Reversed so the best ask sits against the spread. The feed sends both sides best
    // first; only the display order differs.
    for level in asks.iter().rev() {
        lines.push(row(app, level, widest, width, theme::ASK));
    }

    lines.push(spread(app, width));

    for level in &bids {
        lines.push(row(app, level, widest, width, theme::BID));
    }

    if app.feed.bids.is_empty() && app.feed.asks.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.feed.ready {
                "no resting liquidity"
            } else {
                "waiting for a snapshot…"
            },
            Style::default().fg(theme::FAINT),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn row(app: &App, level: &Level, widest: u64, width: usize, colour: ratatui::style::Color) -> Line<'static> {
    let price = app.price(Some(level.price_in_ticks));
    let size = app.size(level.base_lots);

    // Three columns that add up to the width. The bar used to be appended after two that
    // already filled it, which clipped it off the edge on every row — invisible in a
    // screenshot, and exactly the kind of thing only rendering catches.
    let (price_col, size_col, bar_col) = columns(width);
    let cells = (level.base_lots as u128 * bar_col as u128) / widest.max(1) as u128;

    Line::from(vec![
        Span::styled(
            format!("{price:<price_col$}"),
            Style::default().fg(colour),
        ),
        Span::styled(
            format!("{size:>size_col$}"),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(
            format!(" {}", "▚".repeat(cells as usize)),
            Style::default().fg(colour).add_modifier(Modifier::DIM),
        ),
    ])
}

/// Price, size and depth, as three columns that fit.
///
/// Saturating, because a terminal narrow enough to leave nothing for the bar is a real
/// terminal and the ladder should still render its numbers.
fn columns(width: usize) -> (usize, usize, usize) {
    let price = (width * 2 / 5).max(1);
    let size = (width * 2 / 5).max(1);
    let bar = width.saturating_sub(price + size + 1);
    (price, size, bar)
}

/// The midpoint and the spread, between the two sides.
///
/// Both are absent on a one-sided book rather than computed from the side that exists,
/// which would put a number on screen that nobody quoted.
fn spread(app: &App, width: usize) -> Line<'static> {
    let mid = app.price(app.feed.mid());
    let gap = match app.feed.spread() {
        Some(ticks) => format!("{} spread", app.price(Some(ticks))),
        None => "no spread".into(),
    };
    let (left, _, _) = columns(width);
    Line::from(vec![
        Span::styled(
            format!("{mid:<left$}"),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(gap, Style::default().fg(theme::DIM)),
    ])
    .alignment(Alignment::Left)
}
