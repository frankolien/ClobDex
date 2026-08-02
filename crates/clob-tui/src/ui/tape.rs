//! The trade tape.
//!
//! Two things here that most tapes get wrong and this feed makes possible to get right.
//!
//! A fill that is not yet rooted is marked. The indexer runs at `confirmed`, which sees a
//! trade about a second before finality does and accepts that its slot can still be
//! abandoned — so provisional prints are shown as provisional rather than as fact.
//!
//! And finality is re-derived from the current watermark rather than read off each row, so
//! a print that has since rooted stops being marked.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::theme;
use crate::app::App;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE))
        .title(Span::styled(" fills ", Style::default().fg(theme::DIM)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let rows = inner.height as usize;
    let mut lines = Vec::with_capacity(rows);

    lines.push(Line::from(Span::styled(
        format!("{:<w$}{:>r$}", "price", "size", w = width / 2, r = width - width / 2),
        Style::default().fg(theme::FAINT),
    )));

    for fill in app.feed.tape.iter().take(rows.saturating_sub(1)) {
        let rooted = app.feed.is_final(fill);
        let colour = if fill.taker_is_bid { theme::BID } else { theme::ASK };

        let price = app.price(Some(fill.price_in_ticks));
        let size = app.size(fill.base_lots);
        let left = width / 2;
        let right = width - left;

        // Dimmed rather than hidden. It probably happened, and pretending otherwise is its
        // own kind of wrong — the marking says "not yet certain", not "not real".
        let style = |base: Style| {
            if rooted {
                base
            } else {
                base.add_modifier(Modifier::DIM)
            }
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{price:<left$}"), style(Style::default().fg(colour))),
            Span::styled(
                format!("{size:>right$}"),
                style(Style::default().fg(theme::TEXT)),
            ),
        ]));
    }

    if app.feed.tape.is_empty() {
        lines.push(Line::from(Span::styled(
            "nothing has traded",
            Style::default().fg(theme::FAINT),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
