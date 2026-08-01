//! An in-memory store.
//!
//! Not a stub — a complete implementation, so everything downstream of the trait is
//! tested without infrastructure. Also genuinely useful: a single-market deployment that
//! only needs a live feed can run on this and skip the database entirely.

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::Result;
use solana_pubkey::Pubkey;

use super::{Range, StoredTrade, Store};

/// Trades kept in the process.
#[derive(Default)]
pub struct Memory {
    /// Per market, ordered by slot as written.
    trades: RwLock<HashMap<Pubkey, Vec<StoredTrade>>>,
}

impl Memory {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many trades are held, across every market.
    pub fn len(&self) -> usize {
        self.trades
            .read()
            .expect("store lock poisoned")
            .values()
            .map(Vec::len)
            .sum()
    }

    /// Whether anything has been written.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl Store for Memory {
    async fn append(&self, trades: &[StoredTrade]) -> Result<()> {
        let mut all = self.trades.write().expect("store lock poisoned");
        for trade in trades {
            let market = all.entry(trade.market).or_default();
            // Idempotent on the same trade arriving twice, which a reconnect can cause
            // by replaying a slot that was already flushed. A store that deduplicated
            // only on signature would drop the other fills in a multi-fill transaction.
            let duplicate = market.iter().any(|existing| {
                existing.signature == trade.signature
                    && existing.slot == trade.slot
                    && existing.maker_seat == trade.maker_seat
                    && existing.price_in_ticks == trade.price_in_ticks
                    && existing.base_lots == trade.base_lots
            });
            if !duplicate {
                market.push(trade.clone());
            }
        }
        Ok(())
    }

    async fn trades(&self, market: &Pubkey, range: Range) -> Result<Vec<StoredTrade>> {
        let all = self.trades.read().expect("store lock poisoned");
        let Some(trades) = all.get(market) else {
            return Ok(Vec::new());
        };

        let mut matching: Vec<StoredTrade> = trades
            .iter()
            .filter(|trade| trade.slot >= range.from_slot && trade.slot <= range.to_slot)
            .cloned()
            .collect();
        matching.sort_by_key(|trade| trade.slot);

        // The limit takes the most recent, then order is restored — asking for "the last
        // 100" and getting the first 100 ever written is the wrong answer to that query.
        if matching.len() > range.limit {
            matching.drain(..matching.len() - range.limit);
        }
        Ok(matching)
    }

    async fn highest_slot(&self, market: &Pubkey) -> Result<Option<u64>> {
        Ok(self
            .trades
            .read()
            .expect("store lock poisoned")
            .get(market)
            .and_then(|trades| trades.iter().map(|trade| trade.slot).max()))
    }
}
