//! Everything this API puts on the wire.
//!
//! One module so the whole contract can be read in one place, and so neither transport
//! owns types the other needs — the WebSocket used to import its price levels from the
//! HTTP module, which meant deleting an endpoint would have broken the socket.
//!
//! These are deliberately separate from the engine's types. `clob-book` speaks in
//! `Ticks` and `BaseLots`, which are exact and meaningful in-process; on the wire they
//! are plain integers with the unit in the field name, so a consumer cannot mistake a
//! tick for a price or a lot for a token.

use clob_book::Side;
use serde::Serialize;

/// One aggregated price level.
#[derive(Serialize)]
pub struct Level {
    /// Price, in ticks.
    pub price_in_ticks: u64,
    /// Size resting there, in base lots.
    pub base_lots: u64,
}

impl From<&clob_client::state::Level> for Level {
    fn from(level: &clob_client::state::Level) -> Self {
        Self {
            price_in_ticks: level.price_in_ticks.as_u64(),
            base_lots: level.base_lots.as_u64(),
        }
    }
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

/// One trade.
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
    /// A trade cannot know this on its own; the caller supplies how far finality has
    /// advanced. A consumer that cannot tolerate a retraction should wait for it.
    pub finalized: bool,
}

impl Trade {
    /// Renders a trade, marking it final if its slot is rooted.
    pub fn new(trade: &clob_indexer::Trade, finalized_through: u64) -> Self {
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

/// Liveness, and whether the derivation still agrees with the chain.
#[derive(Serialize)]
pub struct Health {
    /// Markets being tracked.
    pub markets: usize,
    /// Trades published since the process started.
    pub trades_seen: u64,
    /// Trades withdrawn because the slot that produced them was abandoned.
    ///
    /// Reported alongside `trades_seen` rather than subtracted from it: netting the two
    /// would make a rollback look like it never happened.
    pub trades_retracted: u64,
    /// Deltas whose derived fees disagreed with the market's own counter.
    ///
    /// Non-zero means the derivation and the program disagree about what happened, which
    /// is a bug or a wire-format change.
    pub reconciliation_failures: u64,
}

/// What the live feed sends.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// The book as it stands, sent once on connect so a client never has to make a
    /// separate call and then reconcile a race against the first delta.
    Snapshot {
        /// The market.
        market: String,
        /// Slot this state came from.
        slot: u64,
        /// Everything at or below this slot is rooted.
        finalized_through: u64,
        /// Bids, best first.
        bids: Vec<Level>,
        /// Asks, best first.
        asks: Vec<Level>,
    },
    /// One transaction's effect.
    Update {
        /// Slot it landed in.
        slot: u64,
        /// Trades it produced.
        trades: Vec<Trade>,
        /// Best bid after it, if the side has liquidity.
        best_bid: Option<u64>,
        /// Best ask after it, if the side has liquidity.
        best_ask: Option<u64>,
        /// Everything at or below this slot is rooted. Anything above it can still be
        /// retracted, which is what a consumer needs in order to decide whether to act.
        finalized_through: u64,
    },
    /// Trades already sent that did not happen: their slot was abandoned.
    ///
    /// Pushed rather than left to be noticed. A client that showed them has to be told,
    /// and silence is indistinguishable from a quiet market.
    Retract {
        /// The slot that was dropped.
        slot: u64,
        /// How many trades went with it.
        trades: usize,
    },
    /// The subscriber fell behind and lost `missed` messages.
    ///
    /// Sent rather than silently skipped: a gap a client knows about can be closed by
    /// re-requesting a snapshot, and one it does not know about cannot.
    Lagged {
        /// Messages dropped for this subscriber.
        missed: u64,
    },
}

/// One OHLCV bucket.
#[derive(Serialize)]
pub struct Candle {
    /// First slot in the bucket. Buckets are `[start_slot, start_slot + interval)`.
    pub start_slot: u64,
    /// Price of the first trade in the bucket.
    pub open: u64,
    /// Highest price traded.
    pub high: u64,
    /// Lowest price traded.
    pub low: u64,
    /// Price of the last trade in the bucket.
    pub close: u64,
    /// Total size, in base lots.
    pub base_lots: u64,
    /// Total gross value, in quote lots.
    pub quote_lots: u64,
    /// How many trades went into it.
    pub trades: u64,
}

impl From<&crate::candle::Candle> for Candle {
    fn from(candle: &crate::candle::Candle) -> Self {
        Self {
            start_slot: candle.start_slot,
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            base_lots: candle.base_lots,
            quote_lots: candle.quote_lots,
            trades: candle.trades,
        }
    }
}

/// A trade read back out of the store.
///
/// Always rooted — nothing else is ever written — so unlike the live shape it carries no
/// `finalized` flag to check.
#[derive(Serialize)]
pub struct HistoricalTrade {
    /// Slot it landed in.
    pub slot: u64,
    /// The transaction, hex-encoded.
    pub signature: String,
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
}

impl From<&crate::store::StoredTrade> for HistoricalTrade {
    fn from(trade: &crate::store::StoredTrade) -> Self {
        Self {
            slot: trade.slot,
            signature: trade.signature.iter().map(|b| format!("{b:02x}")).collect(),
            price_in_ticks: trade.price_in_ticks,
            base_lots: trade.base_lots,
            quote_lots: trade.quote_lots,
            taker_side: match trade.taker_side_is_bid {
                true => "bid",
                false => "ask",
            },
            maker_seat: trade.maker_seat,
        }
    }
}

/// Renders every trade in a delta.
pub fn trades_of(delta: &clob_indexer::BookDelta, finalized_through: u64) -> Vec<Trade> {
    delta
        .trades
        .iter()
        .map(|trade| Trade::new(trade, finalized_through))
        .collect()
}

/// Renders one side of a book, to `depth` levels.
pub fn levels_of(state: &clob_client::state::MarketState, side: Side, depth: usize) -> Vec<Level> {
    state.level_two(side, depth).iter().map(Level::from).collect()
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Bid => "bid",
        Side::Ask => "ask",
    }
}
