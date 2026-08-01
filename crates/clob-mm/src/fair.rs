//! What the bot thinks the asset is worth.
//!
//! # Why not just take the midpoint
//!
//! On a market where the bot is the only maker, the midpoint of the book *is* the bot's
//! own ladder. Quoting around it makes every cycle a fixed point of the last one, and any
//! rounding at all sends the whole ladder walking in one direction — a bot bidding against
//! itself, at prices nobody else ever named.
//!
//! So the touch is read with the bot's own orders removed. What remains is the part of
//! the book that is genuinely other people's opinion, which is the only part worth
//! pricing against.
//!
//! # When the book cannot answer
//!
//! With both sides present the midpoint is the answer. With one side present it is not:
//! a resting bid at 100 says fair is *at least* 100, which is a bound and not a price. So
//! the reference is used, clamped to whatever the visible side bounds it to.
//!
//! That clamp is not decoration. Quoting an ask below somebody's resting bid is quoting a
//! price that is already crossed — the post-only order is rejected and the refresh fails,
//! every cycle, for as long as the reference stays stale. Taking the visible side as a
//! bound means a stale reference costs accuracy instead of liveness.

use clob_client::state::MarketState;

/// The best price on each side of a book, excluding one trader's own orders.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    /// Highest bid somebody else is showing.
    pub bid_in_ticks: Option<u64>,
    /// Lowest ask somebody else is showing.
    pub ask_in_ticks: Option<u64>,
}

impl Touch {
    /// Reads the touch of `state`, ignoring orders owned by `seat`.
    ///
    /// `seat` is `None` before the bot has claimed one, at which point nothing on the
    /// book is its own and the whole book counts.
    pub fn excluding(state: &MarketState, seat: Option<u32>) -> Self {
        // Both sides are stored best-first, so the first order that is not ours is the
        // best price that is not ours.
        let theirs = |order: &&clob_client::state::BookOrder| Some(order.trader_index) != seat;
        Self {
            bid_in_ticks: state
                .bids
                .iter()
                .find(theirs)
                .map(|order| order.price_in_ticks().as_u64()),
            ask_in_ticks: state
                .asks
                .iter()
                .find(theirs)
                .map(|order| order.price_in_ticks().as_u64()),
        }
    }
}

/// Where a fair price came from.
///
/// Worth reporting rather than inferring: a bot running on `Reference` is quoting its own
/// opinion into an empty book, and one running on `Midpoint` is following a market. Those
/// are very different things to be doing, and the price alone does not say which.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Halfway between two other traders' quotes.
    Midpoint,
    /// The configured reference, with nothing in the book to correct it.
    Reference,
    /// The reference, raised to somebody's resting bid.
    RaisedToBid,
    /// The reference, lowered to somebody's resting ask.
    LoweredToAsk,
}

/// A price to quote around, and where it came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fair {
    /// The price, in ticks.
    pub price_in_ticks: u64,
    /// How it was arrived at.
    pub source: Source,
}

/// Prices the market from what other traders are showing, falling back to `reference`.
///
/// Never returns zero: a tick of zero is not a price, and a ladder centred on it would
/// have no bid side at all.
pub fn price(touch: &Touch, reference_in_ticks: u64) -> Fair {
    let (price_in_ticks, source) = match (touch.bid_in_ticks, touch.ask_in_ticks) {
        // Rounds down on an odd spread. The half-tick lost is smaller than the tick the
        // quote is placed on, so nothing downstream can see it.
        (Some(bid), Some(ask)) => ((bid + ask) / 2, Source::Midpoint),
        (Some(bid), None) if reference_in_ticks < bid => (bid, Source::RaisedToBid),
        (None, Some(ask)) if reference_in_ticks > ask => (ask, Source::LoweredToAsk),
        _ => (reference_in_ticks, Source::Reference),
    };

    Fair {
        price_in_ticks: price_in_ticks.max(1),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clob_book::{BaseLots, FIFOOrderId, Side, Ticks};
    use clob_client::state::BookOrder;

    fn touch(bid: Option<u64>, ask: Option<u64>) -> Touch {
        Touch {
            bid_in_ticks: bid,
            ask_in_ticks: ask,
        }
    }

    #[test]
    fn two_sided_interest_prices_at_the_midpoint() {
        let fair = price(&touch(Some(100), Some(110)), 1_000);
        assert_eq!(fair.price_in_ticks, 105);
        assert_eq!(fair.source, Source::Midpoint);
    }

    #[test]
    fn a_two_sided_book_overrides_the_reference_completely() {
        // The reference is an opinion; two live quotes are a market. Being far wrong does
        // not earn the reference a vote.
        for reference in [1, 50, 105, 1_000_000] {
            assert_eq!(price(&touch(Some(100), Some(110)), reference).price_in_ticks, 105);
        }
    }

    #[test]
    fn an_odd_spread_rounds_down() {
        assert_eq!(price(&touch(Some(100), Some(101)), 500).price_in_ticks, 100);
    }

    #[test]
    fn an_empty_book_leaves_the_reference_alone() {
        let fair = price(&touch(None, None), 150_000);
        assert_eq!(fair.price_in_ticks, 150_000);
        assert_eq!(fair.source, Source::Reference);
    }

    #[test]
    fn a_resting_bid_is_a_floor_under_a_stale_reference() {
        // Somebody is willing to pay 100 and we thought it was worth 90. They know
        // something; quoting an ask at 90 would simply be giving it to them.
        let fair = price(&touch(Some(100), None), 90);
        assert_eq!(fair.price_in_ticks, 100);
        assert_eq!(fair.source, Source::RaisedToBid);
    }

    #[test]
    fn a_resting_ask_is_a_ceiling_over_a_stale_reference() {
        let fair = price(&touch(None, Some(100)), 130);
        assert_eq!(fair.price_in_ticks, 100);
        assert_eq!(fair.source, Source::LoweredToAsk);
    }

    #[test]
    fn a_bound_that_does_not_bind_leaves_the_reference_alone() {
        // A bid at 100 says fair is at least 100. It says nothing about 150.
        let fair = price(&touch(Some(100), None), 150);
        assert_eq!(fair.price_in_ticks, 150);
        assert_eq!(fair.source, Source::Reference);
    }

    #[test]
    fn fair_is_never_zero() {
        // A zero centre has no bid side: every bid price would be negative and get
        // dropped, leaving the bot quoting one side of a market forever.
        assert_eq!(price(&touch(Some(0), Some(0)), 1).price_in_ticks, 1);
        assert_eq!(price(&touch(None, Some(0)), 5).price_in_ticks, 1);
    }

    // -----------------------------------------------------------------------------
    // Reading the touch off a book
    // -----------------------------------------------------------------------------

    fn order(side: Side, price_in_ticks: u64, seat: u32) -> BookOrder {
        BookOrder {
            id: FIFOOrderId::new(side, Ticks(price_in_ticks), 1),
            trader_index: seat,
            num_base_lots: BaseLots(10),
        }
    }

    /// A book with the given orders. Only the sides matter here, so the rest is zeroed.
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
    fn our_own_orders_do_not_count_as_a_market() {
        // Seat 7 is the bot. Its ladder is the entire book, so there is no outside
        // opinion at all and the touch is empty on both sides.
        let state = book(
            vec![order(Side::Bid, 100, 7), order(Side::Bid, 99, 7)],
            vec![order(Side::Ask, 110, 7)],
        );
        assert_eq!(Touch::excluding(&state, Some(7)), touch(None, None));
    }

    #[test]
    fn the_best_price_that_is_not_ours_is_the_one_that_counts() {
        // Our bid at 100 is better than theirs at 98. Pricing against our own 100 would
        // be pricing against ourselves, so 98 is the number that matters.
        let state = book(
            vec![order(Side::Bid, 100, 7), order(Side::Bid, 98, 3)],
            vec![order(Side::Ask, 110, 7), order(Side::Ask, 115, 3)],
        );
        assert_eq!(Touch::excluding(&state, Some(7)), touch(Some(98), Some(115)));
    }

    #[test]
    fn without_a_seat_the_whole_book_is_somebody_elses() {
        let state = book(vec![order(Side::Bid, 100, 7)], vec![order(Side::Ask, 110, 7)]);
        assert_eq!(Touch::excluding(&state, None), touch(Some(100), Some(110)));
    }

    #[test]
    fn a_market_the_bot_is_alone_in_prices_off_its_reference() {
        // The end-to-end version of the fixed point this module exists to prevent: with
        // its own quotes excluded there is nothing left, so fair stays where it was put
        // instead of drifting toward whatever the bot last did.
        let state = book(
            vec![order(Side::Bid, 149_950, 7)],
            vec![order(Side::Ask, 150_050, 7)],
        );
        let fair = price(&Touch::excluding(&state, Some(7)), 150_000);
        assert_eq!(fair.price_in_ticks, 150_000);
        assert_eq!(fair.source, Source::Reference);
    }
}
