//! The mutable half of a resting order.

use bytemuck::{Pod, Zeroable};

use crate::quantities::BaseLots;

/// A resting order's mutable state.
///
/// Carries neither price nor side — those live in the [`FIFOOrderId`] key. Since nothing
/// here participates in the ordering, a partial fill can shrink an order in place
/// without touching tree structure.
///
/// `trader_index` is a seat index rather than a `Pubkey`: 8 bytes instead of 32, and it
/// makes "cancel everything for this trader" an integer comparison during a tree walk.
///
/// [`FIFOOrderId`]: crate::order::FIFOOrderId
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RestingOrder {
    /// Index of the owning trader's seat in the market's trader table.
    pub trader_index: u64,
    /// Remaining unfilled size. An order at zero is removed rather than left resting.
    pub num_base_lots: BaseLots,
    /// Last slot at which this order is valid; `0` means no slot expiry.
    pub last_valid_slot: u64,
    /// Last Unix timestamp (seconds) at which this order is valid; `0` means no
    /// timestamp expiry.
    pub last_valid_unix_timestamp_in_seconds: u64,
}

// SAFETY: `repr(C)` over four 8-byte, 8-aligned `Pod` fields. Size 32, no padding.
unsafe impl Zeroable for RestingOrder {}
unsafe impl Pod for RestingOrder {}

impl RestingOrder {
    /// A good-till-cancelled order.
    #[inline(always)]
    pub const fn new(trader_index: u64, num_base_lots: BaseLots) -> Self {
        Self {
            trader_index,
            num_base_lots,
            last_valid_slot: 0,
            last_valid_unix_timestamp_in_seconds: 0,
        }
    }

    /// Whether this order has expired.
    ///
    /// The two bounds are independent, and a zero bound means "no expiry on this
    /// dimension" — so an order with both bounds zero never expires.
    #[inline]
    pub const fn is_expired(&self, slot: u64, unix_timestamp_in_seconds: u64) -> bool {
        (self.last_valid_slot != 0 && slot > self.last_valid_slot)
            || (self.last_valid_unix_timestamp_in_seconds != 0
                && unix_timestamp_in_seconds > self.last_valid_unix_timestamp_in_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bounds_mean_good_till_cancelled() {
        assert!(!RestingOrder::new(0, BaseLots(10)).is_expired(u64::MAX, u64::MAX));
    }

    #[test]
    fn expiry_bounds_are_independent() {
        let by_slot = RestingOrder {
            last_valid_slot: 100,
            ..RestingOrder::new(0, BaseLots(10))
        };
        assert!(!by_slot.is_expired(100, u64::MAX));
        assert!(by_slot.is_expired(101, 0));

        let by_time = RestingOrder {
            last_valid_unix_timestamp_in_seconds: 100,
            ..RestingOrder::new(0, BaseLots(10))
        };
        assert!(!by_time.is_expired(u64::MAX, 100));
        assert!(by_time.is_expired(0, 101));
    }
}
