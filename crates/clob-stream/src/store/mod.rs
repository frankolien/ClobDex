//! Where the tape goes to outlive the process.
//!
//! # Only rooted trades are written
//!
//! A trade from a slot that is still `confirmed` can be rolled back. Writing those and
//! deleting them later would mean the store needs deletes — expensive in a columnar
//! database, and a source of rows that briefly existed and shouldn't have.
//!
//! So nothing is written until its slot finalizes. A retraction can then only ever touch
//! a trade still in memory, which by definition was never stored, and the store becomes
//! strictly append-only. That property is worth more than the extra second of latency:
//! append-only means a reader never sees a row that later turns out to be false.
//!
//! # Testable without a server
//!
//! [`Memory`] is a complete implementation, so everything downstream of the trait —
//! candles, historical queries, the flush path — is tested without infrastructure. The
//! same split as [`Source`](crate::source::Source), for the same reason.

pub mod memory;

use anyhow::Result;
use solana_pubkey::Pubkey;

pub use memory::Memory;

/// One trade, as stored.
///
/// Flat and owned rather than borrowing from the engine's types: a row is written once
/// and read back by things that have never heard of a `Tick`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTrade {
    /// Which market.
    pub market: Pubkey,
    /// Slot it landed in. Rooted by the time it is written.
    pub slot: u64,
    /// The transaction.
    pub signature: [u8; 64],
    /// Execution price — always the maker's.
    pub price_in_ticks: u64,
    /// Size, in base lots.
    pub base_lots: u64,
    /// Gross quote value, before fee.
    pub quote_lots: u64,
    /// Seat that owned the resting order.
    pub maker_seat: u32,
    /// Side the taker was on. A taker on the bid consumed asks.
    pub taker_side_is_bid: bool,
}

/// A query over stored trades.
#[derive(Copy, Clone, Debug)]
pub struct Range {
    /// Lowest slot to include.
    pub from_slot: u64,
    /// Highest slot to include.
    pub to_slot: u64,
    /// Most rows to return. Bounded so one query cannot ask for everything.
    pub limit: usize,
}

impl Range {
    /// The most recent `limit` trades, however far back they are.
    pub fn latest(limit: usize) -> Self {
        Self {
            from_slot: 0,
            to_slot: u64::MAX,
            limit,
        }
    }
}

/// Somewhere trades are kept.
///
/// Append-only by construction — see the module docs. There is deliberately no delete.
///
/// Boxed futures rather than `impl Future`, so the API can hold an `Arc<dyn Store>` and
/// be written once instead of once per backend. One allocation per query is nothing
/// against a database round trip.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Writes rooted trades. Must be idempotent on the whole row, because a reconnect
    /// can replay a slot that was already flushed.
    async fn append(&self, trades: &[StoredTrade]) -> Result<()>;

    /// Reads trades for one market, oldest first.
    async fn trades(&self, market: &Pubkey, range: Range) -> Result<Vec<StoredTrade>>;

    /// The highest slot written for a market, if any.
    ///
    /// Read at startup so a restart resumes where it left off instead of re-flushing
    /// everything it can still see.
    async fn highest_slot(&self, market: &Pubkey) -> Result<Option<u64>>;
}
