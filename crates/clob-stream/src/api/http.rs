//! Snapshot endpoints.
//!
//! Handlers only — the shapes they return live in [`view`](crate::api::view), and the
//! state they read from lives in [`Registry`]. Nothing here decides what a trade is.
//!
//! Every handler clones a view out and releases the lock before serialising. One that
//! held it across an await would stall ingest for every market at once, not just its own.

use actix_web::{HttpResponse, Responder, get, web};
use clob_book::Side;
use solana_pubkey::Pubkey;

use crate::api::view::{Book, Health, Trade, levels_of};
use crate::registry::Registry;

/// How deep a book request may go.
///
/// Bounded so one request cannot ask for an unbounded serialisation.
const MAX_DEPTH: usize = 100;

/// How many trades one request may take.
const MAX_LIMIT: usize = 500;

/// Levels per side when the caller does not say.
const DEFAULT_DEPTH: usize = 20;

/// Trades returned when the caller does not say.
const DEFAULT_LIMIT: usize = 100;

#[derive(serde::Deserialize)]
pub struct DepthQuery {
    depth: Option<usize>,
}

#[derive(serde::Deserialize)]
pub struct LimitQuery {
    limit: Option<usize>,
}

/// Every market being tracked.
#[get("/v1/markets")]
pub async fn markets(registry: web::Data<Registry>) -> impl Responder {
    let markets: Vec<String> = registry.markets().iter().map(Pubkey::to_string).collect();
    HttpResponse::Ok().json(markets)
}

/// One market's book.
#[get("/v1/markets/{market}/book")]
pub async fn book(
    registry: web::Data<Registry>,
    path: web::Path<String>,
    query: web::Query<DepthQuery>,
) -> impl Responder {
    let Ok(market) = path.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let Some(view) = registry.market(&market) else {
        return HttpResponse::NotFound().body("market not tracked");
    };

    let depth = query.depth.unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    HttpResponse::Ok().json(Book {
        market: market.to_string(),
        slot: view.slot,
        bids: levels_of(&view.state, Side::Bid, depth),
        asks: levels_of(&view.state, Side::Ask, depth),
        taker_fee_bps: view.state.fees().taker_fee_bps,
        finalized_through: view.finalized_through,
    })
}

/// One market's recent trades, most recent last.
#[get("/v1/markets/{market}/trades")]
pub async fn trades(
    registry: web::Data<Registry>,
    path: web::Path<String>,
    query: web::Query<LimitQuery>,
) -> impl Responder {
    let Ok(market) = path.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let Some(view) = registry.market(&market) else {
        return HttpResponse::NotFound().body("market not tracked");
    };

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let start = view.tape.len().saturating_sub(limit);
    let trades: Vec<Trade> = view.tape[start..]
        .iter()
        .map(|trade| Trade::new(trade, view.finalized_through))
        .collect();
    HttpResponse::Ok().json(trades)
}

/// Liveness, and whether the derivation still agrees with the chain.
#[get("/health")]
pub async fn health(registry: web::Data<Registry>) -> impl Responder {
    let views: Vec<_> = registry
        .markets()
        .iter()
        .filter_map(|market| registry.market(market))
        .collect();

    let health = Health {
        markets: views.len(),
        trades_seen: views.iter().map(|v| v.trades_seen).sum(),
        trades_retracted: views.iter().map(|v| v.trades_retracted).sum(),
        reconciliation_failures: views.iter().map(|v| v.reconciliation_failures).sum(),
    };

    match health.reconciliation_failures {
        0 => HttpResponse::Ok().json(health),
        // Still serving, but not claiming to be healthy: a monitor should see this.
        _ => HttpResponse::InternalServerError().json(health),
    }
}
