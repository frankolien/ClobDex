//! The live feed.
//!
//! Socket lifecycle only — what goes down the wire is defined in
//! [`view`](crate::api::view), the same place the HTTP side reads it from, so neither
//! transport owns the other's types.
//!
//! A subscriber gets the current book once, then every subsequent change to that market.
//! Sending the snapshot first means a client never has to make a separate HTTP call and
//! then reconcile a race against the first delta.

use actix_web::{HttpRequest, HttpResponse, web};
use actix_ws::AggregatedMessage;
use clob_book::Side;
use futures::StreamExt;
use solana_pubkey::Pubkey;
use tokio::sync::broadcast::error::RecvError;

use crate::api::view::{Message, levels_of, trades_of};
use crate::registry::{Event, Registry};

/// How deep the book goes on this feed, in the snapshot and in every update.
///
/// One number for both: a subscriber that received fifty levels and then started
/// receiving fifteen would watch its ladder shrink for no reason it could see.
const FEED_DEPTH: usize = 50;

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
            let snapshot = Message::Snapshot {
                market: market.to_string(),
                slot: view.slot,
                finalized_through: view.finalized_through,
                bids: levels_of(&view.state, Side::Bid, FEED_DEPTH),
                asks: levels_of(&view.state, Side::Ask, FEED_DEPTH),
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
                        let finalized_through = registry
                            .market(&market)
                            .map(|view| view.finalized_through)
                            .unwrap_or(0);
                        // Levels come from the state this transaction produced rather than
                        // from the registry, which may already have moved on: a subscriber
                        // stitching an older book onto a newer slot number would show a
                        // book that never existed.
                        let message = Message::Update {
                            slot: derived.slot,
                            trades: trades_of(&derived.delta, finalized_through),
                            bids: levels_of(&derived.state, Side::Bid, FEED_DEPTH),
                            asks: levels_of(&derived.state, Side::Ask, FEED_DEPTH),
                            best_bid: derived.state.best_bid().map(|o| o.price_in_ticks().as_u64()),
                            best_ask: derived.state.best_ask().map(|o| o.price_in_ticks().as_u64()),
                            finalized_through,
                        };
                        if send(&mut session, &message).await.is_err() { break }
                    }
                    Ok(Event::Retracted { market: which, slot, trades }) => {
                        if which != market {
                            continue;
                        }
                        if send(&mut session, &Message::Retract { slot, trades }).await.is_err() { break }
                    }
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
