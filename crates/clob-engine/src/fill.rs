//! What a match produced, and how a caller observes it.

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Ticks};

use crate::trader::SeatIndex;

/// A single maker order being hit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    /// The resting order that was hit.
    pub maker_order_id: FIFOOrderId,
    /// Seat that owned the resting order.
    pub maker_seat: SeatIndex,
    /// Seat that took the liquidity.
    pub taker_seat: SeatIndex,
    /// Execution price. Always the *maker's* price — the taker's limit only decides
    /// whether the fill happens, never at what price, which is what makes resting a
    /// quote worth doing.
    pub price_in_ticks: Ticks,
    /// Size traded.
    pub base_lots_filled: BaseLots,
    /// Gross quote value, before fee.
    pub quote_lots_filled: QuoteLots,
    /// Fee taken from the taker on this fill.
    pub fee_in_quote_lots: QuoteLots,
    /// Size still resting on the maker order afterwards; zero if it was consumed.
    pub maker_base_lots_remaining: BaseLots,
}

/// Why matching stopped.
///
/// Returned rather than discarded because it is the difference between "the book is
/// thin" and "you set `match_limit` too low" — a distinction a market maker tuning its
/// parameters cannot otherwise make from outside.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MatchStop {
    /// The order was completely filled.
    FullyFilled,
    /// The next resting order was outside the limit price.
    PriceLimit,
    /// `match_limit` resting orders were consumed.
    MatchLimit,
    /// The opposite side ran out of liquidity.
    BookEmpty,
    /// The taker could not fund the next fill.
    InsufficientFunds,
    /// The order never attempted to take.
    DidNotCross,
}

/// The result of submitting an order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderOutcome {
    /// Id of the resting remainder, if any part of the order posted.
    pub order_id: Option<FIFOOrderId>,
    /// Total size taken from the book.
    pub base_lots_filled: BaseLots,
    /// Total gross quote value taken, before fee.
    pub quote_lots_filled: QuoteLots,
    /// Total fee charged to the taker.
    pub fee_in_quote_lots: QuoteLots,
    /// Size left resting on the book.
    pub base_lots_posted: BaseLots,
    /// Size removed by self-trade handling — cancelled or decremented, never traded.
    pub base_lots_self_traded: BaseLots,
    /// Why matching ended.
    pub stop: MatchStop,
}

impl OrderOutcome {
    /// An order that neither filled nor posted.
    pub const fn empty(stop: MatchStop) -> Self {
        Self {
            order_id: None,
            base_lots_filled: BaseLots::ZERO,
            quote_lots_filled: QuoteLots::ZERO,
            fee_in_quote_lots: QuoteLots::ZERO,
            base_lots_posted: BaseLots::ZERO,
            base_lots_self_traded: BaseLots::ZERO,
            stop,
        }
    }

    /// Whether any liquidity was taken.
    #[inline]
    pub fn did_fill(&self) -> bool {
        !self.base_lots_filled.is_zero()
    }

    /// Whether any size came to rest.
    #[inline]
    pub fn did_post(&self) -> bool {
        !self.base_lots_posted.is_zero()
    }
}

/// Receives each fill as it happens.
///
/// The engine reports fills through this rather than returning a list because it has no
/// allocator: on-chain the implementation emits a log or a self-CPI event, and in tests
/// it pushes to a `Vec`. `()` implements it as a no-op, so callers that only want the
/// aggregate totals pay nothing.
pub trait FillObserver {
    /// Called once per maker order consumed, in execution order.
    fn on_fill(&mut self, fill: &Fill);
}

impl FillObserver for () {
    #[inline(always)]
    fn on_fill(&mut self, _fill: &Fill) {}
}

impl<T: FillObserver + ?Sized> FillObserver for &mut T {
    #[inline(always)]
    fn on_fill(&mut self, fill: &Fill) {
        (**self).on_fill(fill);
    }
}

#[cfg(feature = "std")]
impl FillObserver for std::vec::Vec<Fill> {
    fn on_fill(&mut self, fill: &Fill) {
        self.push(*fill);
    }
}
