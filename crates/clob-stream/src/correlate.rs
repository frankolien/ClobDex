//! Pairing a market's new bytes with the instructions that wrote them.
//!
//! An account update carries what the market became; a transaction update carries what
//! was asked of it. Derivation needs both, and they arrive separately and in either
//! order. This holds one half until the other shows up.
//!
//! # Why the buffers are bounded
//!
//! A transaction whose account update never arrives, or an account update whose
//! transaction is never seen, would otherwise sit in memory forever. Both sides are
//! capped and evicted oldest-first: a dropped pairing costs one missing delta, which the
//! next snapshot corrects, whereas an unbounded buffer costs the process.

use std::collections::{HashMap, VecDeque};

use solana_pubkey::Pubkey;

use crate::source::{RawInstruction, Update};

/// How many unmatched halves to hold before dropping the oldest.
///
/// Generous enough that ordinary out-of-order delivery within a slot never evicts, small
/// enough that a pathological stream cannot exhaust memory.
pub const PENDING_CAPACITY: usize = 4_096;

/// A market's state before and after one transaction, with what that transaction asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Which market.
    pub market: Pubkey,
    /// Slot the transaction landed in.
    pub slot: u64,
    /// The transaction.
    pub signature: [u8; 64],
    /// Account data before it. `None` for the first update seen for a market, where
    /// there is nothing to diff against.
    pub before: Option<Vec<u8>>,
    /// Account data after it.
    pub after: Vec<u8>,
    /// Its instructions, in order.
    pub instructions: Vec<RawInstruction>,
}

/// Holds the halves until they can be paired.
#[derive(Default)]
pub struct Correlator {
    /// Last known bytes per market, which become the next change's `before`.
    latest: HashMap<Pubkey, Vec<u8>>,
    /// Transactions seen before the account write they caused.
    transactions: HashMap<[u8; 64], Vec<RawInstruction>>,
    /// Account writes seen before their transaction.
    accounts: HashMap<[u8; 64], (Pubkey, u64, Vec<u8>)>,
    /// Insertion order, for bounded eviction.
    transaction_order: VecDeque<[u8; 64]>,
    account_order: VecDeque<[u8; 64]>,
    /// Highest slot seen, from any update.
    tip: u64,
}

impl Correlator {
    /// A correlator holding nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// The highest slot any update has reported.
    pub fn tip(&self) -> u64 {
        self.tip
    }

    /// The last bytes seen for a market, if any.
    pub fn latest(&self, market: &Pubkey) -> Option<&[u8]> {
        self.latest.get(market).map(Vec::as_slice)
    }

    /// Seeds a market's state without producing a change.
    ///
    /// Used for the initial fetch, and for snapshot updates the cluster sends without
    /// attributing them to a transaction. There is nothing to derive from a state that
    /// arrived unexplained — only something to diff the *next* one against.
    pub fn seed(&mut self, market: Pubkey, data: Vec<u8>) {
        self.latest.insert(market, data);
    }

    /// Feeds one update in, and gets back a change when a pair completes.
    pub fn accept(&mut self, update: Update) -> Option<Change> {
        match update {
            Update::Slot { slot } => {
                self.tip = self.tip.max(slot);
                None
            }
            Update::Account {
                slot,
                market,
                data,
                signature,
            } => {
                self.tip = self.tip.max(slot);
                let Some(signature) = signature else {
                    // Unattributed: record it as the new baseline and derive nothing.
                    self.latest.insert(market, data);
                    return None;
                };
                match self.transactions.remove(&signature) {
                    Some(instructions) => Some(self.complete(market, slot, signature, data, instructions)),
                    None => {
                        self.remember_account(signature, market, slot, data);
                        None
                    }
                }
            }
            Update::Transaction {
                slot,
                signature,
                succeeded,
                instructions,
            } => {
                self.tip = self.tip.max(slot);
                // A failed transaction wrote nothing, so there is no account update
                // coming and nothing to pair. Holding it would only fill the buffer.
                if !succeeded {
                    return None;
                }
                match self.accounts.remove(&signature) {
                    Some((market, account_slot, data)) => {
                        Some(self.complete(market, account_slot, signature, data, instructions))
                    }
                    None => {
                        self.remember_transaction(signature, instructions);
                        None
                    }
                }
            }
        }
    }

    /// Builds the change and advances the market's baseline.
    fn complete(
        &mut self,
        market: Pubkey,
        slot: u64,
        signature: [u8; 64],
        after: Vec<u8>,
        instructions: Vec<RawInstruction>,
    ) -> Change {
        let before = self.latest.insert(market, after.clone());
        Change {
            market,
            slot,
            signature,
            before,
            after,
            instructions,
        }
    }

    fn remember_transaction(&mut self, signature: [u8; 64], instructions: Vec<RawInstruction>) {
        if self.transactions.insert(signature, instructions).is_none() {
            self.transaction_order.push_back(signature);
        }
        while self.transaction_order.len() > PENDING_CAPACITY {
            if let Some(oldest) = self.transaction_order.pop_front() {
                self.transactions.remove(&oldest);
            }
        }
    }

    fn remember_account(&mut self, signature: [u8; 64], market: Pubkey, slot: u64, data: Vec<u8>) {
        if self.accounts.insert(signature, (market, slot, data)).is_none() {
            self.account_order.push_back(signature);
        }
        while self.account_order.len() > PENDING_CAPACITY {
            if let Some(oldest) = self.account_order.pop_front() {
                self.accounts.remove(&oldest);
            }
        }
    }

    /// How many halves are waiting for their counterpart.
    pub fn pending(&self) -> usize {
        self.transactions.len() + self.accounts.len()
    }
}
