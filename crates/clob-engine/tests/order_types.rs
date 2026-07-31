//! Post-only, immediate-or-cancel, fill-or-kill, and the guards around all of them.

mod common;

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_engine::{EngineError, MatchStop, OrderPacket, PostOnlyRejection, SelfTradeBehavior};

use common::*;

fn post_only(side: Side, price: u64, size: u64, rejection: PostOnlyRejection) -> OrderPacket {
    OrderPacket::PostOnly {
        side,
        price_in_ticks: Ticks(price),
        num_base_lots: BaseLots(size),
        rejection,
    }
}

#[test]
fn a_post_only_order_that_does_not_cross_rests_at_its_own_price() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Ask, 105, 5);

    let outcome = market
        .place_order(maker, post_only(Side::Bid, 100, 5, PostOnlyRejection::Reject), &mut ())
        .unwrap();

    assert_eq!(outcome.base_lots_posted, BaseLots(5));
    assert_eq!(outcome.stop, MatchStop::DidNotCross);
    assert_eq!(prices(&market, Side::Bid), vec![100]);
}

#[test]
fn a_crossing_post_only_order_is_rejected_by_default() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Ask, 100, 5);

    assert_eq!(
        market.place_order(maker, post_only(Side::Bid, 105, 5, PostOnlyRejection::Reject), &mut ()),
        Err(EngineError::PostOnlyWouldCross)
    );
    // Nothing rested and nothing was taken: the resting ask is untouched.
    assert!(market.book().is_empty(Side::Bid));
    assert_eq!(market.book().best_ask().unwrap().value.num_base_lots, BaseLots(5));
}

#[test]
fn a_crossing_post_only_order_slides_just_inside_the_touch() {
    // Saves a market maker a round trip when the book moved between quoting and landing.
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Ask, 100, 5);

    let outcome = market
        .place_order(maker, post_only(Side::Bid, 105, 5, PostOnlyRejection::Slide), &mut ())
        .unwrap();

    assert_eq!(outcome.order_id.unwrap().price_in_ticks, Ticks(99));
    assert_eq!(prices(&market, Side::Bid), vec![99]);
    // Funds are locked at the slid price, not the requested one.
    assert_eq!(locked(&market, maker).1, 99 * 5);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn an_ask_slides_upward_past_the_best_bid() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Bid, 100, 5);

    let outcome = market
        .place_order(maker, post_only(Side::Ask, 95, 5, PostOnlyRejection::Slide), &mut ())
        .unwrap();

    assert_eq!(outcome.order_id.unwrap().price_in_ticks, Ticks(101));
}

#[test]
fn a_bid_with_nowhere_to_slide_is_rejected() {
    // The best ask is one tick, so the only non-crossing bid price would be zero.
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Ask, 1, 5);

    assert_eq!(
        market.place_order(maker, post_only(Side::Bid, 5, 5, PostOnlyRejection::Slide), &mut ()),
        Err(EngineError::PostOnlyNoRoom)
    );
}

#[test]
fn a_post_only_order_never_takes_even_when_it_could() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    rest(&mut market, other, Side::Ask, 100, 5);

    market
        .place_order(maker, post_only(Side::Bid, 105, 5, PostOnlyRejection::Slide), &mut ())
        .unwrap();

    assert_eq!(market.header().collected_quote_lot_fees, QuoteLots::ZERO);
    assert_eq!(market.book().best_ask().unwrap().value.num_base_lots, BaseLots(5));
}

#[test]
fn an_ioc_order_discards_whatever_it_could_not_fill() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 3);

    let outcome = market
        .place_order(taker, OrderPacket::market(Side::Bid, BaseLots(10)), &mut ())
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(3));
    assert_eq!(outcome.base_lots_posted, BaseLots::ZERO);
    assert_eq!(outcome.order_id, None);
    assert!(market.book().is_empty(Side::Bid));
}

#[test]
fn fill_or_kill_succeeds_when_the_book_is_deep_enough() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 5);
    rest(&mut market, maker, Side::Ask, 101, 5);

    let outcome = market
        .place_order(
            taker,
            OrderPacket::fill_or_kill(Side::Bid, Ticks(101), BaseLots(10)),
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(10));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn fill_or_kill_reports_a_shortfall_rather_than_filling_partially() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 3);

    assert_eq!(
        market.place_order(
            taker,
            OrderPacket::fill_or_kill(Side::Bid, Ticks(100), BaseLots(10)),
            &mut ()
        ),
        Err(EngineError::MinimumFillNotMet)
    );
}

#[test]
fn a_failed_minimum_fill_relies_on_the_caller_discarding_the_market() {
    // Pinning documented behaviour rather than asserting it is desirable: the minimum
    // is checked *after* matching, so the error leaves the fills applied. On Solana a
    // returned error reverts the whole instruction, which is exactly the all-or-nothing
    // semantic this order type promises. Anywhere else, the caller must throw the
    // mutated market away -- see `Market::place_order`.
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    // Starts with no base, so any base it holds afterwards came from the failed order.
    let taker = seat_with(&mut market, 2, 0, 100_000);
    rest(&mut market, maker, Side::Ask, 100, 3);

    let err = market
        .place_order(
            taker,
            OrderPacket::fill_or_kill(Side::Bid, Ticks(100), BaseLots(10)),
            &mut (),
        )
        .unwrap_err();
    assert_eq!(err, EngineError::MinimumFillNotMet);

    // The partial fill really did land, and value is still conserved -- the state is
    // inconsistent with the caller's intent, not with the market's books.
    assert_eq!(free(&market, taker).0, 3);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_partial_minimum_is_honoured_when_met() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 6);

    let outcome = market
        .place_order(
            taker,
            OrderPacket::ImmediateOrCancel {
                side: Side::Bid,
                price_in_ticks: None,
                num_base_lots: BaseLots(10),
                min_base_lots_to_fill: BaseLots(5),
                self_trade_behavior: SelfTradeBehavior::DecrementTake,
                match_limit: u32::MAX,
            },
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(6));
}

#[test]
fn a_zero_size_order_is_rejected_before_anything_happens() {
    let mut market = market();
    let seat = funded_seat(&mut market, 1);

    assert_eq!(
        market.place_order(seat, OrderPacket::limit(Side::Bid, Ticks(100), BaseLots::ZERO), &mut ()),
        Err(EngineError::ZeroSize)
    );
}

#[test]
fn an_unknown_seat_cannot_trade() {
    let mut market = market();

    assert_eq!(
        market.place_order(
            clob_engine::NO_SEAT,
            OrderPacket::limit(Side::Bid, Ticks(100), BaseLots(1)),
            &mut ()
        ),
        Err(EngineError::SeatNotFound)
    );
}

#[test]
fn an_unfunded_maker_cannot_post() {
    let mut market = market();
    let seat = seat_with(&mut market, 1, 0, 50);

    // 5 lots at 100 ticks costs 500 quote lots; the seat has 50.
    assert_eq!(
        market.place_order(seat, post_only(Side::Bid, 100, 5, PostOnlyRejection::Reject), &mut ()),
        Err(EngineError::InsufficientQuoteFunds)
    );
    assert!(market.book().is_empty(Side::Bid));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn locked_funds_cannot_be_withdrawn() {
    let mut market = market();
    let seat = seat_with(&mut market, 1, 10, 0);
    rest(&mut market, seat, Side::Ask, 100, 10);

    assert_eq!(
        market.withdraw(seat, BaseLots(1), QuoteLots::ZERO),
        Err(EngineError::InsufficientBaseFunds)
    );
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_full_book_side_leaves_the_seat_exactly_as_it_was() {
    let mut market = TestMarket::new_boxed(lot_config(), clob_engine::FeeSchedule::FREE).unwrap();
    let seat = funded_seat(&mut market, 1);
    for price in 1..=32u64 {
        rest(&mut market, seat, Side::Bid, price, 1);
    }
    let before = (free(&market, seat), locked(&market, seat));

    assert_eq!(
        market.place_order(seat, post_only(Side::Bid, 50, 1, PostOnlyRejection::Reject), &mut ()),
        Err(EngineError::BookSideFull)
    );

    // The lock taken in anticipation of posting must be released on failure.
    assert_eq!((free(&market, seat), locked(&market, seat)), before);
    assert_eq!(market.check_conservation(), Ok(()));
}
