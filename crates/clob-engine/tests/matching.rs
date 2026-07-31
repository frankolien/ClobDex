//! How the cross loop behaves: execution price, priority, and every stop condition.

mod common;

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_engine::{EngineError, MatchStop, OrderPacket, SelfTradeBehavior};

use common::*;

#[test]
fn fills_execute_at_the_makers_price_not_the_takers() {
    // The economic core of resting a quote: a maker who asked 100 gets 100 even when
    // the taker would have paid 110. Any other rule makes posting strictly worse than
    // taking, and the book empties out.
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 10);

    let (outcome, fills) = place_recording(
        &mut market,
        taker,
        OrderPacket::limit(Side::Bid, Ticks(110), BaseLots(10)),
    );
    let outcome = outcome.unwrap();

    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].price_in_ticks, Ticks(100));
    assert_eq!(outcome.quote_lots_filled, QuoteLots(1_000));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn liquidity_is_consumed_in_price_then_time_order() {
    let mut market = market();
    let maker_a = funded_seat(&mut market, 1);
    let maker_b = funded_seat(&mut market, 2);
    let taker = funded_seat(&mut market, 3);

    // Two asks at 101 with A first, and a better one at 100 posted last.
    let second_at_101 = rest(&mut market, maker_a, Side::Ask, 101, 5);
    let _third_at_101 = rest(&mut market, maker_b, Side::Ask, 101, 5);
    let best = rest(&mut market, maker_b, Side::Ask, 100, 5);
    let _ = second_at_101;

    let (_, fills) = place_recording(
        &mut market,
        taker,
        OrderPacket::limit(Side::Bid, Ticks(101), BaseLots(15)),
    );

    let order: Vec<_> = fills.iter().map(|f| (f.price_in_ticks.as_u64(), f.maker_seat)).collect();
    assert_eq!(order, vec![(100, maker_b), (101, maker_a), (101, maker_b)]);
    assert_eq!(fills[0].maker_order_id, best);
}

#[test]
fn a_partial_fill_leaves_the_maker_at_the_front_of_the_queue() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let other = funded_seat(&mut market, 2);
    let taker = funded_seat(&mut market, 3);

    let first = rest(&mut market, maker, Side::Ask, 100, 10);
    let second = rest(&mut market, other, Side::Ask, 100, 10);

    let (_, fills) = place_recording(
        &mut market,
        taker,
        OrderPacket::market(Side::Bid, BaseLots(4)),
    );

    assert_eq!(fills[0].maker_base_lots_remaining, BaseLots(6));
    // Still ahead of the order that arrived after it.
    let queue: Vec<_> = market.book().iter_side(Side::Ask).map(|e| e.key).collect();
    assert_eq!(queue, vec![first, second]);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_sweep_walks_down_the_book_until_the_size_is_met() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    for (price, size) in [(100u64, 5u64), (101, 5), (102, 5)] {
        rest(&mut market, maker, Side::Ask, price, size);
    }

    let (outcome, fills) = place_recording(
        &mut market,
        taker,
        OrderPacket::market(Side::Bid, BaseLots(12)),
    );
    let outcome = outcome.unwrap();

    assert_eq!(fills.len(), 3);
    assert_eq!(outcome.base_lots_filled, BaseLots(12));
    // 5 at 100, 5 at 101, 2 at 102.
    assert_eq!(outcome.quote_lots_filled, QuoteLots(500 + 505 + 204));
    assert_eq!(outcome.stop, MatchStop::FullyFilled);
    assert_eq!(prices(&market, Side::Ask), vec![102]);
}

#[test]
fn a_limit_order_rests_whatever_it_could_not_fill() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 4);

    let outcome = market
        .place_order(
            taker,
            OrderPacket::limit(Side::Bid, Ticks(100), BaseLots(10)),
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(4));
    assert_eq!(outcome.base_lots_posted, BaseLots(6));
    assert_eq!(market.book().best_bid().unwrap().key, outcome.order_id.unwrap());
    // The posted remainder is backed by locked quote, not free quote.
    assert_eq!(locked(&market, taker).1, 600);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn matching_stops_at_the_limit_price() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    rest(&mut market, maker, Side::Ask, 100, 5);
    rest(&mut market, maker, Side::Ask, 105, 5);

    let outcome = market
        .place_order(
            taker,
            OrderPacket::ImmediateOrCancel {
                side: Side::Bid,
                price_in_ticks: Some(Ticks(102)),
                num_base_lots: BaseLots(10),
                min_base_lots_to_fill: BaseLots::ZERO,
                self_trade_behavior: SelfTradeBehavior::DecrementTake,
                match_limit: u32::MAX,
            },
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(5));
    assert_eq!(outcome.stop, MatchStop::PriceLimit);
    assert_eq!(prices(&market, Side::Ask), vec![105]);
}

#[test]
fn match_limit_bounds_the_walk() {
    // The reason this exists: an unbounded sweep can exceed the compute budget and
    // revert, costing the taker its fee for nothing.
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let taker = funded_seat(&mut market, 2);
    for price in 100..110u64 {
        rest(&mut market, maker, Side::Ask, price, 1);
    }

    let outcome = market
        .place_order(
            taker,
            OrderPacket::ImmediateOrCancel {
                side: Side::Bid,
                price_in_ticks: None,
                num_base_lots: BaseLots(10),
                min_base_lots_to_fill: BaseLots::ZERO,
                self_trade_behavior: SelfTradeBehavior::DecrementTake,
                match_limit: 3,
            },
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(3));
    assert_eq!(outcome.stop, MatchStop::MatchLimit);
}

#[test]
fn an_empty_book_reports_why_nothing_happened() {
    let mut market = market();
    let taker = funded_seat(&mut market, 1);

    let outcome = market
        .place_order(taker, OrderPacket::market(Side::Bid, BaseLots(5)), &mut ())
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots::ZERO);
    assert_eq!(outcome.stop, MatchStop::BookEmpty);
}

#[test]
fn an_underfunded_taker_stops_rather_than_filling_what_it_cannot_pay_for() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    // Enough quote for one lot at 100 (100 quote lots), not two.
    let taker = seat_with(&mut market, 2, 0, 150);
    rest(&mut market, maker, Side::Ask, 100, 1);
    rest(&mut market, maker, Side::Ask, 101, 1);

    let outcome = market
        .place_order(taker, OrderPacket::market(Side::Bid, BaseLots(2)), &mut ())
        .unwrap();

    assert_eq!(outcome.base_lots_filled, BaseLots(1));
    assert_eq!(outcome.stop, MatchStop::InsufficientFunds);
    assert_eq!(free(&market, taker), (1, 50));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_taker_selling_receives_the_proceeds_less_the_fee() {
    // 10 bps on a 1_000 quote lot fill.
    let mut market = market_with_fee(10);
    let maker = funded_seat(&mut market, 1);
    let taker = seat_with(&mut market, 2, 10, 0);
    rest(&mut market, maker, Side::Bid, 100, 10);

    let outcome = market
        .place_order(taker, OrderPacket::market(Side::Ask, BaseLots(10)), &mut ())
        .unwrap();

    assert_eq!(outcome.quote_lots_filled, QuoteLots(1_000));
    assert_eq!(outcome.fee_in_quote_lots, QuoteLots(1));
    assert_eq!(free(&market, taker), (0, 999));
    // The maker pays nothing.
    assert_eq!(free(&market, maker).1, 1_000_000_000 - 1_000);
    assert_eq!(market.header().unclaimed_quote_lot_fees, QuoteLots(1));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_taker_buying_pays_the_fee_on_top_of_the_fill() {
    let mut market = market_with_fee(10);
    let maker = funded_seat(&mut market, 1);
    let taker = seat_with(&mut market, 2, 0, 5_000);
    rest(&mut market, maker, Side::Ask, 100, 10);

    market
        .place_order(taker, OrderPacket::market(Side::Bid, BaseLots(10)), &mut ())
        .unwrap();

    // 1_000 to the maker, 1 to the venue.
    assert_eq!(free(&market, taker), (10, 5_000 - 1_001));
    assert_eq!(free(&market, maker).1, 1_000_000_000 + 1_000);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn cancelling_returns_exactly_what_was_locked() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let before = free(&market, maker);

    let id = rest(&mut market, maker, Side::Bid, 100, 7);
    assert_eq!(locked(&market, maker).1, 700);

    let cancelled = market.cancel_order(maker, &id).unwrap();

    assert_eq!(cancelled, BaseLots(7));
    assert_eq!(free(&market, maker), before);
    assert_eq!(locked(&market, maker), (0, 0));
}

#[test]
fn only_the_owner_can_cancel_or_reduce() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let stranger = funded_seat(&mut market, 2);
    let id = rest(&mut market, maker, Side::Ask, 100, 5);

    assert_eq!(
        market.cancel_order(stranger, &id),
        Err(EngineError::NotOrderOwner)
    );
    assert_eq!(
        market.reduce_order(stranger, &id, BaseLots(1)),
        Err(EngineError::NotOrderOwner)
    );
    assert!(market.book().contains(&id));
}

#[test]
fn reducing_releases_funds_proportionally_and_keeps_priority() {
    let mut market = market();
    let maker = funded_seat(&mut market, 1);
    let id = rest(&mut market, maker, Side::Bid, 100, 10);
    assert_eq!(locked(&market, maker).1, 1_000);

    let removed = market.reduce_order(maker, &id, BaseLots(4)).unwrap();

    assert_eq!(removed, BaseLots(4));
    assert_eq!(locked(&market, maker).1, 600);
    assert_eq!(market.book().best_bid().unwrap().key, id);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn cancel_all_for_a_seat_leaves_other_traders_alone() {
    let mut market = market();
    let mine = funded_seat(&mut market, 1);
    let theirs = funded_seat(&mut market, 2);
    for price in 100..104u64 {
        rest(&mut market, mine, Side::Bid, price, 1);
    }
    let survivor = rest(&mut market, theirs, Side::Bid, 99, 1);

    let cancelled = market.cancel_orders_for_seat(mine, Side::Bid, u32::MAX).unwrap();

    assert_eq!(cancelled, 4);
    assert_eq!(market.book().len(Side::Bid), 1);
    assert_eq!(market.book().best_bid().unwrap().key, survivor);
    assert_eq!(locked(&market, mine), (0, 0));
}

#[test]
fn cancel_all_respects_its_bound() {
    let mut market = market();
    let seat = funded_seat(&mut market, 1);
    for price in 100..106u64 {
        rest(&mut market, seat, Side::Bid, price, 1);
    }

    assert_eq!(market.cancel_orders_for_seat(seat, Side::Bid, 2).unwrap(), 2);
    assert_eq!(market.book().len(Side::Bid), 4);
}
