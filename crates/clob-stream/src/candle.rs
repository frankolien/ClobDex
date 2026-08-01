//! OHLCV, aggregated from stored trades.
//!
//! # Bucketed by slot, not by wall clock
//!
//! A slot is what a trade actually carries. Mapping slots to timestamps means trusting
//! the cluster's block times, which drift and are occasionally revised — and a candle
//! whose boundary moves is worse than one measured in a slightly odd unit. Callers that
//! want minutes convert at the edge, where the fudge is visible.
//!
//! # One implementation
//!
//! This runs in Rust over rows the store returned, rather than as a `GROUP BY` in the
//! database. A SQL rollup would be faster and would be a second copy of this logic —
//! exactly the drift this codebase avoids everywhere else. If it ever becomes the
//! bottleneck, this stays as the reference the rollup is tested against.

use crate::store::StoredTrade;

/// One aggregation bucket.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Aggregates trades into buckets of `interval` slots.
///
/// `trades` must be ordered by slot, which is what the store returns. Empty intervals
/// produce no candle rather than a flat one: a bucket with no trades has no open and no
/// close, and inventing them by carrying the previous price states a fact that was never
/// observed. A caller drawing a chart can forward-fill; a caller computing a statistic
/// must not have that done for it silently.
pub fn aggregate(trades: &[StoredTrade], interval: u64) -> Vec<Candle> {
    if interval == 0 {
        return Vec::new();
    }

    let mut candles: Vec<Candle> = Vec::new();
    for trade in trades {
        let start_slot = (trade.slot / interval) * interval;

        match candles.last_mut() {
            Some(candle) if candle.start_slot == start_slot => {
                candle.high = candle.high.max(trade.price_in_ticks);
                candle.low = candle.low.min(trade.price_in_ticks);
                candle.close = trade.price_in_ticks;
                candle.base_lots = candle.base_lots.saturating_add(trade.base_lots);
                candle.quote_lots = candle.quote_lots.saturating_add(trade.quote_lots);
                candle.trades += 1;
            }
            _ => candles.push(Candle {
                start_slot,
                open: trade.price_in_ticks,
                high: trade.price_in_ticks,
                low: trade.price_in_ticks,
                close: trade.price_in_ticks,
                base_lots: trade.base_lots,
                quote_lots: trade.quote_lots,
                trades: 1,
            }),
        }
    }
    candles
}

/// Collapses every trade into one bucket, whatever slots they came from.
///
/// A window is a candle whose boundaries the caller chose rather than the interval. Built
/// by [`aggregate`] rather than beside it so that open, close, high and low mean the same
/// thing in a 24-hour statistic as they do in a one-minute bar — two folds that agree
/// today would not stay agreeing.
///
/// The interval is [`u64::MAX`], which puts every slot below it in bucket zero. That is
/// every slot that can exist: a slot equal to `u64::MAX` would need the chain to have
/// produced 2^64 of them.
///
/// `None` when nothing traded. A window with no trades has no open and no close, and
/// zeroes there read as a market that printed at zero.
pub fn summarise(trades: &[StoredTrade]) -> Option<Candle> {
    aggregate(trades, u64::MAX).pop()
}

/// Volume-weighted average price across a set of trades, in ticks.
///
/// `None` when nothing traded — a VWAP of zero would read as a real price of zero.
pub fn vwap(trades: &[StoredTrade]) -> Option<u64> {
    let base: u128 = trades.iter().map(|t| t.base_lots as u128).sum();
    if base == 0 {
        return None;
    }
    let weighted: u128 = trades
        .iter()
        .map(|t| t.price_in_ticks as u128 * t.base_lots as u128)
        .sum();
    u64::try_from(weighted / base).ok()
}
