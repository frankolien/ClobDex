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

use crate::api::view::{
    Book, Candle, Health, HistoricalTrade, MarketSummary, Trade, TraderView, Window, levels_of,
};
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
pub async fn markets(
    registry: web::Data<Registry>,
    program_id: web::Data<Pubkey>,
) -> impl Responder {
    let program_id = program_id.into_inner();
    HttpResponse::Ok().json(registry.map_markets(|market, view| {
        MarketSummary::new(&program_id, market, view)
    }))
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

/// Slots a window covers when the caller does not say.
///
/// About twenty-four hours, at four hundred milliseconds a slot. This is the only
/// wall-clock assumption in the crate, and it is a default rather than a conversion:
/// nothing stored or returned is in seconds, so being wrong about slot times moves which
/// trades a caller asked for and never what any of them says. Everything else stays
/// slot-native for the reason [`candle`](crate::candle) gives — block times drift, and a
/// boundary that moves is worse than one measured in an odd unit.
const DEFAULT_WINDOW_SLOTS: u64 = 216_000;

/// Trades one window may aggregate.
///
/// Larger than a history page because a window is a total rather than a listing, and a
/// total assembled from a tenth of the trades is wrong rather than short. A span busy
/// enough to exceed this says so — see [`Window::truncated`](crate::api::view::Window).
const MAX_WINDOW_TRADES: usize = 100_000;

#[derive(serde::Deserialize)]
pub struct HistoryQuery {
    from_slot: Option<u64>,
    to_slot: Option<u64>,
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub struct WindowQuery {
    slots: Option<u64>,
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

/// One trader's balances and resting orders in one market.
///
/// Served here rather than left to a client because reaching it means walking the market
/// account's seat table and both order trees. Those are red-black trees in a `Pod` arena;
/// a second implementation of that walk in another language is a second thing that can be
/// wrong about who owns what.
///
/// 404 when the wallet holds no seat, which is a different answer from a row of zeroes: a
/// wallet that never traded here has no position, and one that withdrew everything has an
/// empty one.
#[get("/v1/markets/{market}/traders/{trader}")]
pub async fn trader(
    registry: web::Data<Registry>,
    path: web::Path<(String, String)>,
) -> impl Responder {
    let (market, trader) = path.into_inner();
    let Ok(market) = market.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let Ok(trader) = trader.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let Some(view) = registry.market(&market) else {
        return HttpResponse::NotFound().body("market not tracked");
    };

    match TraderView::new(&market, &trader, &view) {
        Some(position) => HttpResponse::Ok().json(position),
        None => HttpResponse::NotFound().body("no seat in this market"),
    }
}

/// What one market traded over the last `slots` slots.
///
/// The rolling statistic a markets table and a landing page want — volume, range, and
/// change — kept off `/v1/markets` because it costs a query per market and that endpoint
/// costs none.
///
/// The span ends at the last slot this process has processed for the market, not at the
/// chain tip: those differ while the indexer is catching up, and measuring to a tip whose
/// trades have not been read yet would report a lull that is really a backlog.
#[get("/v1/markets/{market}/window")]
pub async fn window(
    registry: web::Data<Registry>,
    store: web::Data<Arc<dyn Store>>,
    path: web::Path<String>,
    query: web::Query<WindowQuery>,
) -> impl Responder {
    let Ok(market) = path.parse::<Pubkey>() else {
        return HttpResponse::BadRequest().body("not a pubkey");
    };
    let Some(view) = registry.market(&market) else {
        return HttpResponse::NotFound().body("market not tracked");
    };

    // At least one slot: a zero-slot window is a span containing nothing, and answering
    // it with the whole history would be the opposite of what was asked.
    let slots = query.slots.unwrap_or(DEFAULT_WINDOW_SLOTS).max(1);
    let to_slot = view.slot;
    let from_slot = to_slot.saturating_sub(slots - 1);

    let range = Range {
        from_slot,
        to_slot,
        limit: MAX_WINDOW_TRADES,
    };
    match store.trades(&market, range).await {
        Ok(found) => {
            let truncated = found.len() >= MAX_WINDOW_TRADES;
            HttpResponse::Ok().json(Window::new(&market, from_slot, to_slot, &found, truncated))
        }
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
