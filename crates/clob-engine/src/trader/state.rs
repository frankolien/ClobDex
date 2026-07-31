//! Per-seat balances.
//!
//! Funds are split free/locked rather than tracked as a single balance plus a scan of
//! open orders. Locking at placement is what makes settlement crankless: by the time a
//! resting order is hit, the funds backing it are already committed, so a fill is a
//! balance transfer that cannot fail.

use bytemuck::{Pod, Zeroable};
use clob_book::{BaseLots, QuoteLots};

use crate::error::{EngineError, Result};

/// One seat's balances on a single market.
///
/// `locked` is funds committed to resting orders. The sum of all four fields across all
/// seats, plus unclaimed fees, is exactly what the market's vaults must hold — the
/// conservation invariant the whole engine is built to preserve.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TraderState {
    /// Base lots available to withdraw or commit to a new order.
    pub base_lots_free: BaseLots,
    /// Base lots committed to resting asks.
    pub base_lots_locked: BaseLots,
    /// Quote lots available to withdraw or commit to a new order.
    pub quote_lots_free: QuoteLots,
    /// Quote lots committed to resting bids.
    pub quote_lots_locked: QuoteLots,
}

// SAFETY: repr(C) over four 8-byte, 8-aligned Pod fields. Size 32, no padding.
unsafe impl Zeroable for TraderState {}
unsafe impl Pod for TraderState {}

impl TraderState {
    /// Total base lots this seat owns, resting or not.
    #[inline]
    pub fn total_base_lots(&self) -> BaseLots {
        self.base_lots_free.saturating_add(self.base_lots_locked)
    }

    /// Total quote lots this seat owns, resting or not.
    #[inline]
    pub fn total_quote_lots(&self) -> QuoteLots {
        self.quote_lots_free.saturating_add(self.quote_lots_locked)
    }

    /// Whether the seat holds nothing at all — the precondition for releasing it.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_base_lots().is_zero() && self.total_quote_lots().is_zero()
    }

    /// Moves base lots from free to locked, backing a new ask.
    pub fn lock_base(&mut self, amount: BaseLots) -> Result<()> {
        self.base_lots_free = self
            .base_lots_free
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        self.base_lots_locked = self
            .base_lots_locked
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Moves base lots from locked back to free, on cancel.
    pub fn unlock_base(&mut self, amount: BaseLots) -> Result<()> {
        self.base_lots_locked = self
            .base_lots_locked
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        self.base_lots_free = self
            .base_lots_free
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Moves quote lots from free to locked, backing a new bid.
    pub fn lock_quote(&mut self, amount: QuoteLots) -> Result<()> {
        self.quote_lots_free = self
            .quote_lots_free
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientQuoteFunds)?;
        self.quote_lots_locked = self
            .quote_lots_locked
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Moves quote lots from locked back to free, on cancel.
    pub fn unlock_quote(&mut self, amount: QuoteLots) -> Result<()> {
        self.quote_lots_locked = self
            .quote_lots_locked
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientQuoteFunds)?;
        self.quote_lots_free = self
            .quote_lots_free
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Adds to the free base balance.
    pub fn credit_base(&mut self, amount: BaseLots) -> Result<()> {
        self.base_lots_free = self
            .base_lots_free
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Adds to the free quote balance.
    pub fn credit_quote(&mut self, amount: QuoteLots) -> Result<()> {
        self.quote_lots_free = self
            .quote_lots_free
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }

    /// Removes from the free base balance.
    pub fn debit_base(&mut self, amount: BaseLots) -> Result<()> {
        self.base_lots_free = self
            .base_lots_free
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        Ok(())
    }

    /// Removes from the free quote balance.
    pub fn debit_quote(&mut self, amount: QuoteLots) -> Result<()> {
        self.quote_lots_free = self
            .quote_lots_free
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientQuoteFunds)?;
        Ok(())
    }

    /// Consumes locked base lots outright, as a maker ask does when it is hit.
    pub fn settle_locked_base(&mut self, amount: BaseLots) -> Result<()> {
        self.base_lots_locked = self
            .base_lots_locked
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        Ok(())
    }

    /// Consumes locked quote lots outright, as a maker bid does when it is hit.
    pub fn settle_locked_quote(&mut self, amount: QuoteLots) -> Result<()> {
        self.quote_lots_locked = self
            .quote_lots_locked
            .checked_sub(amount)
            .ok_or(EngineError::InsufficientQuoteFunds)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locking_conserves_the_total() {
        let mut state = TraderState {
            base_lots_free: BaseLots(100),
            ..Default::default()
        };
        let before = state.total_base_lots();

        state.lock_base(BaseLots(40)).unwrap();

        assert_eq!(state.base_lots_free, BaseLots(60));
        assert_eq!(state.base_lots_locked, BaseLots(40));
        assert_eq!(state.total_base_lots(), before);
    }

    #[test]
    fn locking_more_than_free_is_rejected_and_leaves_state_untouched() {
        let mut state = TraderState {
            base_lots_free: BaseLots(10),
            ..Default::default()
        };

        assert_eq!(
            state.lock_base(BaseLots(11)),
            Err(EngineError::InsufficientBaseFunds)
        );
        assert_eq!(state.base_lots_free, BaseLots(10));
        assert_eq!(state.base_lots_locked, BaseLots::ZERO);
    }

    #[test]
    fn unlock_reverses_lock_exactly() {
        let mut state = TraderState {
            quote_lots_free: QuoteLots(500),
            ..Default::default()
        };

        state.lock_quote(QuoteLots(200)).unwrap();
        state.unlock_quote(QuoteLots(200)).unwrap();

        assert_eq!(state.quote_lots_free, QuoteLots(500));
        assert_eq!(state.quote_lots_locked, QuoteLots::ZERO);
    }

    #[test]
    fn a_seat_is_empty_only_when_both_sides_are_clear() {
        assert!(TraderState::default().is_empty());
        assert!(
            !TraderState {
                base_lots_locked: BaseLots(1),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
