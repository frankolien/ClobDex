//! The watched wallet's balances and resting orders.
//!
//! Free and locked are shown apart because they answer different questions: a wallet that
//! deposited and then quoted still owns all of it, but only the free part can be withdrawn
//! or committed to something new. One combined number matches neither the vault nor the
//! wallet's own arithmetic.
//!
//! No seat is a different state from an empty one, and the indexer distinguishes them with
//! a 404 rather than a row of zeroes, so this does too.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::theme;
use crate::app::App;
use crate::lots;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let title = match &app.position {
        Some(view) => format!(" position · seat {} ", view.seat),
        None => " position ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE))
        .title(Span::styled(title, Style::default().fg(theme::DIM)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(trader) = &app.trader else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "pass --trader <wallet> to watch one",
                Style::default().fg(theme::FAINT),
            ))),
            inner,
        );
        return;
    };

    let Some(view) = &app.position else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    short(trader),
                    Style::default().fg(theme::DIM),
                )),
                Line::from(Span::styled(
                    "holds no seat in this market",
                    Style::default().fg(theme::FAINT),
                )),
            ]),
            inner,
        );
        return;
    };

    let width = inner.width as usize;
    let mut lines = Vec::new();

    let read = |text: &str, field: &str| lots::quantity(text, field).unwrap_or(0);

    lines.push(balance(
        "base free",
        &app.size(read(&view.base_lots_free, "base_lots_free")),
        width,
        false,
    ));
    lines.push(balance(
        "locked",
        &app.size(read(&view.base_lots_locked, "base_lots_locked")),
        width,
        true,
    ));
    lines.push(balance(
        "quote free",
        &app.quote(read(&view.quote_lots_free, "quote_lots_free")),
        width,
        false,
    ));
    lines.push(balance(
        "locked",
        &app.quote(read(&view.quote_lots_locked, "quote_lots_locked")),
        width,
        true,
    ));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("resting  {}", view.orders.len()),
        Style::default().fg(theme::FAINT),
    )));

    for order in &view.orders {
        let colour = if order.side == "bid" { theme::BID } else { theme::ASK };
        let price = app.price(lots::quantity(&order.price_in_ticks, "price").ok());
        let size = app.size(read(&order.base_lots, "base_lots"));
        let left = width / 2;
        lines.push(Line::from(vec![
            Span::styled(format!("{price:<left$}"), Style::default().fg(colour)),
            Span::styled(
                format!("{size:>w$}", w = width - left),
                Style::default().fg(theme::TEXT),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn balance(label: &str, value: &str, width: usize, muted: bool) -> Line<'static> {
    let left = width / 2;
    let style = if muted {
        Style::default().fg(theme::DIM)
    } else {
        Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(
            format!("{label:<left$}"),
            Style::default().fg(theme::FAINT),
        ),
        Span::styled(format!("{value:>w$}", w = width - left), style),
    ])
}

/// A wallet address, shortened. The middle of a base58 key carries no information anyone
/// reads, and the ends are what gets compared.
fn short(address: &str) -> String {
    if address.len() <= 12 {
        return address.to_string();
    }
    format!("{}…{}", &address[..6], &address[address.len() - 4..])
}
