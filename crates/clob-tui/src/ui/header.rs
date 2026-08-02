//! The market, and whether what is on screen can be believed.
//!
//! The status line carries three things a viewer has no other way to learn: whether the
//! socket is up, how far behind finality the book is, and whether this subscriber has ever
//! been told it missed messages. The last two are the difference between a quiet market and
//! a broken client, which look identical otherwise.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::theme;
use crate::app::{App, Link};
use crate::lots;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut top = vec![Span::styled(
        short(&app.market),
        Style::default()
            .fg(theme::TEXT)
            .add_modifier(Modifier::BOLD),
    )];

    if let Some(summary) = &app.summary {
        top.push(Span::styled("   ", Style::default()));
        top.push(field("bid", &app.price(read(&summary.best_bid_in_ticks)), theme::BID));
        top.push(field("ask", &app.price(read(&summary.best_ask_in_ticks)), theme::ASK));
        top.push(field("last", &app.price(read(&summary.last_price_in_ticks)), theme::TEXT));
        top.push(field(
            "spread",
            &app.price(read(&summary.spread_in_ticks)),
            theme::DIM,
        ));
        top.push(field("fee", &format!("{} bps", summary.taker_fee_bps), theme::DIM));
    }

    let depth = Line::from(vec![
        Span::styled(
            format!(
                "{} bids · {} asks",
                app.feed.bids.len(),
                app.feed.asks.len()
            ),
            Style::default().fg(theme::DIM),
        ),
        Span::styled(
            match &app.summary {
                Some(summary) => format!("   {} seats · {} fills seen", summary.seats, summary.trades_seen),
                None => String::new(),
            },
            Style::default().fg(theme::FAINT),
        ),
    ]);

    frame.render_widget(Paragraph::new(vec![Line::from(top), depth]), inner);
}

/// The bottom line: connection, finality lag, and anything that has gone wrong.
pub fn status(frame: &mut Frame, area: Rect, app: &App) {
    let (label, colour) = match &app.link {
        Link::Live => ("live".to_string(), theme::BID),
        Link::Connecting => ("connecting…".to_string(), theme::WARN),
        Link::Down(why) => (format!("down — {why}"), theme::ASK),
    };

    let mut spans = vec![
        Span::styled("● ", Style::default().fg(colour)),
        Span::styled(label, Style::default().fg(colour)),
        Span::styled(format!("   {}", app.indexer), Style::default().fg(theme::FAINT)),
    ];

    // How far the book is ahead of what can no longer be rolled back. A viewer that never
    // shows this cannot tell a market that is quiet from one whose feed has stopped.
    if app.feed.slot > 0 {
        let behind = app.feed.slot.saturating_sub(app.feed.finalized_through);
        spans.push(Span::styled(
            format!("   slot {} · {behind} from final", app.feed.slot),
            Style::default().fg(theme::DIM),
        ));
    }

    // Only once either has happened. A permanent "0 retracted" trains the eye to skip the
    // place where the number that matters will appear.
    if app.feed.retracted > 0 {
        spans.push(Span::styled(
            format!("   {} retracted", app.feed.retracted),
            Style::default().fg(theme::WARN),
        ));
    }
    if app.feed.missed > 0 {
        spans.push(Span::styled(
            format!("   {} missed", app.feed.missed),
            Style::default().fg(theme::ASK),
        ));
    }

    spans.push(Span::styled(
        "   q to quit",
        Style::default().fg(theme::FAINT),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn field(label: &str, value: &str, colour: ratatui::style::Color) -> Span<'static> {
    Span::styled(
        format!("{label} {value}    "),
        Style::default().fg(colour),
    )
}

fn read(value: &Option<String>) -> Option<u64> {
    lots::maybe(value, "price").ok().flatten()
}

fn short(address: &str) -> String {
    if address.len() <= 16 {
        return address.to_string();
    }
    format!("{}…{}", &address[..8], &address[address.len() - 6..])
}
