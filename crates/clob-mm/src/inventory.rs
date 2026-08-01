//! The position the bot did not choose, and what it does about it.
//!
//! A maker never decides what it holds. It offers both sides and the market picks, so the
//! position is whatever the flow happened to be — and flow is not symmetric when the price
//! is moving. Left alone, a maker in a falling market buys all the way down.
//!
//! Two mechanisms, and they are deliberately driven by one number:
//!
//! - **Skew** shifts the whole ladder toward the side that would unwind the position.
//!   Long base means quoting lower, which makes the bot's ask the attractive one. The
//!   position is still unwound by trading, and trading is still paid for by the spread.
//! - **The cap** stops quoting the side that would make it worse. Skew is a preference;
//!   this is a limit, and something has to be, because a preference does not bound a loss.
//!
//! Both scale off [`inventory_limit_lots`](crate::Params::inventory_limit_lots): skew
//! reaches its maximum exactly where the cap engages. Two separate knobs would let the
//! skew saturate somewhere the cap does not care about, which reads as a bot that leans
//! hard and then keeps trading anyway.
//!
//! Only base is steered. Quote is the other leg of the same trade, so a bounded base
//! position bounds both.

use clob_book::Side;
use clob_engine::TraderState;

use crate::Params;

/// A position, measured against where the bot wants it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    /// Base lots held, resting or free.
    pub base_lots: u64,
    /// Distance from target, in base lots. Positive is long.
    pub deviation_in_lots: i128,
}

impl Inventory {
    /// Measures a seat against the target.
    pub fn of(state: &TraderState, params: &Params) -> Self {
        let base_lots = state.total_base_lots().as_u64();
        Self {
            base_lots,
            // i128 so the subtraction is exact for every pair of u64s. A saturating i64
            // would report a smaller position than the bot actually holds, which is the
            // one direction this number must never be wrong in.
            deviation_in_lots: i128::from(base_lots) - i128::from(params.target_base_lots),
        }
    }

    /// Ticks to shift the whole ladder by. Negative when long.
    ///
    /// Proportional to the deviation and clamped at the limit, so a position twice as far
    /// out as the cap leans no harder than one exactly at it — past the cap the side that
    /// would deepen it is not quoting anyway, and leaning further would only push the
    /// other side away from the market that is trying to close it.
    pub fn skew_in_ticks(&self, params: &Params) -> i64 {
        let limit = i128::from(params.inventory_limit_lots).max(1);
        let capped = self.deviation_in_lots.clamp(-limit, limit);
        let max_skew = i128::from(params.max_skew_in_ticks);

        // Truncates toward zero, so a deviation too small to move a whole tick moves
        // nothing. Rounding away from zero would make the bot re-quote over positions it
        // cannot actually express.
        let skew = -(max_skew * capped) / limit;
        skew.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }

    /// Whether the bot may still quote `side`.
    ///
    /// A bid buys base and an ask sells it, so each side is refused at the end of the
    /// range it would push the position further into.
    pub fn may_quote(&self, side: Side, params: &Params) -> bool {
        let limit = i128::from(params.inventory_limit_lots);
        match side {
            Side::Bid => self.deviation_in_lots < limit,
            Side::Ask => self.deviation_in_lots > -limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clob_book::{BaseLots, QuoteLots};

    fn params() -> Params {
        Params {
            reference_in_ticks: 150_000,
            half_spread_in_ticks: 50,
            level_step_in_ticks: 25,
            levels: 3,
            size_in_base_lots: 100,
            target_base_lots: 1_000,
            inventory_limit_lots: 500,
            max_skew_in_ticks: 20,
            drift_tolerance_in_ticks: 10,
        }
    }

    /// A seat holding `free` base lots and nothing else.
    fn holding(free: u64) -> TraderState {
        TraderState {
            base_lots_free: BaseLots(free),
            base_lots_locked: BaseLots::ZERO,
            quote_lots_free: QuoteLots::ZERO,
            quote_lots_locked: QuoteLots::ZERO,
        }
    }

    #[test]
    fn a_position_at_target_leans_neither_way() {
        let inventory = Inventory::of(&holding(1_000), &params());
        assert_eq!(inventory.deviation_in_lots, 0);
        assert_eq!(inventory.skew_in_ticks(&params()), 0);
        assert!(inventory.may_quote(Side::Bid, &params()));
        assert!(inventory.may_quote(Side::Ask, &params()));
    }

    #[test]
    fn base_behind_a_resting_ask_still_counts_as_held() {
        // Locked base is committed, not gone: the ask can be cancelled and the lots come
        // straight back. Counting only free base would report a bot that had quoted its
        // whole position as flat, and it would go buy more.
        let state = TraderState {
            base_lots_free: BaseLots(400),
            base_lots_locked: BaseLots(600),
            quote_lots_free: QuoteLots::ZERO,
            quote_lots_locked: QuoteLots::ZERO,
        };
        assert_eq!(Inventory::of(&state, &params()).base_lots, 1_000);
        assert_eq!(Inventory::of(&state, &params()).deviation_in_lots, 0);
    }

    #[test]
    fn being_long_shifts_the_ladder_down() {
        // Long by half the limit, so half the skew. Down, because a lower ladder makes
        // the ask the side the market wants to hit.
        let inventory = Inventory::of(&holding(1_250), &params());
        assert_eq!(inventory.deviation_in_lots, 250);
        assert_eq!(inventory.skew_in_ticks(&params()), -10);
    }

    #[test]
    fn being_short_shifts_the_ladder_up() {
        let inventory = Inventory::of(&holding(750), &params());
        assert_eq!(inventory.deviation_in_lots, -250);
        assert_eq!(inventory.skew_in_ticks(&params()), 10);
    }

    #[test]
    fn the_lean_is_symmetric() {
        for offset in [1u64, 50, 137, 250, 499, 500] {
            let long = Inventory::of(&holding(1_000 + offset), &params());
            let short = Inventory::of(&holding(1_000 - offset), &params());
            assert_eq!(
                long.skew_in_ticks(&params()),
                -short.skew_in_ticks(&params()),
                "offset {offset}"
            );
        }
    }

    #[test]
    fn the_lean_stops_at_the_limit() {
        let at = Inventory::of(&holding(1_500), &params());
        let well_past = Inventory::of(&holding(1_000_000), &params());
        assert_eq!(at.skew_in_ticks(&params()), -20);
        assert_eq!(well_past.skew_in_ticks(&params()), -20);
    }

    #[test]
    fn the_lean_never_reaches_the_half_spread() {
        // The property the whole no-cross argument depends on, checked against the skew
        // actually produced rather than against the parameter it is derived from.
        let params = params();
        for held in [0u64, 1, 500, 999, 1_000, 1_001, 1_500, 5_000, u64::MAX] {
            let skew = Inventory::of(&holding(held), &params).skew_in_ticks(&params);
            assert!(
                skew.unsigned_abs() < params.half_spread_in_ticks,
                "holding {held} produced a skew of {skew}"
            );
        }
    }

    #[test]
    fn a_full_position_stops_the_side_that_would_grow_it() {
        let long = Inventory::of(&holding(1_500), &params());
        assert!(!long.may_quote(Side::Bid, &params()), "should not keep buying");
        assert!(long.may_quote(Side::Ask, &params()), "must still be able to sell");

        let short = Inventory::of(&holding(500), &params());
        assert!(short.may_quote(Side::Bid, &params()), "must still be able to buy");
        assert!(!short.may_quote(Side::Ask, &params()), "should not keep selling");
    }

    #[test]
    fn one_lot_inside_the_limit_still_quotes_both_sides() {
        // The cap engages at the limit, not before it: a bot that stopped early would
        // quote one side over a range it is entitled to quote both.
        let inventory = Inventory::of(&holding(1_499), &params());
        assert!(inventory.may_quote(Side::Bid, &params()));
        assert!(inventory.may_quote(Side::Ask, &params()));
    }

    #[test]
    fn an_empty_seat_at_a_zero_target_is_flat_and_may_quote_both_sides() {
        // Holding nothing while wanting nothing is at target, not short of it, so the cap
        // has no opinion about either side. That the seat cannot actually back an ask is
        // a different question — "should I?" is answered here, "can I?" is answered by
        // the budget in `ladder`, and conflating the two would silence a side for a
        // reason the position limit was never asked about.
        let params = Params {
            target_base_lots: 0,
            ..params()
        };
        let inventory = Inventory::of(&holding(0), &params);
        assert_eq!(inventory.skew_in_ticks(&params), 0);
        assert!(inventory.may_quote(Side::Bid, &params));
        assert!(inventory.may_quote(Side::Ask, &params));
    }

    #[test]
    fn an_enormous_position_does_not_overflow_the_skew() {
        // u64::MAX lots against a target of zero: the deviation is larger than an i64 and
        // the multiplication would overflow in anything narrower than i128.
        let params = Params {
            target_base_lots: 0,
            ..params()
        };
        let inventory = Inventory::of(&holding(u64::MAX), &params);
        assert_eq!(inventory.skew_in_ticks(&params), -20);
    }
}
