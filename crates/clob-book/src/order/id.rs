//! Order identity, and the encoding that makes price-time priority fall out of a
//! derived `Ord`.
//!
//! Both sides want "best price, then oldest", but *best* is opposite: asks want the
//! lowest price, bids the highest. Storing both in ascending `(price, sequence)` order
//! gets asks right and bids wrong twice — wrong on price, and wrong on time, since at
//! the best bid price the maximum entry is the newest order rather than the oldest.
//!
//! The fix is to store bids with a bitwise-inverted sequence number. Under one
//! ascending `(price, stored_sequence)` order:
//!
//! - Best ask = tree **minimum**: lowest price, smallest sequence number within it.
//! - Best bid = tree **maximum**: highest price, largest `!sequence` within it — which
//!   is the *smallest* `sequence`, so again the oldest order.
//!
//! One `#[derive(Ord)]` therefore gives exact price-time priority on both sides. No
//! side-aware comparator, no second code path.
//!
//! Side comes free: sequence numbers count up from zero, so a real one has its high bit
//! clear and `!sequence` has it set. That bit is an exact side tag. It holds while fewer
//! than `2^63` orders are placed on a market — ~292,000 years at a million per second —
//! so the encoding treats it as an invariant rather than a runtime error.

use bytemuck::{Pod, Zeroable};

use super::side::Side;
use crate::quantities::Ticks;

/// Highest sequence number that does not alias the side tag.
const MAX_SEQUENCE_NUMBER: u64 = (1 << 63) - 1;

/// The identity of a resting order: price plus a sequence number encoding both arrival
/// order and side.
///
/// Field order is load-bearing. The derived [`Ord`] compares `price_in_ticks` first and
/// `order_sequence_number` second which — with the bid-side inversion described in the
/// [module docs](self) — *is* price-time priority. Reordering these fields silently
/// breaks matching.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FIFOOrderId {
    /// Limit price, in ticks. Compared first, ascending.
    pub price_in_ticks: Ticks,
    /// The *stored* sequence number: the raw counter for asks, its complement for bids.
    /// Use [`FIFOOrderId::sequence_number`] and [`FIFOOrderId::side`] to decode.
    pub order_sequence_number: u64,
}

// SAFETY: `repr(C)` over `Ticks` (transparent `u64`) and `u64`. Size 16, align 8, no
// padding, all bit patterns valid.
unsafe impl Zeroable for FIFOOrderId {}
unsafe impl Pod for FIFOOrderId {}

impl FIFOOrderId {
    /// Builds an order ID, applying the side encoding to `sequence_number`.
    ///
    /// `sequence_number` is the market's raw counter value and must not exceed
    /// `2^63 - 1`; debug builds assert this.
    #[inline(always)]
    pub const fn new(side: Side, price_in_ticks: Ticks, sequence_number: u64) -> Self {
        debug_assert!(
            sequence_number <= MAX_SEQUENCE_NUMBER,
            "sequence number would alias the side tag"
        );
        Self {
            price_in_ticks,
            order_sequence_number: match side {
                Side::Bid => !sequence_number,
                Side::Ask => sequence_number,
            },
        }
    }

    /// Rebuilds an ID from an already-encoded sequence number, as sent by a client or
    /// read back from account bytes.
    #[inline(always)]
    pub const fn from_encoded(price_in_ticks: Ticks, order_sequence_number: u64) -> Self {
        Self {
            price_in_ticks,
            order_sequence_number,
        }
    }

    /// The side this order rests on, read from the high bit.
    #[inline(always)]
    pub const fn side(&self) -> Side {
        if self.order_sequence_number >> 63 == 1 {
            Side::Bid
        } else {
            Side::Ask
        }
    }

    /// The raw counter value, with the encoding undone.
    ///
    /// [`crate::OrderBook`] issues both sides from one counter, so raw sequence numbers
    /// are unique across a whole market, not just within a side.
    #[inline(always)]
    pub const fn sequence_number(&self) -> u64 {
        match self.side() {
            Side::Bid => !self.order_sequence_number,
            Side::Ask => self.order_sequence_number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_and_sequence_round_trip() {
        for sequence in [0u64, 1, 12_345, MAX_SEQUENCE_NUMBER] {
            for side in [Side::Bid, Side::Ask] {
                let id = FIFOOrderId::new(side, Ticks(100), sequence);
                assert_eq!(id.side(), side);
                assert_eq!(id.sequence_number(), sequence);
            }
        }
    }

    #[test]
    fn opposite_sides_never_share_a_stored_sequence() {
        let bid = FIFOOrderId::new(Side::Bid, Ticks(100), 7);
        let ask = FIFOOrderId::new(Side::Ask, Ticks(100), 7);
        assert_ne!(bid.order_sequence_number, ask.order_sequence_number);
    }

    #[test]
    fn ascending_order_prioritises_asks_correctly() {
        // Best ask = minimum: lowest price, oldest first within a price.
        let mut ids = [
            FIFOOrderId::new(Side::Ask, Ticks(101), 1),
            FIFOOrderId::new(Side::Ask, Ticks(100), 9),
            FIFOOrderId::new(Side::Ask, Ticks(100), 4),
        ];
        ids.sort();
        assert_eq!(ids[0].price_in_ticks, Ticks(100));
        assert_eq!(ids[0].sequence_number(), 4);
        assert_eq!(ids[1].sequence_number(), 9);
        assert_eq!(ids[2].price_in_ticks, Ticks(101));
    }

    #[test]
    fn ascending_order_prioritises_bids_correctly() {
        // Best bid = maximum: highest price, oldest first within a price.
        let mut ids = [
            FIFOOrderId::new(Side::Bid, Ticks(99), 1),
            FIFOOrderId::new(Side::Bid, Ticks(100), 9),
            FIFOOrderId::new(Side::Bid, Ticks(100), 4),
        ];
        ids.sort();
        assert_eq!(ids[2].price_in_ticks, Ticks(100));
        assert_eq!(ids[2].sequence_number(), 4);
        assert_eq!(ids[1].sequence_number(), 9);
        assert_eq!(ids[0].price_in_ticks, Ticks(99));
    }
}
