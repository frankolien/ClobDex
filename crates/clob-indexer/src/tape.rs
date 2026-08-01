//! What an observer records.

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};

/// One maker order consumed by a taker.
///
/// Prices come from the order id rather than from anywhere else, which is what makes
/// this exact rather than estimated: an order's price is part of its identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    /// Slot the transaction landed in.
    pub slot: u64,
    /// Execution price — always the maker's.
    pub price_in_ticks: Ticks,
    /// Size traded.
    pub base_lots: BaseLots,
    /// Gross quote value, before fee.
    pub quote_lots: QuoteLots,
    /// The resting order that was hit.
    pub maker_order_id: FIFOOrderId,
    /// Seat that owned it.
    pub maker_seat: u32,
    /// Seat that crossed it, when the transaction names exactly one.
    ///
    /// `None` when the taker's seat could not be resolved, and when several takers hit
    /// the same side in one transaction — a diff shows liquidity leaving, not which of
    /// them took it, and there is no way to tell them apart without replaying.
    ///
    /// Naming the first one would be a guess, and this is the field a trader's own fill
    /// history is built from: a wrong name there puts someone else's trade in your
    /// history, which is worse than an unattributed one.
    pub taker_seat: Option<u32>,
    /// Side the taker was on. A taker on the bid consumed asks.
    pub taker_side: Side,
}

/// Liquidity that left the book without trading.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    /// The order affected.
    pub order_id: FIFOOrderId,
    /// Seat that owned it.
    pub seat: u32,
    /// Size removed. Less than the resting size for a partial reduce.
    pub base_lots: BaseLots,
    /// Why it went.
    pub reason: RemovalReason,
}

/// Why liquidity left the book.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RemovalReason {
    /// The owner cancelled or reduced it.
    Cancelled,
    /// It was removed by its own owner's incoming order under a self-trade policy. No
    /// value changed hands, so it is not a trade — recording it as one would inflate
    /// reported volume, which is the number a venue is most tempted to inflate.
    SelfTraded,
}

/// Liquidity that appeared.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    /// The new order.
    pub order_id: FIFOOrderId,
    /// Seat that placed it.
    pub seat: u32,
    /// Resting size.
    pub base_lots: BaseLots,
}

/// Everything one transaction did to a market.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BookDelta {
    /// Fills, in book order: best price first.
    pub trades: Vec<Trade>,
    /// Liquidity removed without trading.
    pub removals: Vec<Removal>,
    /// Liquidity added.
    pub posted: Vec<Posted>,
    /// Fees the market accrued, from its own running total rather than from the trades.
    /// Comparing the two is how [`BookDelta::fees_reconcile`] catches a bad derivation.
    pub fees_earned: QuoteLots,
}

impl BookDelta {
    /// Whether anything happened.
    pub fn is_empty(&self) -> bool {
        self.trades.is_empty() && self.removals.is_empty() && self.posted.is_empty()
    }

    /// Total size traded.
    pub fn base_lots_traded(&self) -> BaseLots {
        self.trades
            .iter()
            .fold(BaseLots::ZERO, |sum, t| sum.saturating_add(t.base_lots))
    }

    /// Total gross quote value traded.
    pub fn quote_lots_traded(&self) -> QuoteLots {
        self.trades
            .iter()
            .fold(QuoteLots::ZERO, |sum, t| sum.saturating_add(t.quote_lots))
    }

    /// Whether the fee the market recorded matches what the derived trades imply.
    ///
    /// A free self-audit, and a sharp one: the market's fee counter is written by the
    /// program, and the trades here are derived independently from the book. If the
    /// derivation invented, missed or mispriced a fill, the two stop agreeing.
    ///
    /// Always true on a zero-fee market, where there is nothing to check against.
    pub fn fees_reconcile(&self, taker_fee_bps: u64) -> bool {
        let expected = clob_engine::FeeSchedule { taker_fee_bps }
            .fee_on(self.quote_lots_traded())
            .unwrap_or(QuoteLots::ZERO);

        // Fees are charged per fill and each is rounded up, so summing per-trade is not
        // the same as charging on the total. The gap is at most one lot per trade.
        let per_trade = self.trades.iter().fold(QuoteLots::ZERO, |sum, t| {
            let fee = clob_engine::FeeSchedule { taker_fee_bps }
                .fee_on(t.quote_lots)
                .unwrap_or(QuoteLots::ZERO);
            sum.saturating_add(fee)
        });

        self.fees_earned == per_trade || self.fees_earned == expected
    }
}
