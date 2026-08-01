//! What the read side serves from.
//!
//! Ingest runs on its own tokio runtime and actix runs `!Send` futures per worker, so
//! the two share state through an `Arc` and a lock rather than by passing values. The
//! lock is held only long enough to clone out a snapshot: an HTTP handler that awaited
//! while holding it would stall ingest for every market at once.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use clob_client::state::MarketState;
use clob_indexer::Trade;
use solana_pubkey::Pubkey;
use tokio::sync::broadcast;

use crate::pipeline::Derived;

/// How many recent trades to keep per market.
///
/// The tape is a stream, not a store: anything older belongs in a database, and holding
/// an unbounded history in a process that never restarts is a slow memory leak.
pub const TAPE_CAPACITY: usize = 1_024;

/// How many undelivered messages a subscriber may fall behind by before it is dropped.
///
/// A slow WebSocket client must not be able to stall ingest, so the channel drops for
/// that subscriber rather than applying back-pressure to the producer.
pub const BROADCAST_CAPACITY: usize = 256;

/// One market's current state and recent history.
#[derive(Clone)]
pub struct MarketView {
    /// The book as of the last processed transaction.
    pub state: MarketState,
    /// Slot that state came from.
    pub slot: u64,
    /// Most recent trades, oldest first.
    pub tape: Vec<Trade>,
    /// Trades seen since the process started.
    pub trades_seen: u64,
    /// Every slot at or below this is rooted, so its trades can no longer be rolled back.
    ///
    /// A consumer that cannot tolerate a retraction should ignore anything above it.
    pub finalized_through: u64,
    /// Trades withdrawn because the slot that produced them was abandoned.
    pub trades_retracted: u64,
    /// Deltas whose derived fees disagreed with the market's own counter.
    ///
    /// Non-zero means the derivation and the program disagree about what happened, which
    /// is a bug or a wire-format change — worth surfacing rather than hiding.
    pub reconciliation_failures: u64,
}

/// Something a subscriber needs to know about.
#[derive(Clone)]
pub enum Event {
    /// One transaction's effect on a market.
    Change(Arc<Derived>),
    /// Trades withdrawn because the slot that produced them was abandoned.
    ///
    /// Published rather than quietly dropped: a client that already showed a trade has
    /// to be told it did not happen, and silence looks identical to a quiet market.
    Retracted {
        /// Which market.
        market: Pubkey,
        /// The slot that was dropped.
        slot: u64,
        /// How many trades went with it.
        trades: usize,
    },
}

/// Everything being tracked, shared between ingest and the API.
pub struct Registry {
    markets: RwLock<HashMap<Pubkey, MarketView>>,
    updates: broadcast::Sender<Event>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Arc<Self> {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            markets: RwLock::new(HashMap::new()),
            updates,
        })
    }

    /// Records a derived change and notifies subscribers.
    pub fn apply(&self, derived: Derived, reconciled: bool) {
        {
            let mut markets = self.markets.write().expect("registry lock poisoned");
            let view = markets
                .entry(derived.market)
                .or_insert_with(|| MarketView {
                    state: derived.state.clone(),
                    slot: derived.slot,
                    tape: Vec::new(),
                    trades_seen: 0,
                    finalized_through: 0,
                    trades_retracted: 0,
                    reconciliation_failures: 0,
                });

            view.state = derived.state.clone();
            view.slot = derived.slot;
            view.trades_seen += derived.delta.trades.len() as u64;
            if !reconciled {
                view.reconciliation_failures += 1;
            }

            view.tape.extend(derived.delta.trades.iter().copied());
            if view.tape.len() > TAPE_CAPACITY {
                view.tape.drain(..view.tape.len() - TAPE_CAPACITY);
            }
        }

        // Fails only when nobody is listening, which is the normal case.
        let _ = self.updates.send(Event::Change(Arc::new(derived)));
    }

    /// Records that every slot up to `slot` is rooted.
    ///
    /// Not broadcast. Slots finalize continuously, so a message per slot per subscriber
    /// would be almost entirely noise; the number rides along on the snapshot and on
    /// every change instead.
    pub fn finalize(&self, slot: u64) {
        let mut markets = self.markets.write().expect("registry lock poisoned");
        for view in markets.values_mut() {
            view.finalized_through = view.finalized_through.max(slot);
        }
    }

    /// Withdraws every trade that came from an abandoned slot.
    ///
    /// Only the tape is corrected. Account state needs no rollback: the writes from a
    /// dead slot were never real, and the next update from a live slot carries the true
    /// state — so a stale book self-heals, while a phantom trade never would.
    pub fn retract(&self, slot: u64) {
        let retracted: Vec<(Pubkey, usize)> = {
            let mut markets = self.markets.write().expect("registry lock poisoned");
            markets
                .iter_mut()
                .filter_map(|(market, view)| {
                    let before = view.tape.len();
                    view.tape.retain(|trade| trade.slot != slot);
                    let removed = before - view.tape.len();
                    if removed == 0 {
                        return None;
                    }
                    view.trades_retracted += removed as u64;
                    // trades_seen counts what was published, and a retraction is not an
                    // un-publishing — the two are reported separately rather than netted.
                    Some((*market, removed))
                })
                .collect()
        };

        for (market, trades) in retracted {
            let _ = self.updates.send(Event::Retracted {
                market,
                slot,
                trades,
            });
        }
    }

    /// Seeds a market's state without recording any trades.
    pub fn seed(&self, market: Pubkey, state: MarketState, slot: u64) {
        let mut markets = self.markets.write().expect("registry lock poisoned");
        markets.entry(market).or_insert(MarketView {
            state,
            slot,
            tape: Vec::new(),
            trades_seen: 0,
            finalized_through: 0,
            trades_retracted: 0,
            reconciliation_failures: 0,
        });
    }

    /// A snapshot of one market.
    pub fn market(&self, market: &Pubkey) -> Option<MarketView> {
        self.markets
            .read()
            .expect("registry lock poisoned")
            .get(market)
            .cloned()
    }

    /// Every market being tracked.
    pub fn markets(&self) -> Vec<Pubkey> {
        self.markets
            .read()
            .expect("registry lock poisoned")
            .keys()
            .copied()
            .collect()
    }

    /// A live feed of changes and retractions.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.updates.subscribe()
    }
}
