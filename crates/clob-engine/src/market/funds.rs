//! Moving value in and out of the market.
//!
//! Every operation here computes all of its results before writing any of them. A
//! partially applied deposit would break conservation permanently, and there is no
//! transaction to roll back inside a single instruction.

use clob_book::{BaseLots, QuoteLots};

use super::Market;
use crate::error::{EngineError, Result};
use crate::trader::SeatIndex;

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    /// Credits a seat's free balances.
    ///
    /// The caller is responsible for having actually moved the tokens; this only records
    /// them. On-chain the token transfer and this call are in the same instruction, so
    /// they succeed or fail together.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`], or [`EngineError::Overflow`] if either running
    /// total would exceed `u64`.
    pub fn deposit(&mut self, seat: SeatIndex, base: BaseLots, quote: QuoteLots) -> Result<()> {
        let new_base_deposited = self
            .header
            .base_lots_deposited
            .checked_add(base)
            .ok_or(EngineError::Overflow)?;
        let new_quote_deposited = self
            .header
            .quote_lots_deposited
            .checked_add(quote)
            .ok_or(EngineError::Overflow)?;

        let state = self.traders.state_mut(seat)?;
        let new_base_free = state
            .base_lots_free
            .checked_add(base)
            .ok_or(EngineError::Overflow)?;
        let new_quote_free = state
            .quote_lots_free
            .checked_add(quote)
            .ok_or(EngineError::Overflow)?;

        state.base_lots_free = new_base_free;
        state.quote_lots_free = new_quote_free;
        self.header.base_lots_deposited = new_base_deposited;
        self.header.quote_lots_deposited = new_quote_deposited;
        Ok(())
    }

    /// Debits a seat's free balances.
    ///
    /// Locked funds are untouchable: withdrawing them would leave a resting order with
    /// nothing behind it. Cancel first.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`], or [`EngineError::InsufficientBaseFunds`] /
    /// [`EngineError::InsufficientQuoteFunds`] if the free balance does not cover it.
    pub fn withdraw(&mut self, seat: SeatIndex, base: BaseLots, quote: QuoteLots) -> Result<()> {
        let state = self.traders.state(seat)?;
        let new_base_free = state
            .base_lots_free
            .checked_sub(base)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        let new_quote_free = state
            .quote_lots_free
            .checked_sub(quote)
            .ok_or(EngineError::InsufficientQuoteFunds)?;

        let new_base_deposited = self
            .header
            .base_lots_deposited
            .checked_sub(base)
            .ok_or(EngineError::InsufficientBaseFunds)?;
        let new_quote_deposited = self
            .header
            .quote_lots_deposited
            .checked_sub(quote)
            .ok_or(EngineError::InsufficientQuoteFunds)?;

        let state = self.traders.state_mut(seat)?;
        state.base_lots_free = new_base_free;
        state.quote_lots_free = new_quote_free;
        self.header.base_lots_deposited = new_base_deposited;
        self.header.quote_lots_deposited = new_quote_deposited;
        Ok(())
    }

    /// Withdraws everything a seat holds free, and reports what moved.
    ///
    /// # Errors
    ///
    /// [`EngineError::SeatNotFound`].
    pub fn withdraw_all(&mut self, seat: SeatIndex) -> Result<(BaseLots, QuoteLots)> {
        let state = *self.traders.state(seat)?;
        let (base, quote) = (state.base_lots_free, state.quote_lots_free);
        self.withdraw(seat, base, quote)?;
        Ok((base, quote))
    }

    /// Sweeps accrued fees out of the market, returning the amount.
    ///
    /// Fees sit inside `quote_lots_deposited` until collected, because until then they
    /// are still quote tokens the market's vault is holding.
    ///
    /// # Errors
    ///
    /// [`EngineError::Overflow`] only if the running totals are already inconsistent.
    pub fn collect_fees(&mut self) -> Result<QuoteLots> {
        let amount = self.header.unclaimed_quote_lot_fees;
        let new_quote_deposited = self
            .header
            .quote_lots_deposited
            .checked_sub(amount)
            .ok_or(EngineError::Overflow)?;

        self.header.unclaimed_quote_lot_fees = QuoteLots::ZERO;
        self.header.quote_lots_deposited = new_quote_deposited;
        Ok(amount)
    }

    /// Records a fee earned on a fill.
    pub(super) fn accrue_fee(&mut self, amount: QuoteLots) -> Result<()> {
        self.header.unclaimed_quote_lot_fees = self
            .header
            .unclaimed_quote_lot_fees
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        self.header.collected_quote_lot_fees = self
            .header
            .collected_quote_lot_fees
            .checked_add(amount)
            .ok_or(EngineError::Overflow)?;
        Ok(())
    }
}
