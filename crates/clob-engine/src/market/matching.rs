//! The cross loop: consuming resting liquidity and settling each fill atomically.
//!
//! # Crankless settlement
//!
//! Every fill moves value between the two seats' balances *inside the taker's
//! transaction*. There is no event queue and no second step, so a maker's proceeds are
//! spendable the moment the taker's transaction confirms.
//!
//! What makes that possible is that maker funds were locked at placement. By the time a
//! resting order is hit, the value backing it is already committed, so settlement is a
//! transfer between two balances that cannot fail for lack of funds — no rollback path
//! is needed, and none exists mid-instruction.
//!
//! # Fills happen at the maker's price
//!
//! The taker's limit decides *whether* a fill happens, never at what price. That is the
//! entire economic reason to rest a quote: a maker who quoted 100 gets 100 even when a
//! taker was willing to pay 105. Any other rule makes posting strictly worse than
//! taking.

use clob_book::{BaseLots, QuoteLots, Side, Ticks};

use super::Market;
use crate::error::{EngineError, Result};
use crate::fill::{Fill, FillObserver, MatchStop};
use crate::order::SelfTradeBehavior;
use crate::trader::SeatIndex;

/// Running totals for one pass through the book.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct CrossOutcome {
    pub remaining: BaseLots,
    pub base_lots_filled: BaseLots,
    pub quote_lots_filled: QuoteLots,
    pub fee_in_quote_lots: QuoteLots,
    pub base_lots_self_traded: BaseLots,
    pub stop: MatchStop,
}

impl<const BIDS: usize, const ASKS: usize, const SEATS: usize> Market<BIDS, ASKS, SEATS> {
    /// Whether a taker on `side` with limit `limit` will accept `maker_price`.
    ///
    /// `None` means no limit, i.e. a market order.
    #[inline]
    fn accepts(side: Side, maker_price: Ticks, limit: Option<Ticks>) -> bool {
        match limit {
            None => true,
            Some(limit) => match side {
                // Buying: pay no more than the limit.
                Side::Bid => maker_price <= limit,
                // Selling: receive no less than the limit.
                Side::Ask => maker_price >= limit,
            },
        }
    }

    /// Walks the opposite side of the book, filling until the order is satisfied or a
    /// stop condition trips.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cross<O: FillObserver>(
        &mut self,
        taker: SeatIndex,
        side: Side,
        limit: Option<Ticks>,
        size: BaseLots,
        self_trade_behavior: SelfTradeBehavior,
        match_limit: u32,
        observer: &mut O,
    ) -> Result<CrossOutcome> {
        let mut remaining = size;
        let mut orders_touched = 0u32;
        let mut base_lots_filled = BaseLots::ZERO;
        let mut quote_lots_filled = QuoteLots::ZERO;
        let mut fee_in_quote_lots = QuoteLots::ZERO;
        let mut base_lots_self_traded = BaseLots::ZERO;

        let stop = loop {
            if remaining.is_zero() {
                break MatchStop::FullyFilled;
            }
            if orders_touched >= match_limit {
                break MatchStop::MatchLimit;
            }

            let Some(entry) = self.book.best(side.opposite()) else {
                break MatchStop::BookEmpty;
            };
            let maker_price = entry.key.price_in_ticks;
            if !Self::accepts(side, maker_price, limit) {
                break MatchStop::PriceLimit;
            }

            // Counted before the self-trade branches too: cancelling or decrementing
            // one's own quotes costs compute, so it has to be bounded the same way.
            orders_touched += 1;

            let maker_seat = entry.value.trader_index as SeatIndex;
            let maker_size = entry.value.num_base_lots;

            if maker_seat == taker {
                let removed = self.resolve_self_trade(
                    &entry.key,
                    maker_seat,
                    maker_size,
                    remaining,
                    self_trade_behavior,
                )?;
                // CancelProvide removes the whole maker order without consuming taker
                // size, so only DecrementTake reduces `remaining`.
                if self_trade_behavior == SelfTradeBehavior::DecrementTake {
                    remaining -= removed;
                }
                base_lots_self_traded = base_lots_self_traded
                    .checked_add(removed)
                    .ok_or(EngineError::Overflow)?;
                continue;
            }

            let fill_size = remaining.min(maker_size);
            let gross = self.quote_value(maker_price, fill_size)?;
            let fee = self.header.fees.fee_on(gross)?;

            if !self.taker_can_afford(taker, side, fill_size, gross, fee)? {
                break MatchStop::InsufficientFunds;
            }

            self.settle_fill(taker, maker_seat, side, fill_size, gross, fee)?;

            let maker_remaining = maker_size - fill_size;
            if maker_remaining.is_zero() {
                self.book.cancel(&entry.key);
            } else {
                self.book
                    .get_mut(&entry.key)
                    .ok_or(EngineError::OrderNotFound)?
                    .num_base_lots = maker_remaining;
            }

            observer.on_fill(&Fill {
                maker_order_id: entry.key,
                maker_seat,
                taker_seat: taker,
                price_in_ticks: maker_price,
                base_lots_filled: fill_size,
                quote_lots_filled: gross,
                fee_in_quote_lots: fee,
                maker_base_lots_remaining: maker_remaining,
            });

            remaining -= fill_size;
            base_lots_filled = base_lots_filled
                .checked_add(fill_size)
                .ok_or(EngineError::Overflow)?;
            quote_lots_filled = quote_lots_filled
                .checked_add(gross)
                .ok_or(EngineError::Overflow)?;
            fee_in_quote_lots = fee_in_quote_lots
                .checked_add(fee)
                .ok_or(EngineError::Overflow)?;
        };

        Ok(CrossOutcome {
            remaining,
            base_lots_filled,
            quote_lots_filled,
            fee_in_quote_lots,
            base_lots_self_traded,
            stop,
        })
    }

    /// Applies the caller's self-trade policy, returning the size removed from the book.
    fn resolve_self_trade(
        &mut self,
        maker_order_id: &clob_book::FIFOOrderId,
        seat: SeatIndex,
        maker_size: BaseLots,
        taker_remaining: BaseLots,
        behavior: SelfTradeBehavior,
    ) -> Result<BaseLots> {
        match behavior {
            SelfTradeBehavior::Abort => Err(EngineError::SelfTradeAborted),
            SelfTradeBehavior::CancelProvide => {
                self.unlock_backing(maker_order_id, seat, maker_size)?;
                self.book.cancel(maker_order_id);
                Ok(maker_size)
            }
            SelfTradeBehavior::DecrementTake => {
                let decrement = taker_remaining.min(maker_size);
                self.unlock_backing(maker_order_id, seat, decrement)?;
                let remaining = maker_size - decrement;
                if remaining.is_zero() {
                    self.book.cancel(maker_order_id);
                } else {
                    self.book
                        .get_mut(maker_order_id)
                        .ok_or(EngineError::OrderNotFound)?
                        .num_base_lots = remaining;
                }
                Ok(decrement)
            }
        }
    }

    /// Whether the taker's *free* balance covers this fill.
    ///
    /// Underfunded takers stop matching rather than getting a clamped partial fill.
    /// Clamping would make the executed size depend on a fee rounding, which is a poor
    /// property for a venue: an aggregator cannot predict it, and a maker cannot explain
    /// it. Stopping is predictable, and a caller who wants the largest affordable order
    /// can size it before submitting.
    fn taker_can_afford(
        &self,
        taker: SeatIndex,
        side: Side,
        fill_size: BaseLots,
        gross: QuoteLots,
        fee: QuoteLots,
    ) -> Result<bool> {
        let state = self.traders.state(taker)?;
        Ok(match side {
            Side::Bid => {
                let Some(cost) = gross.checked_add(fee) else {
                    return Ok(false);
                };
                state.quote_lots_free >= cost
            }
            Side::Ask => state.base_lots_free >= fill_size,
        })
    }

    /// Moves value between the two seats and accrues the fee.
    ///
    /// Both states are read out as copies, mutated, then written back, because the seat
    /// table cannot hand out two mutable references at once. Self-trades are resolved
    /// before this point, so the two seats are always distinct.
    fn settle_fill(
        &mut self,
        taker: SeatIndex,
        maker: SeatIndex,
        side: Side,
        fill_size: BaseLots,
        gross: QuoteLots,
        fee: QuoteLots,
    ) -> Result<()> {
        debug_assert_ne!(taker, maker, "self-trades must be resolved before settlement");

        let mut maker_state = *self.traders.state(maker)?;
        let mut taker_state = *self.traders.state(taker)?;

        match side {
            // Taker buys: the maker's locked base becomes the taker's, the taker's quote
            // becomes the maker's, and the fee is skimmed on top of what the maker gets.
            Side::Bid => {
                let cost = gross.checked_add(fee).ok_or(EngineError::Overflow)?;
                maker_state.settle_locked_base(fill_size)?;
                maker_state.credit_quote(gross)?;
                taker_state.debit_quote(cost)?;
                taker_state.credit_base(fill_size)?;
            }
            // Taker sells: the maker's locked quote becomes the taker's, less the fee,
            // and the taker's base becomes the maker's.
            Side::Ask => {
                let proceeds = gross.checked_sub(fee).ok_or(EngineError::Overflow)?;
                maker_state.settle_locked_quote(gross)?;
                maker_state.credit_base(fill_size)?;
                taker_state.debit_base(fill_size)?;
                taker_state.credit_quote(proceeds)?;
            }
        }

        *self.traders.state_mut(maker)? = maker_state;
        *self.traders.state_mut(taker)? = taker_state;
        self.accrue_fee(fee)
    }
}
