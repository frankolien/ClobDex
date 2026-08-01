//! Snapshot endpoints.
//!
//! Everything here is a clone-out-and-release read: the registry lock is never held
//! across serialisation, let alone across an await, because a handler that did would
//! stall ingest for every market at once.

use actix_web::{HttpResponse, Responder, get, web};
use clob_book::Side;
use serde::Serialize;
use solana_pubkey::Pubkey;

use crate::registry::Registry;

/// One aggregated price level, as served.
#[derive(Serialize)]
pub struct Level {
    /// Price, in ticks.
    pub price_in_ticks: u64,
    /// Size resting there, in base lots.
    pub base_lots: u64,
}

/// A market's book at a slot.
#[derive(Serialize)]
pub struct Book {
    /// The market.
    pub market: String,
    /// Slot this state came from.
    pub slot: u64,
    /// Bids, best first.
    pub bids: Vec<Level>,
    /// Asks, best first.
    pub asks: Vec<Level>,
    /// Taker fee in basis points.
    pub taker_fee_bps: u64,
    /// Everything at or below this slot is rooted. A book above it can still change if
    /// the slot it came from is abandoned.
    pub finalized_through: u64,
}

/// One trade, as served.
#[derive(Serialize)]
pub struct Trade {
    /// Slot it landed in.
    pub slot: u64,
    /// Execution price — always the maker's.
    pub price_in_ticks: u64,
    /// Size, in base lots.
    pub base_lots: u64,
    /// Gross quote value, before fee.
    pub quote_lots: u64,
    /// Side the taker was on.
    pub taker_side: &'static str,
    /// Seat that owned the resting order.
    pub maker_seat: u32,
    /// Whether the slot this came from is rooted.
    ///
    /// Filled in by the caller, which knows how far finality has advanced; a trade on
    /// its own does not. A consumer that cannot tolerate a retraction should wait.
    pub finalized: bool,
}

impl Trade {
    /// Renders a trade, marking it final if its slot is rooted.
    pub(crate) fn new(trade: &clob_indexer::Trade, finalized_through: u64) -> Self {
        Self {
            slot: trade.slot,
            price_in_ticks: trade.price_in_ticks.as_u64(),
            base_lots: trade.base_lots.as_u64(),
            quote_lots: trade.quote_lots.as_u64(),
            taker_side: side_name(trade.taker_side),
            maker_seat: trade.maker_seat,
            finalized: trade.slot <= finalized_through,
        }
    }
}

pub(crate) fn side_name(side: Side) -> &'static str {
    match side {
        Side::Bid => "bid",
        Side::Ask => "ask",
    }
}

/// How deep a book request may go.
///
/// Bounded so one request cannot ask for an unbounded serialisation.
const MAX_DEPTH: usize = 100;

/// How many trades one request may take.
const MAX_LIMIT: usize = 500;

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

    let depth = query.depth.unwrap_or(20).min(MAX_DEPTH);
    let levels = |side| {
        view.state
            .level_two(side, depth)
            .iter()
            .map(|level| Level {
                price_in_ticks: level.price_in_ticks.as_u64(),
                base_lots: level.base_lots.as_u64(),
            })
            .collect()
    };

    HttpResponse::Ok().json(Book {
        market: market.to_string(),
        slot: view.slot,
        bids: levels(Side::Bid),
        asks: levels(Side::Ask),
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

    let limit = query.limit.unwrap_or(100).min(MAX_LIMIT);
    let start = view.tape.len().saturating_sub(limit);
    let trades: Vec<Trade> = view.tape[start..]
        .iter()
        .map(|trade| Trade::new(trade, view.finalized_through))
        .collect();
    HttpResponse::Ok().json(trades)
}

/// Liveness, and whether the derivation still agrees with the chain.
#[derive(Serialize)]
struct Health {
    markets: usize,
    trades_seen: u64,
    /// Trades withdrawn because the slot that produced them was abandoned. Expected to
    /// be small and non-zero on a cluster indexed at confirmed; zero forever suggests
    /// rollbacks are not being seen at all.
    trades_retracted: u64,
    /// Deltas whose derived fees disagreed with the market's own counter. Non-zero means
    /// the derivation and the program disagree, which is worth alerting on rather than
    /// serving quietly.
    reconciliation_failures: u64,
}

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

/// Shared by the HTTP and WebSocket sides so both report a trade identically.
pub(crate) fn trades_of(delta: &clob_indexer::BookDelta, finalized_through: u64) -> Vec<Trade> {
    delta
        .trades
        .iter()
        .map(|trade| Trade::new(trade, finalized_through))
        .collect()
}
