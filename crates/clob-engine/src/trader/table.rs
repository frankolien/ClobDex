//! The seat table: trader identity to balances.
//!
//! # Why resting orders store a seat index
//!
//! A [`RestingOrder`](clob_book::RestingOrder) identifies its owner by a [`SeatIndex`],
//! which is the *arena handle* of that trader's node in this table. Two consequences:
//!
//! - Settling a fill is an O(1) array index, not an O(log n) tree descent by key. On a
//!   sweep through twenty price levels that is the difference between forty tree
//!   descents and forty array reads.
//! - A seat can only be released when it is completely empty, because releasing it frees
//!   the arena slot and any resting order still pointing at that index would silently
//!   re-target whoever claims the slot next. [`TraderTable::release_seat`] enforces this.
//!
//! Handles are stable across unrelated inserts and removals — a property the underlying
//! tree is explicitly tested for — so an index held across a match stays valid.

use bytemuck::{Pod, Zeroable};
use clob_book::{Handle, NIL, RedBlackTree};

use super::key::TraderKey;
use super::state::TraderState;
use crate::error::{EngineError, Result};

/// A seat's position in the trader table. Stable for the life of the seat.
pub type SeatIndex = Handle;

/// The sentinel meaning "no seat".
pub const NO_SEAT: SeatIndex = NIL;

/// Maps trader identities to their balances, with `SEATS` capacity.
#[repr(C)]
pub struct TraderTable<const SEATS: usize> {
    seats: RedBlackTree<TraderKey, TraderState, SEATS>,
}

impl<const SEATS: usize> Clone for TraderTable<SEATS> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<const SEATS: usize> Copy for TraderTable<SEATS> {}

// SAFETY: a newtype over a single Pod field, so layout and padding are the tree's.
unsafe impl<const SEATS: usize> Zeroable for TraderTable<SEATS> {}
unsafe impl<const SEATS: usize> Pod for TraderTable<SEATS> {}

impl<const SEATS: usize> Default for TraderTable<SEATS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SEATS: usize> TraderTable<SEATS> {
    /// Account space this table needs, in bytes.
    pub const SIZE_IN_BYTES: usize = core::mem::size_of::<Self>();

    /// An empty table.
    #[inline]
    pub fn new() -> Self {
        Self {
            seats: RedBlackTree::new(),
        }
    }

    /// Seats currently claimed.
    #[inline]
    pub fn len(&self) -> usize {
        self.seats.len()
    }

    /// Whether no seat is claimed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.seats.is_empty()
    }

    /// Whether the table is at capacity.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.seats.is_full()
    }

    /// Maximum seats.
    #[inline]
    pub const fn capacity(&self) -> usize {
        SEATS
    }

    /// Claims a seat for `key`, or returns the existing one.
    ///
    /// Idempotent, so a client can call it before every order without checking first.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatTableFull`] if the table is at capacity and `key` is new.
    pub fn claim_seat(&mut self, key: TraderKey) -> Result<SeatIndex> {
        let existing = self.seats.find(&key);
        if existing != NO_SEAT {
            return Ok(existing);
        }
        self.seats
            .insert(key, TraderState::default())
            .ok_or(EngineError::SeatTableFull)
    }

    /// Releases a seat, returning its slot to the table.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`] if `key` has no seat, or
    /// [`EngineError::SeatNotEmpty`] if it still holds funds — which, since placing an
    /// order locks funds, also covers "still has resting orders". Releasing a
    /// non-empty seat would strand its balance and leave resting orders pointing at a
    /// slot the next trader may claim.
    pub fn release_seat(&mut self, key: &TraderKey) -> Result<()> {
        let index = self.seats.find(key);
        if index == NO_SEAT {
            return Err(EngineError::SeatNotFound);
        }
        if !self.state(index)?.is_empty() {
            return Err(EngineError::SeatNotEmpty);
        }
        self.seats.remove_handle(index);
        Ok(())
    }

    /// The seat index for `key`, or [`NO_SEAT`].
    #[inline]
    pub fn index_of(&self, key: &TraderKey) -> SeatIndex {
        self.seats.find(key)
    }

    /// Balances at `index`.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`] if the index is [`NO_SEAT`].
    #[inline]
    pub fn state(&self, index: SeatIndex) -> Result<&TraderState> {
        self.seats
            .get_by_handle(index)
            .ok_or(EngineError::SeatNotFound)
    }

    /// Mutable balances at `index`.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`] if the index is [`NO_SEAT`].
    #[inline]
    pub fn state_mut(&mut self, index: SeatIndex) -> Result<&mut TraderState> {
        self.seats
            .get_mut_by_handle(index)
            .ok_or(EngineError::SeatNotFound)
    }

    /// Every claimed seat, ascending by key.
    pub fn iter(&self) -> impl Iterator<Item = (TraderKey, TraderState)> + '_ {
        self.seats.iter().map(|entry| (entry.key, entry.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clob_book::BaseLots;

    type Table = TraderTable<8>;

    fn key(byte: u8) -> TraderKey {
        TraderKey([byte; 32])
    }

    #[test]
    fn claiming_is_idempotent() {
        let mut table = Table::new();

        let first = table.claim_seat(key(1)).unwrap();
        let again = table.claim_seat(key(1)).unwrap();

        assert_eq!(first, again);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn distinct_traders_get_distinct_seats() {
        let mut table = Table::new();

        let a = table.claim_seat(key(1)).unwrap();
        let b = table.claim_seat(key(2)).unwrap();

        assert_ne!(a, b);
        assert_eq!(table.index_of(&key(1)), a);
        assert_eq!(table.index_of(&key(2)), b);
    }

    #[test]
    fn a_full_table_rejects_new_traders_but_still_serves_existing_ones() {
        let mut table = Table::new();
        for i in 0..8u8 {
            table.claim_seat(key(i)).unwrap();
        }

        assert_eq!(table.claim_seat(key(99)), Err(EngineError::SeatTableFull));
        assert!(table.claim_seat(key(0)).is_ok());
    }

    #[test]
    fn a_seat_holding_funds_cannot_be_released() {
        let mut table = Table::new();
        let index = table.claim_seat(key(1)).unwrap();
        table.state_mut(index).unwrap().base_lots_free = BaseLots(1);

        assert_eq!(table.release_seat(&key(1)), Err(EngineError::SeatNotEmpty));

        table.state_mut(index).unwrap().base_lots_free = BaseLots::ZERO;
        assert!(table.release_seat(&key(1)).is_ok());
        assert_eq!(table.index_of(&key(1)), NO_SEAT);
    }

    #[test]
    fn seat_indices_survive_unrelated_churn() {
        // Resting orders hold a seat index across matches, so this must hold.
        let mut table = Table::new();
        let pinned = table.claim_seat(key(0)).unwrap();

        for i in 1..7u8 {
            table.claim_seat(key(i)).unwrap();
        }
        for i in 1..7u8 {
            table.release_seat(&key(i)).unwrap();
        }

        assert_eq!(table.index_of(&key(0)), pinned);
    }

    #[test]
    fn an_unknown_index_is_reported_rather_than_panicking() {
        let table = Table::new();
        assert_eq!(table.state(NO_SEAT).err(), Some(EngineError::SeatNotFound));
    }
}
