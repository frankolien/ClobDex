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

pub mod clickhouse;
pub mod file;
pub mod memory;

use anyhow::Result;
use solana_pubkey::Pubkey;

pub use file::Files;
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
    /// Sequence number of the resting order that was hit.
    ///
    /// The only field that distinguishes two fills of the same size, at the same price,
    /// against the same maker, in the same transaction — which is what a taker sweeping
    /// a maker's refreshed quotes produces. Without it those two rows are identical and
    /// deduplication drops one, under-reporting volume.
    pub maker_order_sequence: u64,
    /// Side the taker was on. A taker on the bid consumed asks.
    pub taker_side_is_bid: bool,
}

/// A market's raw account bytes at a rooted slot.
///
/// Trades alone are not enough to resume: derivation diffs one book against another, so
/// picking up where a previous process stopped needs the book as it stood there. Without
/// it, a restart can only baseline on the next update it happens to see — losing that
/// transaction and everything between the two runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// Slot the bytes are true at. Rooted, so it cannot be rolled back.
    pub slot: u64,
    /// The account's full data.
    pub data: Vec<u8>,
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

    /// Records a market's state at a rooted slot, replacing any earlier one.
    ///
    /// Only ever called with rooted state, so a checkpoint can never describe a book
    /// that was later rolled back.
    async fn save_checkpoint(&self, market: &Pubkey, checkpoint: &Checkpoint) -> Result<()>;

    /// The most recent checkpoint for a market.
    async fn checkpoint(&self, market: &Pubkey) -> Result<Option<Checkpoint>>;

    /// Every market that has a checkpoint.
    ///
    /// Read at startup to decide where to resume the stream from, before any market is
    /// known from anywhere else.
    async fn checkpointed_markets(&self) -> Result<Vec<Pubkey>>;
}
