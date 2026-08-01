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
    /// Deltas whose derived fees disagreed with the market's own counter.
    ///
    /// Non-zero means the derivation and the program disagree about what happened, which
    /// is a bug or a wire-format change — worth surfacing rather than hiding.
    pub reconciliation_failures: u64,
}

/// Everything being tracked, shared between ingest and the API.
pub struct Registry {
    markets: RwLock<HashMap<Pubkey, MarketView>>,
    updates: broadcast::Sender<Arc<Derived>>,
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
        let _ = self.updates.send(Arc::new(derived));
    }

    /// Seeds a market's state without recording any trades.
    pub fn seed(&self, market: Pubkey, state: MarketState, slot: u64) {
        let mut markets = self.markets.write().expect("registry lock poisoned");
        markets.entry(market).or_insert(MarketView {
            state,
            slot,
            tape: Vec::new(),
            trades_seen: 0,
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

    /// A live feed of derived changes.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Derived>> {
        self.updates.subscribe()
    }
}
