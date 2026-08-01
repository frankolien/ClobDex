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
    /// Seat that crossed it, when the derivation could say which. `null` otherwise.
    pub taker_seat: Option<u32>,
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
            taker_seat: trade.taker_seat,
            finalized: trade.slot <= finalized_through,
        }
    }
}

/// The tick and lot geometry a client needs to render any of these numbers.
///
/// Carried on the summary so a market list does not need one account fetch per row just
/// to turn ticks into a price. It is immutable after market creation, so a client can
/// cache it for as long as it keeps the address.
#[derive(Serialize)]
pub struct Lots {
    /// Base lots per whole base unit.
    pub base_lots_per_base_unit: u64,
    /// One tick, in quote lots per base unit.
    pub tick_size_in_quote_lots_per_base_unit: u64,
    /// Base-token atoms per base lot.
    pub base_atoms_per_base_lot: u64,
    /// Quote-token atoms per quote lot.
    pub quote_atoms_per_quote_lot: u64,
}

impl From<&clob_book::LotConfig> for Lots {
    fn from(config: &clob_book::LotConfig) -> Self {
        Self {
            base_lots_per_base_unit: config.base_lots_per_base_unit,
            tick_size_in_quote_lots_per_base_unit: config.tick_size_in_quote_lots_per_base_unit,
            base_atoms_per_base_lot: config.base_atoms_per_base_lot,
            quote_atoms_per_quote_lot: config.quote_atoms_per_quote_lot,
        }
    }
}

/// One market, as a list or a landing page wants it.
///
/// Everything here is read from state already in memory, so serving every tracked market
/// costs no queries. Rolling volume is deliberately absent for that reason — it needs the
/// store, and putting it here would make the cheapest endpoint the most expensive one.
/// It lives on [`Window`] behind a route that asks for it.
///
/// The optional prices are `None` on an empty or one-sided book, which is a real state
/// for a new market and not an error. A client that renders `null` as zero will draw a
/// market trading at zero.
#[derive(Serialize)]
pub struct MarketSummary {
    /// The market account.
    pub market: String,
    /// Slot the book state came from.
    pub slot: u64,
    /// Everything at or below this slot is rooted.
    pub finalized_through: u64,
    /// Base token mint.
    pub base_mint: String,
    /// Quote token mint.
    pub quote_mint: String,
    /// Taker fee, in basis points.
    pub taker_fee_bps: u64,
    /// Tick and lot geometry.
    pub lots: Lots,
    /// Best bid, if the side has liquidity.
    pub best_bid_in_ticks: Option<u64>,
    /// Best ask, if the side has liquidity.
    pub best_ask_in_ticks: Option<u64>,
    /// Ask minus bid, when both sides have liquidity.
    pub spread_in_ticks: Option<u64>,
    /// Midpoint, when both sides have liquidity.
    pub mid_price_in_ticks: Option<u64>,
    /// Price of the most recent trade this process has seen, if it has seen one.
    ///
    /// From the in-memory tape, so it is empty after a restart until something trades —
    /// unlike the book, which is restored from a checkpoint. A client that needs a last
    /// price across restarts should read it from the history endpoint.
    pub last_price_in_ticks: Option<u64>,
    /// Resting orders on the bid.
    pub bid_orders: usize,
    /// Resting orders on the ask.
    pub ask_orders: usize,
    /// Base lots the market holds for all seats.
    pub base_lots_deposited: u64,
    /// Quote lots the market holds for all seats, including unclaimed fees.
    pub quote_lots_deposited: u64,
    /// Seats claimed.
    pub seats: usize,
    /// Trades this process has seen for the market.
    pub trades_seen: u64,
}

impl MarketSummary {
    /// Summarises one tracked market.
    pub fn new(market: &solana_pubkey::Pubkey, view: &crate::registry::MarketView) -> Self {
        let state = &view.state;
        Self {
            market: market.to_string(),
            slot: view.slot,
            finalized_through: view.finalized_through,
            base_mint: solana_pubkey::Pubkey::new_from_array(state.account.base_mint).to_string(),
            quote_mint: solana_pubkey::Pubkey::new_from_array(state.account.quote_mint).to_string(),
            taker_fee_bps: state.fees().taker_fee_bps,
            lots: Lots::from(state.lot_config()),
            best_bid_in_ticks: state.best_bid().map(|o| o.price_in_ticks().as_u64()),
            best_ask_in_ticks: state.best_ask().map(|o| o.price_in_ticks().as_u64()),
            spread_in_ticks: state.spread_in_ticks(),
            mid_price_in_ticks: state.mid_price_in_ticks(),
            last_price_in_ticks: view.tape.last().map(|t| t.price_in_ticks.as_u64()),
            bid_orders: state.bids.len(),
            ask_orders: state.asks.len(),
            base_lots_deposited: state.header.base_lots_deposited.as_u64(),
            quote_lots_deposited: state.header.quote_lots_deposited.as_u64(),
            seats: state.traders.len(),
            trades_seen: view.trades_seen,
        }
    }
}

/// What traded over a span of slots.
///
/// Measured in slots, not hours. A trade carries a slot; turning that into a wall clock
/// means trusting block times, which drift and are occasionally revised (see
/// [`candle`](crate::candle)). The span a caller asks for is the span it gets, and the
/// only place a "24 hours is about this many slots" assumption exists is the default the
/// handler applies — written down, once, rather than baked into the numbers.
///
/// Prices are absent rather than zero when nothing traded in the span, which is the
/// ordinary state of a quiet market.
#[derive(Serialize)]
pub struct Window {
    /// The market.
    pub market: String,
    /// Lowest slot included.
    pub from_slot: u64,
    /// Highest slot included. Both ends are inclusive.
    pub to_slot: u64,
    /// Slots the span covers.
    pub slots: u64,
    /// Price of the first trade in the span.
    pub open_in_ticks: Option<u64>,
    /// Highest price traded.
    pub high_in_ticks: Option<u64>,
    /// Lowest price traded.
    pub low_in_ticks: Option<u64>,
    /// Price of the last trade in the span.
    pub close_in_ticks: Option<u64>,
    /// Close minus open. Signed, because a market can fall.
    pub change_in_ticks: Option<i64>,
    /// Volume-weighted average price.
    pub vwap_in_ticks: Option<u64>,
    /// Total size traded, in base lots.
    pub base_lots: u64,
    /// Total gross value traded, in quote lots.
    pub quote_lots: u64,
    /// How many fills went into it.
    pub trades: u64,
    /// Whether the span held more trades than one query may read.
    ///
    /// When true, everything above describes only the most recent trades in the span, so
    /// the volume is a floor and the open is not the span's open. Reported rather than
    /// hidden: a truncated total is indistinguishable from a real one, and a venue
    /// under-reporting its own volume without saying so is still misreporting it.
    pub truncated: bool,
}

impl Window {
    /// Summarises the trades a span returned.
    ///
    /// `trades` must be what the store returned for exactly this span, oldest first.
    pub fn new(
        market: &solana_pubkey::Pubkey,
        from_slot: u64,
        to_slot: u64,
        trades: &[crate::store::StoredTrade],
        truncated: bool,
    ) -> Self {
        let candle = crate::candle::summarise(trades);
        Self {
            market: market.to_string(),
            from_slot,
            to_slot,
            slots: to_slot.saturating_sub(from_slot).saturating_add(1),
            open_in_ticks: candle.as_ref().map(|c| c.open),
            high_in_ticks: candle.as_ref().map(|c| c.high),
            low_in_ticks: candle.as_ref().map(|c| c.low),
            close_in_ticks: candle.as_ref().map(|c| c.close),
            // Through i128 so the subtraction cannot wrap before it is narrowed. Two u64
            // prices can differ by more than an i64 holds; a change that large is not a
            // real market, and reporting nothing beats reporting a negative rally.
            change_in_ticks: candle
                .as_ref()
                .and_then(|c| i64::try_from(i128::from(c.close) - i128::from(c.open)).ok()),
            vwap_in_ticks: crate::candle::vwap(trades),
            base_lots: candle.as_ref().map(|c| c.base_lots).unwrap_or(0),
            quote_lots: candle.as_ref().map(|c| c.quote_lots).unwrap_or(0),
            trades: candle.as_ref().map(|c| c.trades).unwrap_or(0),
            truncated,
        }
    }
}

/// One of a trader's resting orders.
#[derive(Serialize)]
pub struct OpenOrder {
    /// Which side it rests on.
    pub side: &'static str,
    /// Limit price.
    pub price_in_ticks: u64,
    /// The identity `CancelOrder` takes, together with the price.
    ///
    /// Side-encoded — bids store the complement of the counter so one ascending
    /// comparison gives price-time priority on both sides — so this is *not* the arrival
    /// order and must not be compared against one. It is the value to send back.
    pub order_sequence_number: u64,
    /// Arrival order, decoded.
    ///
    /// What the tape records as `maker_order_sequence`, so this is the field to join a
    /// fill against. Carried alongside the encoded one rather than instead of it because
    /// the two are equal on asks and complements on bids: a client that had only one
    /// would work perfectly until someone cancelled a bid.
    pub sequence_number: u64,
    /// Size still resting.
    pub base_lots: u64,
}

/// A trader's position in one market.
///
/// The seat is the market's own index for a wallet. Balances are held by the market, not
/// by the wallet: `free` can be withdrawn or committed to a new order, `locked` is
/// already committed to something resting. A dashboard showing only one of the two
/// reports a balance that does not match the wallet's own arithmetic.
#[derive(Serialize)]
pub struct TraderView {
    /// The market.
    pub market: String,
    /// The wallet.
    pub trader: String,
    /// The market's index for this wallet. Resting orders and tape entries name it.
    pub seat: u32,
    /// Slot this state came from.
    pub slot: u64,
    /// Everything at or below this slot is rooted.
    pub finalized_through: u64,
    /// Base lots available to withdraw or commit.
    pub base_lots_free: u64,
    /// Base lots committed to resting asks.
    pub base_lots_locked: u64,
    /// Quote lots available to withdraw or commit.
    pub quote_lots_free: u64,
    /// Quote lots committed to resting bids.
    pub quote_lots_locked: u64,
    /// Everything this trader has resting, best price first on each side.
    pub orders: Vec<OpenOrder>,
}

impl TraderView {
    /// A trader's balances and resting orders, or `None` if it holds no seat here.
    ///
    /// No seat is a different answer from an empty one: a wallet that never traded this
    /// market has no row, while one that withdrew everything has a row of zeroes. A
    /// dashboard needs to tell "you are not set up here" from "you are, and you are flat".
    pub fn new(
        market: &solana_pubkey::Pubkey,
        trader: &solana_pubkey::Pubkey,
        view: &crate::registry::MarketView,
    ) -> Option<Self> {
        let state = &view.state;
        let key = clob_engine::TraderKey(trader.to_bytes());
        let seat = state.seat_of(&key)?;
        let balances = state.trader(&key)?;

        let orders = [Side::Bid, Side::Ask]
            .into_iter()
            .flat_map(|side| {
                state
                    .side(side)
                    .iter()
                    .filter(move |order| order.trader_index == seat)
                    .map(move |order| OpenOrder {
                        side: side_name(side),
                        price_in_ticks: order.price_in_ticks().as_u64(),
                        order_sequence_number: order.id.order_sequence_number,
                        sequence_number: order.id.sequence_number(),
                        base_lots: order.num_base_lots.as_u64(),
                    })
            })
            .collect();

        Some(Self {
            market: market.to_string(),
            trader: trader.to_string(),
            seat,
            slot: view.slot,
            finalized_through: view.finalized_through,
            base_lots_free: balances.base_lots_free.as_u64(),
            base_lots_locked: balances.base_lots_locked.as_u64(),
            quote_lots_free: balances.quote_lots_free.as_u64(),
            quote_lots_locked: balances.quote_lots_locked.as_u64(),
            orders,
        })
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
    /// Seat that crossed it, when the derivation could say which. `null` otherwise.
    pub taker_seat: Option<u32>,
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
            taker_seat: trade.taker_seat,
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
