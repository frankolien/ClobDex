//! The live feed.
//!
//! A subscriber gets the current book once, then every subsequent change to that market.
//! Sending the snapshot first means a client never has to make a separate HTTP call and
//! then reconcile a race against the first delta.

use actix_web::{HttpRequest, HttpResponse, web};
use actix_ws::AggregatedMessage;
use futures::StreamExt;
use serde::Serialize;
use solana_pubkey::Pubkey;
use tokio::sync::broadcast::error::RecvError;

use crate::api::http;
use crate::registry::{Event, Registry};

/// What the socket sends.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    /// The book as it stands, sent once on connect.
    Snapshot {
        market: String,
        slot: u64,
        bids: Vec<http::Level>,
        asks: Vec<http::Level>,
    },
    /// One transaction's effect.
    Update {
        slot: u64,
        trades: Vec<http::Trade>,
        /// Best bid and ask after it, when either side has liquidity.
        best_bid: Option<u64>,
        best_ask: Option<u64>,
    },
    /// The subscriber fell too far behind and lost `missed` messages.
    ///
    /// Sent rather than silently skipping, because a gap a client does not know about is
    /// worse than one it does: it can re-request a snapshot.
    Lagged { missed: u64 },
}

/// How deep the initial snapshot goes.
const SNAPSHOT_DEPTH: usize = 50;

/// Opens a feed for one market.
pub async fn stream(
    request: HttpRequest,
    body: web::Payload,
    registry: web::Data<Registry>,
    path: web::Path<String>,
) -> Result<HttpResponse, actix_web::Error> {
    let Ok(market) = path.parse::<Pubkey>() else {
        return Ok(HttpResponse::BadRequest().body("not a pubkey"));
    };
    if registry.market(&market).is_none() {
        return Ok(HttpResponse::NotFound().body("market not tracked"));
    }

    let (response, mut session, message_stream) = actix_ws::handle(&request, body)?;
    let registry = registry.into_inner();
    let mut updates = registry.subscribe();

    actix_web::rt::spawn(async move {
        if let Some(view) = registry.market(&market) {
            let levels = |side| {
                view.state
                    .level_two(side, SNAPSHOT_DEPTH)
                    .iter()
                    .map(|level| http::Level {
                        price_in_ticks: level.price_in_ticks.as_u64(),
                        base_lots: level.base_lots.as_u64(),
                    })
                    .collect()
            };
            let snapshot = Message::Snapshot {
                market: market.to_string(),
                slot: view.slot,
                bids: levels(clob_book::Side::Bid),
                asks: levels(clob_book::Side::Ask),
            };
            if send(&mut session, &snapshot).await.is_err() {
                return;
            }
        }

        let mut client = message_stream.aggregate_continuations();
        loop {
            tokio::select! {
                // Pings are answered so an idle market's connection is not reaped by an
                // intermediary that sees no traffic.
                incoming = client.next() => match incoming {
                    Some(Ok(AggregatedMessage::Ping(bytes))) if session.pong(&bytes).await.is_err() => break,
                    Some(Ok(AggregatedMessage::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                },
                update = updates.recv() => match update {
                    Ok(Event::Change(derived)) => {
                        if derived.market != market {
                            continue;
                        }
                        // A transaction that only moved funds changes nothing a feed
                        // subscriber can see; sending an empty frame is noise.
                        if derived.delta.is_empty() {
                            continue;
                        }
                        let message = Message::Update {
                            slot: derived.slot,
                            trades: http::trades_of(&derived.delta),
                            best_bid: derived.state.best_bid().map(|o| o.price_in_ticks().as_u64()),
                            best_ask: derived.state.best_ask().map(|o| o.price_in_ticks().as_u64()),
                        };
                        if send(&mut session, &message).await.is_err() { break }
                    }
                    // Surfacing a retraction to the client comes with the rest of the
                    // finality surface.
                    Ok(Event::Retracted { .. }) => {}
                    Err(RecvError::Lagged(missed)) => {
                        if send(&mut session, &Message::Lagged { missed }).await.is_err() { break }
                    }
                    Err(RecvError::Closed) => break,
                },
            }
        }

        let _ = session.close(None).await;
    });

    Ok(response)
}

async fn send(session: &mut actix_ws::Session, message: &Message) -> Result<(), ()> {
    let body = serde_json::to_string(message).map_err(|_| ())?;
    session.text(body).await.map_err(|_| ())
}
