//! Whether this cycle is worth a transaction.
//!
//! The bot recomputes its ladder every cycle, and most of the time the answer is the one
//! already on the book. Sending it anyway costs a signature and a slot to say nothing.
//!
//! A maker's edge is the spread it collects. Fees spent chasing a tick it did not need to
//! chase come out of that edge, so the tolerance is not a performance tweak — it is the
//! difference between a strategy that pays for itself and one that pays the network to
//! watch it think.
//!
//! # What forces a refresh
//!
//! Three things, and the first two are not optional:
//!
//! - A quote that is **gone**. It filled, so the bot is one level short of the liquidity
//!   it means to be showing.
//! - A quote that **shrank**. Partly filled, same argument.
//! - A quote that has **drifted** further than the tolerance from where it now belongs.
//!
//! # All or nothing
//!
//! A refresh cancels everything and re-places everything, rather than repairing the
//! levels that moved. Cancelling only some of it would leave the survivors' capital
//! locked at prices the new ladder has to be built around, and the budget in
//! [`ladder`](crate::ladder) assumes the opposite — that the whole balance is free,
//! because `BatchUpdate` cancels before it places. Two rules about the same capital, one
//! of which is wrong.

use clob_book::{BaseLots, FIFOOrderId, Side, Ticks};
use clob_client::state::MarketState;

use crate::ladder::Quote;

/// One of the bot's resting orders.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Resting {
    /// What a cancel names it by.
    pub id: FIFOOrderId,
    /// Which side.
    pub side: Side,
    /// Price, in ticks.
    pub price_in_ticks: Ticks,
    /// Size still resting.
    pub base_lots: BaseLots,
}

impl Resting {
    /// The orders `seat` owns, in the order [`ladder::build`](crate::ladder::build)
    /// emits: bids best-first, then asks best-first.
    ///
    /// Sharing an order with the ladder is what makes the comparison positional. Both
    /// sides of the book are stored best-first on chain, so this is a filter and not a
    /// sort.
    pub fn owned_by(state: &MarketState, seat: u32) -> Vec<Self> {
        let mine = |side: Side| {
            state
                .side(side)
                .iter()
                .filter(move |order| order.trader_index == seat)
                .map(move |order| Self {
                    id: order.id,
                    side,
                    price_in_ticks: order.price_in_ticks(),
                    base_lots: order.num_base_lots,
                })
        };
        mine(Side::Bid).chain(mine(Side::Ask)).collect()
    }
}

/// Why the ladder has to be re-sent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The resting quotes no longer line up with the ladder, level for level. Usually a
    /// quote that filled completely and is no longer there.
    Missing,
    /// A resting quote is no longer the size it should be. Usually a partial fill.
    Resized,
    /// A resting quote is too far from where it now belongs.
    Drifted {
        /// How far, in ticks.
        by_ticks: u64,
    },
}

/// What to do this cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The book already says what the bot wants it to say. Send nothing.
    Hold,
    /// Cancel all of it, place all of it, in one instruction.
    Replace {
        /// Why.
        reason: Reason,
        /// Every order the bot currently has resting.
        cancels: Vec<FIFOOrderId>,
        /// The ladder it wants instead.
        places: Vec<Quote>,
    },
}

impl Plan {
    /// Whether this cycle sends a transaction.
    pub fn sends(&self) -> bool {
        matches!(self, Self::Replace { .. })
    }
}

/// Decides whether what is resting is close enough to what is wanted.
pub fn decide(resting: &[Resting], desired: &[Quote], tolerance_in_ticks: u64) -> Plan {
    let replace = |reason| Plan::Replace {
        reason,
        cancels: resting.iter().map(|order| order.id).collect(),
        places: desired.to_vec(),
    };

    if resting.len() != desired.len() {
        return replace(Reason::Missing);
    }

    for (resting, desired) in resting.iter().zip(desired) {
        // A side mismatch at the same position means the two ladders disagree about
        // shape, not price — one side lost a level and the other gained one, which the
        // length check cannot see.
        if resting.side != desired.side {
            return replace(Reason::Missing);
        }
        if resting.base_lots != desired.base_lots {
            return replace(Reason::Resized);
        }
        let drift = resting
            .price_in_ticks
            .as_u64()
            .abs_diff(desired.price_in_ticks.as_u64());
        if drift > tolerance_in_ticks {
            return replace(Reason::Drifted { by_ticks: drift });
        }
    }

    Plan::Hold
}

#[cfg(test)]
mod tests {
    use super::*;
    use clob_client::state::BookOrder;

    fn quote(side: Side, price: u64, size: u64) -> Quote {
        Quote {
            side,
            price_in_ticks: Ticks(price),
            base_lots: BaseLots(size),
        }
    }

    /// A resting order matching `quote`, with a sequence number nothing here depends on.
    fn resting(quote: &Quote) -> Resting {
        Resting {
            id: FIFOOrderId::new(quote.side, quote.price_in_ticks, 1),
            side: quote.side,
            price_in_ticks: quote.price_in_ticks,
            base_lots: quote.base_lots,
        }
    }

    /// A two-level ladder, tight around 150,000.
    fn ladder() -> Vec<Quote> {
        vec![
            quote(Side::Bid, 149_950, 100),
            quote(Side::Bid, 149_925, 100),
            quote(Side::Ask, 150_050, 100),
            quote(Side::Ask, 150_075, 100),
        ]
    }

    fn on_book(quotes: &[Quote]) -> Vec<Resting> {
        quotes.iter().map(resting).collect()
    }

    #[test]
    fn a_ladder_already_on_the_book_is_left_alone() {
        assert_eq!(decide(&on_book(&ladder()), &ladder(), 10), Plan::Hold);
    }

    #[test]
    fn quoting_nothing_when_nothing_is_resting_sends_nothing() {
        // The state a capped-out bot reaches after its cancel lands. Without this it
        // would re-send an empty refresh every cycle forever.
        assert_eq!(decide(&[], &[], 10), Plan::Hold);
    }

    #[test]
    fn a_filled_quote_forces_a_refresh() {
        let mut book = on_book(&ladder());
        book.remove(0);
        let plan = decide(&book, &ladder(), 10);

        assert!(matches!(plan, Plan::Replace { reason: Reason::Missing, .. }));
    }

    #[test]
    fn a_partial_fill_forces_a_refresh() {
        // Still resting, just smaller. Leaving it would quietly halve the depth the bot
        // believes it is showing.
        let mut book = on_book(&ladder());
        book[0].base_lots = BaseLots(40);
        let plan = decide(&book, &ladder(), 10);

        assert!(matches!(plan, Plan::Replace { reason: Reason::Resized, .. }));
    }

    #[test]
    fn drift_inside_the_tolerance_is_not_worth_a_transaction() {
        let mut book = on_book(&ladder());
        book[0].price_in_ticks = Ticks(149_940);
        assert_eq!(decide(&book, &ladder(), 10), Plan::Hold);
    }

    #[test]
    fn drift_past_the_tolerance_is() {
        let mut book = on_book(&ladder());
        book[0].price_in_ticks = Ticks(149_939);
        let plan = decide(&book, &ladder(), 10);

        assert!(matches!(
            plan,
            Plan::Replace {
                reason: Reason::Drifted { by_ticks: 11 },
                ..
            }
        ));
    }

    #[test]
    fn the_tolerance_is_a_distance_not_a_direction() {
        for moved in [149_939u64, 149_961] {
            let mut book = on_book(&ladder());
            book[0].price_in_ticks = Ticks(moved);
            assert!(decide(&book, &ladder(), 10).sends(), "price {moved}");
        }
    }

    #[test]
    fn a_zero_tolerance_refreshes_on_a_single_tick() {
        let mut book = on_book(&ladder());
        book[0].price_in_ticks = Ticks(149_951);
        assert!(decide(&book, &ladder(), 0).sends());
    }

    #[test]
    fn a_refresh_cancels_everything_and_places_everything() {
        // Not just the level that moved. The budget in `ladder` assumes the whole balance
        // is available, which is true only because the batch cancels before it places.
        let mut book = on_book(&ladder());
        book[3].price_in_ticks = Ticks(151_000);

        match decide(&book, &ladder(), 10) {
            Plan::Replace { cancels, places, .. } => {
                assert_eq!(cancels.len(), 4);
                assert_eq!(places, ladder());
            }
            Plan::Hold => panic!("a level moved 925 ticks"),
        }
    }

    #[test]
    fn a_bot_that_wants_to_quote_nothing_cancels_what_it_has() {
        // What the position cap looks like from here: the ladder came back empty, so the
        // plan is a cancel with nothing to place.
        match decide(&on_book(&ladder()), &[], 10) {
            Plan::Replace { cancels, places, reason } => {
                assert_eq!(reason, Reason::Missing);
                assert_eq!(cancels.len(), 4);
                assert!(places.is_empty());
            }
            Plan::Hold => panic!("four orders are resting and none are wanted"),
        }
    }

    #[test]
    fn one_side_losing_a_level_while_the_other_gains_one_is_still_a_refresh() {
        // The count matches, so only the side comparison can see it. Zipping two ladders
        // that disagree about shape would otherwise compare a bid against an ask and call
        // the difference drift.
        let book = on_book(&[
            quote(Side::Bid, 149_950, 100),
            quote(Side::Ask, 150_050, 100),
            quote(Side::Ask, 150_075, 100),
            quote(Side::Ask, 150_100, 100),
        ]);
        let plan = decide(&book, &ladder(), 10);

        assert!(matches!(plan, Plan::Replace { reason: Reason::Missing, .. }));
    }

    // ---------------------------------------------------------------------------------
    // Reading the bot's own orders off a book
    // ---------------------------------------------------------------------------------

    fn order(side: Side, price: u64, seat: u32, sequence: u64) -> BookOrder {
        BookOrder {
            id: FIFOOrderId::new(side, Ticks(price), sequence),
            trader_index: seat,
            num_base_lots: BaseLots(100),
        }
    }

    fn book(bids: Vec<BookOrder>, asks: Vec<BookOrder>) -> MarketState {
        MarketState {
            account: bytemuck::Zeroable::zeroed(),
            size_class: clob_program::state::SizeClass::Small,
            header: Default::default(),
            bids,
            asks,
            traders: Vec::new(),
        }
    }

    #[test]
    fn only_our_own_orders_are_ours_to_cancel() {
        let state = book(
            vec![order(Side::Bid, 149_950, 3, 1), order(Side::Bid, 149_925, 7, 2)],
            vec![order(Side::Ask, 150_050, 7, 3), order(Side::Ask, 150_075, 3, 4)],
        );
        let ours = Resting::owned_by(&state, 7);

        assert_eq!(ours.len(), 2);
        assert_eq!(ours[0].price_in_ticks, Ticks(149_925));
        assert_eq!(ours[1].price_in_ticks, Ticks(150_050));
    }

    #[test]
    fn resting_orders_come_back_in_the_order_the_ladder_builds_them() {
        // Bids first, then asks, each best-first — the positional contract `decide`
        // depends on. A mismatch here would read as constant drift and re-quote forever.
        let state = book(
            vec![order(Side::Bid, 149_950, 7, 1), order(Side::Bid, 149_925, 7, 2)],
            vec![order(Side::Ask, 150_050, 7, 3), order(Side::Ask, 150_075, 7, 4)],
        );
        let ours = Resting::owned_by(&state, 7);

        assert_eq!(
            ours.iter().map(|o| o.price_in_ticks.as_u64()).collect::<Vec<_>>(),
            vec![149_950, 149_925, 150_050, 150_075]
        );
        assert!(ours[..2].iter().all(|o| o.side == Side::Bid));
        assert!(ours[2..].iter().all(|o| o.side == Side::Ask));
    }

    #[test]
    fn a_book_the_bot_has_no_orders_on_yields_nothing_to_cancel() {
        let state = book(vec![order(Side::Bid, 149_950, 3, 1)], Vec::new());
        assert!(Resting::owned_by(&state, 7).is_empty());
    }
}
