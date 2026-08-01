//! Snapshot endpoints.
//!
//! Handlers only — the shapes they return live in [`view`](crate::api::view), and the
//! state they read from lives in [`Registry`]. Nothing here decides what a trade is.
//!
//! Every handler clones a view out and releases the lock before serialising. One that
//! held it across an await would stall ingest for every market at once, not just its own.

use std::sync::Arc;

use actix_web::{HttpResponse, Responder, get, web};
use clob_book::Side;
use solana_pubkey::Pubkey;

use crate::api::view::{Book, Candle, Health, HistoricalTrade, MarketSummary, Trade, levels_of};
use crate::candle;
use crate::registry::Registry;
use crate::store::{Range, Store};

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

/// Every market being tracked, summarised.
///
/// Returns enough to render a markets table or a landing page — price, spread, depth,
/// what the market holds, and the lot geometry needed to format any of it — without a
/// follow-up call per row. Everything comes from memory; rolling volume is on
/// `/v1/markets/{market}/window`, which costs a query and so is not folded in here.
#[get("/v1/markets")]
pub async fn markets(registry: web::Data<Registry>) -> impl Responder {
    HttpResponse::Ok().json(registry.map_markets(MarketSummary::new))
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

// -------------------------------------------------------------------------------------
// History
//
// Served from the store rather than the in-memory tape, so it survives a restart and
// reaches further back than the 1,024 trades the process keeps.
// -------------------------------------------------------------------------------------

/// Slots per candle when the caller does not say. Roughly a minute of slots.
const DEFAULT_INTERVAL: u64 = 150;

/// Candles a single request may return.
const MAX_CANDLES: usize = 1_000;

/// Trades one history request may scan.
const MAX_HISTORY: usize = 10_000;

#[derive(serde::Deserialize)]
pub struct HistoryQuery {
    from_slot: Option<u64>,
    to_slot: Option<u64>,
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub struct CandleQuery {
    interval: Option<u64>,
    from_slot: Option<u64>,
    to_slot: Option<u64>,
}

fn range(from: Option<u64>, to: Option<u64>, limit: usize) -> Range {
    Range {
        from_slot: from.unwrap_or(0),
        to_slot: to.unwrap_or(u64::MAX),
        limit,
    }
}

/// Stored trades for one market, oldest first.
#[get("/v1/markets/{market}/history")]
pub async fn history(
    store: web::Data<Arc<dyn Store>>,
    path: web::Path<String>,
    query: web::Query<HistoryQuery>,
) -> impl Responder {
    let Ok(market) = path.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_HISTORY);
    match store
        .trades(&market, range(query.from_slot, query.to_slot, limit))
        .await
    {
        // Named `found`, not `trades`: the #[get] macro defines a unit struct called
        // `trades`, and a unit struct in scope turns `Ok(trades)` into a pattern that
        // matches it rather than a fresh binding.
        Ok(found) => {
            let rows: Vec<HistoricalTrade> = found.iter().map(HistoricalTrade::from).collect();
            HttpResponse::Ok().json(rows)
        }
        // A store that is down is a 503, not a 200 with an empty list — the difference
        // between "nothing traded" and "we cannot tell" matters to whoever is asking.
        Err(error) => HttpResponse::ServiceUnavailable().body(format!("{error:#}")),
    }
}

/// OHLCV for one market, bucketed by slot.
#[get("/v1/markets/{market}/candles")]
pub async fn candles(
    store: web::Data<Arc<dyn Store>>,
    path: web::Path<String>,
    query: web::Query<CandleQuery>,
) -> impl Responder {
    let Ok(market) = path.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let interval = query.interval.unwrap_or(DEFAULT_INTERVAL);
    if interval == 0 {
        return HttpResponse::BadRequest().body("interval must be at least one slot");
    }

    match store
        .trades(&market, range(query.from_slot, query.to_slot, MAX_HISTORY))
        .await
    {
        Ok(found) => {
            let mut aggregated = candle::aggregate(&found, interval);
            // Bounded after aggregating, keeping the most recent: a chart wants the
            // latest window, not the first one ever recorded.
            if aggregated.len() > MAX_CANDLES {
                aggregated.drain(..aggregated.len() - MAX_CANDLES);
            }
            let rows: Vec<Candle> = aggregated.iter().map(Candle::from).collect();
            HttpResponse::Ok().json(rows)
        }
        Err(error) => HttpResponse::ServiceUnavailable().body(format!("{error:#}")),
    }
}
