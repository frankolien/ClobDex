//! `clob-tui` — a terminal view of a ClobDex market.
//!
//! Read-only. No key is loaded and nothing is signed, so this can be left on a screen,
//! pointed at somebody else's wallet, or screenshotted without any of those being a
//! decision about custody.
//!
//! ```text
//! clob-tui --market <address> --indexer http://localhost:8080 --trader <wallet>
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use clob_tui::app::{App, Link};
use clob_tui::feed::Action;
use clob_tui::indexer::{self, Endpoint};
use clob_tui::terminal::Screen;
use clob_tui::ui;
use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;

/// How often the screen is repainted when nothing has arrived.
///
/// Redrawing on every message would repaint several times a slot on a busy market for
/// changes nobody can perceive; redrawing only on messages would leave a quiet market's
/// clock frozen. This is the floor, and any event redraws immediately.
const TICK: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(about = "Watch a ClobDex market from a terminal", version)]
struct Args {
    /// The market account to watch.
    #[arg(long)]
    market: String,

    /// Base URL of a `clob-stream` instance.
    #[arg(long, default_value = "http://localhost:8080")]
    indexer: String,

    /// A wallet whose balances and resting orders to show. Read-only — no key is loaded.
    #[arg(long)]
    trader: Option<String>,

    /// Decimals of the base mint. Not on the wire, so it is given rather than guessed.
    #[arg(long, default_value_t = 9)]
    base_decimals: u32,

    /// Decimals of the quote mint.
    #[arg(long, default_value_t = 6)]
    quote_decimals: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let endpoint = Endpoint::new(&args.indexer, &args.market);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("cannot build an HTTP client")?;

    // Asked for once before the terminal is taken over, so a wrong market or an unreachable
    // indexer is an ordinary error message rather than an empty screen that has to be quit
    // out of to read.
    let summary = indexer::summary(&client, &endpoint)
        .await
        .context("could not read the market — is clob-stream running and tracking it?")?;

    let mut app = App::new(
        args.market.clone(),
        endpoint.base.clone(),
        args.trader.clone(),
        args.base_decimals,
        args.quote_decimals,
    );
    app.handle(indexer::Event::Summary(Box::new(summary)));

    let (sender, mut events) = indexer::channel();
    let socket = tokio::spawn({
        let endpoint = endpoint.clone();
        let sender = sender.clone();
        async move { indexer::run(endpoint, sender).await }
    });
    indexer::poll(client, endpoint, args.trader, sender);

    let mut screen = Screen::open()?;
    let mut keys = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);

    while !app.quit {
        screen.terminal().draw(|frame| ui::draw(frame, &app))?;

        tokio::select! {
            // Biased, so a full event queue cannot starve the keyboard: a viewer that
            // cannot be quit during a burst of updates is a viewer that has to be killed
            // from another terminal.
            biased;

            key = keys.next() => match key {
                Some(Ok(TermEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
                        KeyCode::Char('c') if ctrl => app.quit = true,
                        _ => {}
                    }
                }
                Some(Err(error)) => return Err(error).context("the terminal stopped reporting input"),
                None => app.quit = true,
                _ => {}
            },

            event = events.recv() => match event {
                Some(event) => {
                    // The socket closes itself on a lag and its own loop dials again; this
                    // only reflects that on screen. Aborting the task here would kill the
                    // reconnect loop and leave a viewer that never comes back.
                    if app.handle(event) == Action::Resubscribe {
                        app.link = Link::Connecting;
                    }
                }
                None => break,
            },

            _ = ticker.tick() => {}
        }
    }

    socket.abort();
    Ok(())
}
