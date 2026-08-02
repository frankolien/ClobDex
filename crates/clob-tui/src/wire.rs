//! The JSON `clob-stream` sends, spelled exactly as it arrives.
//!
//! # Why every quantity is a string here
//!
//! The indexer quotes money and order identities as decimal strings because JSON has one
//! numeric type — an IEEE-754 double — and a `u64` does not fit in one. A bid's stored
//! sequence number is the complement of the arrival counter, so it sits just below
//! `u64::MAX`, and a JavaScript client reading it as a number would cancel an order that
//! does not exist.
//!
//! That constraint is not ours. Rust parses these back to `u64` exactly, so the round trip
//! is lossless in both directions and the strings cost one `parse` per field. They are kept
//! as `String` in these types rather than deserialised straight to `u64` so that a
//! malformed field fails where it is read, with the field named, rather than failing the
//! whole frame with serde's message about the wrong type.

use serde::Deserialize;

/// One aggregated price level.
#[derive(Clone, Debug, Deserialize)]
pub struct Level {
    pub price_in_ticks: String,
    pub base_lots: String,
}

/// Tick and lot geometry, carried on every market summary.
#[derive(Clone, Debug, Deserialize)]
pub struct Lots {
    pub base_lots_per_base_unit: String,
    pub tick_size_in_quote_lots_per_base_unit: String,
    pub base_atoms_per_base_lot: String,
    pub quote_atoms_per_quote_lot: String,
}

/// One market, as `/v1/markets` returns it.
#[derive(Clone, Debug, Deserialize)]
pub struct MarketSummary {
    pub market: String,
    pub slot: u64,
    pub finalized_through: u64,
    pub base_mint: String,
    pub quote_mint: String,
    pub taker_fee_bps: u64,
    pub lots: Lots,
    pub best_bid_in_ticks: Option<String>,
    pub best_ask_in_ticks: Option<String>,
    pub spread_in_ticks: Option<String>,
    pub mid_price_in_ticks: Option<String>,
    pub last_price_in_ticks: Option<String>,
    pub bid_orders: usize,
    pub ask_orders: usize,
    pub seats: usize,
    pub trades_seen: u64,
}

/// One fill.
#[derive(Clone, Debug, Deserialize)]
pub struct Trade {
    pub slot: u64,
    pub price_in_ticks: String,
    pub base_lots: String,
    pub taker_side: String,
    pub maker_seat: u32,
    pub taker_seat: Option<u32>,
}

/// One of a trader's resting orders.
#[derive(Clone, Debug, Deserialize)]
pub struct OpenOrder {
    pub side: String,
    pub price_in_ticks: String,
    pub base_lots: String,
}

/// A trader's position in one market.
#[derive(Clone, Debug, Deserialize)]
pub struct TraderView {
    pub seat: u32,
    pub base_lots_free: String,
    pub base_lots_locked: String,
    pub quote_lots_free: String,
    pub quote_lots_locked: String,
    pub orders: Vec<OpenOrder>,
}

/// A message from the live feed.
///
/// Four kinds, and the two that are easy to skip are the two that matter. `retract` says
/// fills already on screen did not happen, because the slot that produced them was
/// abandoned. `lagged` says this subscriber fell behind and messages were dropped — silence
/// and a gap look identical, so the feed says which it is.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Snapshot {
        slot: u64,
        finalized_through: u64,
        bids: Vec<Level>,
        asks: Vec<Level>,
    },
    Update {
        slot: u64,
        trades: Vec<Trade>,
        bids: Vec<Level>,
        asks: Vec<Level>,
        finalized_through: u64,
    },
    Retract {
        slot: u64,
        trades: usize,
    },
    Lagged {
        missed: u64,
    },
}
