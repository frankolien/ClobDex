//! Talking to `clob-stream`.
//!
//! The socket runs in its own task and pushes parsed messages down a channel, so a slow
//! render never applies back-pressure to the feed and a stalled feed never freezes the
//! keyboard. The rest of the program sees a stream of [`Event`]s and does not know whether
//! one arrived over a socket or from a key press.
//!
//! Reconnection is unconditional and delayed. An indexer that is restarting should cost a
//! second of "reconnecting" on screen rather than an exit — the whole point of a viewer is
//! that it is still there when you look back at it.

use std::time::Duration;

use anyhow::{Context, Result};
pub use reqwest::Client;
use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::wire;

/// How long to wait before dialling again after the socket ends.
const RECONNECT: Duration = Duration::from_secs(2);

/// How often the summary and the position are refetched.
///
/// These change slowly and are not on the socket, so a slow poll is right. The book and the
/// tape arrive as they happen.
const POLL: Duration = Duration::from_secs(3);

/// Anything the interface reacts to.
#[derive(Clone, Debug)]
pub enum Event {
    /// A message from the live feed.
    Feed(wire::Message),
    /// The socket came up.
    Connected,
    /// The socket went away, with why if it said.
    Disconnected(String),
    /// A repoll of the market summary.
    Summary(Box<wire::MarketSummary>),
    /// A repoll of the watched trader's position. `None` means no seat in this market.
    Position(Option<Box<wire::TraderView>>),
}

/// Where the indexer is, and which market is being watched.
#[derive(Clone)]
pub struct Endpoint {
    /// Base URL, without a trailing slash.
    pub base: String,
    /// The market account.
    pub market: String,
}

impl Endpoint {
    pub fn new(base: &str, market: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            market: market.to_string(),
        }
    }

    /// The WebSocket URL for this market's feed.
    ///
    /// Derived from the same base as the HTTP calls, so there is no way to point the feed
    /// at a different instance from the one answering the snapshots — which would show a
    /// book and a tape from two different processes' idea of the same market.
    fn socket(&self) -> String {
        let base = self
            .base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{base}/v1/markets/{}/stream", self.market)
    }

    fn http(&self, path: &str) -> String {
        format!("{}/v1/markets/{}{path}", self.base, self.market)
    }
}

/// Fetches this market's summary once.
pub async fn summary(client: &Client, endpoint: &Endpoint) -> Result<wire::MarketSummary> {
    let url = format!("{}/v1/markets", endpoint.base);
    let markets: Vec<wire::MarketSummary> = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("cannot reach {url}"))?
        .error_for_status()?
        .json()
        .await
        .context("the markets list is not what this expects")?;

    markets
        .into_iter()
        .find(|summary| summary.market == endpoint.market)
        .with_context(|| format!("the indexer is not tracking {}", endpoint.market))
}

/// Fetches one trader's position, or `None` when they hold no seat.
///
/// A 404 is a real answer here rather than an error: no seat is a different state from an
/// empty one, and the indexer distinguishes them deliberately.
pub async fn position(
    client: &Client,
    endpoint: &Endpoint,
    trader: &str,
) -> Result<Option<wire::TraderView>> {
    let url = endpoint.http(&format!("/traders/{trader}"));
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("cannot reach {url}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response
        .error_for_status()?
        .json()
        .await
        .map(Some)
        .context("a position is not what this expects")
}

/// Runs the socket until the program ends, pushing everything it sees down `events`.
pub async fn run(endpoint: Endpoint, events: UnboundedSender<Event>) {
    loop {
        match connect(&endpoint, &events).await {
            Ok(()) => {
                let _ = events.send(Event::Disconnected("the feed closed".into()));
            }
            Err(error) => {
                let _ = events.send(Event::Disconnected(format!("{error:#}")));
            }
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

async fn connect(endpoint: &Endpoint, events: &UnboundedSender<Event>) -> Result<()> {
    let (stream, _) = tokio_tungstenite::connect_async(endpoint.socket())
        .await
        .with_context(|| format!("cannot reach {}", endpoint.socket()))?;
    let _ = events.send(Event::Connected);

    let (_, mut incoming) = stream.split();
    while let Some(frame) = incoming.next().await {
        let frame = frame.context("the feed errored")?;
        let Frame::Text(text) = frame else { continue };
        // One unreadable frame costs a frame, not the session. A server that changed its
        // wire format will produce a stream of these, which the status line makes visible.
        match serde_json::from_str::<wire::Message>(&text) {
            Ok(message) => {
                // A lag means the book is wrong by an unknown amount and no later update
                // carries what was dropped. Only a fresh snapshot fixes it, so the socket
                // is dropped here and `run` dials again — the decision lives with the
                // thing that can act on it rather than with the state it corrupted.
                let lagged = matches!(message, wire::Message::Lagged { .. });
                if events.send(Event::Feed(message)).is_err() {
                    return Ok(());
                }
                if lagged {
                    return Ok(());
                }
            }
            Err(error) => {
                let _ = events.send(Event::Disconnected(format!("unreadable frame: {error}")));
            }
        }
    }
    Ok(())
}

/// Polls the summary and the position on an interval.
pub fn poll(client: Client, endpoint: Endpoint, trader: Option<String>, events: UnboundedSender<Event>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL);
        loop {
            ticker.tick().await;
            // A failed poll is skipped rather than reported. The socket is what says
            // whether the indexer is reachable, and two components arguing about it on one
            // status line is noise.
            if let Ok(summary) = summary(&client, &endpoint).await
                && events.send(Event::Summary(Box::new(summary))).is_err()
            {
                return;
            }
            if let Some(trader) = &trader
                && let Ok(found) = position(&client, &endpoint, trader).await
                && events.send(Event::Position(found.map(Box::new))).is_err()
            {
                return;
            }
        }
    });
}

/// A channel for events, and the handle the socket task writes to.
pub fn channel() -> (
    UnboundedSender<Event>,
    tokio::sync::mpsc::UnboundedReceiver<Event>,
) {
    unbounded_channel()
}
