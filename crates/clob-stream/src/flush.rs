//! Moving rooted trades out of memory and into the store.
//!
//! Nothing is written until its slot finalizes. That single rule is what makes the store
//! append-only: a retraction can only ever target a slot that is still `confirmed`, and
//! such a trade has not been written yet, so there is never a row to delete.
//!
//! The consequence is that a trade is durable about a second after it is visible. That
//! is the right trade: a reader of the store never sees a row that later turns out to be
//! false, and a reader who wants the faster answer has the live feed, which says plainly
//! which trades are still provisional.

use std::collections::BTreeMap;

use anyhow::Result;
use clob_indexer::Trade;
use solana_pubkey::Pubkey;

use crate::store::{StoredTrade, Store};

/// Trades waiting for their slot to be rooted.
///
/// Keyed by slot so finalization drains a contiguous prefix and a retraction removes one
/// key, both without scanning.
#[derive(Default)]
pub struct Pending {
    by_slot: BTreeMap<u64, Vec<StoredTrade>>,
    /// Highest slot already written, so a replayed slot is not queued a second time.
    flushed_through: u64,
}

impl Pending {
    /// Nothing waiting.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts from a slot already known to be in the store.
    ///
    /// Read at startup so a restart does not re-queue everything the endpoint replays.
    /// The store deduplicates anyway, but not doing the work is better than undoing it.
    pub fn resuming_from(slot: u64) -> Self {
        Self {
            by_slot: BTreeMap::new(),
            flushed_through: slot,
        }
    }

    /// How many trades are waiting.
    pub fn len(&self) -> usize {
        self.by_slot.values().map(Vec::len).sum()
    }

    /// Whether anything is waiting.
    pub fn is_empty(&self) -> bool {
        self.by_slot.is_empty()
    }

    /// Queues a transaction's trades until its slot is rooted.
    pub fn record(&mut self, market: Pubkey, slot: u64, signature: [u8; 64], trades: &[Trade]) {
        if trades.is_empty() || slot <= self.flushed_through {
            return;
        }
        self.by_slot
            .entry(slot)
            .or_default()
            .extend(trades.iter().map(|trade| StoredTrade {
                market,
                slot,
                signature,
                price_in_ticks: trade.price_in_ticks.as_u64(),
                base_lots: trade.base_lots.as_u64(),
                quote_lots: trade.quote_lots.as_u64(),
                maker_seat: trade.maker_seat,
                taker_side_is_bid: trade.taker_side == clob_book::Side::Bid,
            }));
    }

    /// Drops everything from an abandoned slot.
    ///
    /// Returns how many trades went. A rooted slot cannot be abandoned, so anything this
    /// removes was never written — which is precisely why the store needs no delete.
    pub fn retract(&mut self, slot: u64) -> usize {
        self.by_slot.remove(&slot).map(|trades| trades.len()).unwrap_or(0)
    }

    /// Takes everything at or below `slot`, which is now rooted.
    pub fn take_through(&mut self, slot: u64) -> Vec<StoredTrade> {
        if slot <= self.flushed_through {
            return Vec::new();
        }
        // split_off leaves the rooted prefix behind and keeps the rest.
        let still_pending = self.by_slot.split_off(&(slot + 1));
        let rooted = std::mem::replace(&mut self.by_slot, still_pending);
        self.flushed_through = slot;
        rooted.into_values().flatten().collect()
    }

    /// The highest slot written so far.
    pub fn flushed_through(&self) -> u64 {
        self.flushed_through
    }
}

/// Writes everything rooted by `slot`.
///
/// Returns how many trades were written. A failure leaves them queued rather than
/// dropping them: the next finalization retries, and a store that is briefly unreachable
/// should cost latency, not data.
pub async fn flush(pending: &mut Pending, store: &dyn Store, slot: u64) -> Result<usize> {
    let rooted = pending.take_through(slot);
    if rooted.is_empty() {
        return Ok(0);
    }

    match store.append(&rooted).await {
        Ok(()) => Ok(rooted.len()),
        Err(error) => {
            pending.requeue(rooted);
            Err(error)
        }
    }
}

impl Pending {
    /// Puts trades back after a failed write.
    fn requeue(&mut self, trades: Vec<StoredTrade>) {
        let lowest = trades.iter().map(|trade| trade.slot).min();
        for trade in trades {
            self.by_slot.entry(trade.slot).or_default().push(trade);
        }
        // The watermark moves back with them, or the retry would skip them as already
        // flushed and the loss would be silent.
        if let Some(lowest) = lowest {
            self.flushed_through = self.flushed_through.min(lowest.saturating_sub(1));
        }
    }
}
