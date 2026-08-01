//! What arrives from a cluster, and where from.
//!
//! Everything downstream depends on [`Update`] rather than on a gRPC stream, which is
//! what makes the rest of this crate testable without an endpoint. [`Replay`] is a
//! source that yields a scripted sequence; the LaserStream source yields the same shape
//! from a real cluster.

use solana_pubkey::Pubkey;

/// One thing that happened, as the cluster reported it.
///
/// Account and transaction updates arrive on the same stream but separately: an account
/// update carries the market's new bytes, and a transaction update carries the
/// instructions that produced them. Neither is sufficient alone, which is why
/// [`correlate`](crate::correlate) exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// A market account's contents after some transaction wrote them.
    Account {
        /// Slot the write landed in.
        slot: u64,
        /// Which market.
        market: Pubkey,
        /// The account's full data.
        data: Vec<u8>,
        /// The transaction that wrote it, when the cluster reported one.
        ///
        /// Absent for snapshot and startup updates, which describe a state without
        /// attributing it to anything — nothing can be derived from those, only stored.
        signature: Option<[u8; 64]>,
    },
    /// A transaction touching this program.
    Transaction {
        /// Slot it landed in.
        slot: u64,
        /// Its signature.
        signature: [u8; 64],
        /// Whether it succeeded. Failed transactions change nothing and are dropped.
        succeeded: bool,
        /// Its instructions addressed to this program, in order.
        instructions: Vec<RawInstruction>,
    },
    /// A slot reached a commitment level. Used to know how far the stream has advanced
    /// even while no market is trading.
    Slot {
        /// The slot.
        slot: u64,
    },
}

/// One instruction, with the accounts it names already resolved to addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawInstruction {
    /// The program it addresses.
    pub program_id: Pubkey,
    /// Its accounts, in order.
    pub accounts: Vec<Pubkey>,
    /// Its data.
    pub data: Vec<u8>,
}

impl RawInstruction {
    /// The account at `index`, if the instruction names that many.
    pub fn account(&self, index: usize) -> Option<&Pubkey> {
        self.accounts.get(index)
    }
}

/// Where updates come from.
///
/// A trait rather than a concrete stream so the pipeline can be driven by a scripted
/// sequence in tests. The alternative — testing the pipeline through a live endpoint —
/// tests the endpoint.
pub trait Source {
    /// The next update, or `None` when the source is exhausted.
    ///
    /// Ordering within a slot is the cluster's, and the pipeline does not assume
    /// transactions arrive before the account writes they caused.
    fn next(&mut self) -> impl Future<Output = Option<Update>> + Send;
}

/// A source that yields a fixed sequence, for tests.
pub struct Replay {
    updates: std::collections::VecDeque<Update>,
}

impl Replay {
    /// A source that will yield `updates` in order, then stop.
    pub fn new(updates: impl IntoIterator<Item = Update>) -> Self {
        Self {
            updates: updates.into_iter().collect(),
        }
    }

    /// How many updates are left.
    pub fn remaining(&self) -> usize {
        self.updates.len()
    }
}

impl Source for Replay {
    async fn next(&mut self) -> Option<Update> {
        self.updates.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(slot: u64) -> Update {
        Update::Account {
            slot,
            market: Pubkey::new_from_array([1u8; 32]),
            data: vec![0u8; 8],
            signature: None,
        }
    }

    #[tokio::test]
    async fn a_replay_yields_its_updates_in_order_then_stops() {
        let mut source = Replay::new([account(1), account(2)]);

        assert_eq!(source.next().await, Some(account(1)));
        assert_eq!(source.next().await, Some(account(2)));
        assert_eq!(source.next().await, None);
        assert_eq!(source.remaining(), 0);
    }
}
