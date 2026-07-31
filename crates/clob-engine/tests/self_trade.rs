//! The three self-trade policies.
//!
//! A market maker quoting both sides crosses itself whenever it moves a quote through
//! the touch, so this is routine rather than exceptional. What differs between desks is
//! what they want to happen, which is why the policy is the caller's to pick.

mod common;

use clob_book::{BaseLots, Side, Ticks};
use clob_engine::{EngineError, OrderPacket, SelfTradeBehavior};

use common::*;

/// `mine` rests an ask at 100; a stranger rests one at 101. `mine` then bids through
/// both. What happens to the ask at 100 is the policy under test.
fn crossing_own_quote() -> (std::boxed::Box<TestMarket>, u32, u32) {
    let mut market = market();
    let mine = funded_seat(&mut market, 1);
    let theirs = funded_seat(&mut market, 2);
    rest(&mut market, mine, Side::Ask, 100, 10);
    rest(&mut market, theirs, Side::Ask, 101, 10);
    (market, mine, theirs)
}

fn bid_through(behavior: SelfTradeBehavior, size: u64) -> OrderPacket {
    OrderPacket::Limit {
        side: Side::Bid,
        price_in_ticks: Ticks(101),
        num_base_lots: BaseLots(size),
        self_trade_behavior: behavior,
        match_limit: u32::MAX,
    }
}

#[test]
fn decrement_take_shrinks_both_sides_and_trades_nothing() {
    let (mut market, mine, _) = crossing_own_quote();
    let before = free(&market, mine);

    let outcome = market
        .place_order(mine, bid_through(SelfTradeBehavior::DecrementTake, 10), &mut ())
        .unwrap();

    // The overlap is removed from the book but never trades: no fill, no fee, and the
    // taker's own size is consumed by the removal.
    assert_eq!(outcome.base_lots_filled, BaseLots::ZERO);
    assert_eq!(outcome.base_lots_self_traded, BaseLots(10));
    assert_eq!(outcome.base_lots_posted, BaseLots::ZERO);
    assert_eq!(market.header().collected_quote_lot_fees.as_u64(), 0);

    // Own ask gone, stranger's untouched, and the locked base came back.
    assert_eq!(prices(&market, Side::Ask), vec![101]);
    assert_eq!(free(&market, mine), (before.0 + 10, before.1));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn decrement_take_only_removes_the_overlap() {
    let (mut market, mine, _) = crossing_own_quote();

    // Taker size 4 against an own resting 10: only 4 comes off.
    let outcome = market
        .place_order(mine, bid_through(SelfTradeBehavior::DecrementTake, 4), &mut ())
        .unwrap();

    assert_eq!(outcome.base_lots_self_traded, BaseLots(4));
    assert_eq!(
        market.book().best_ask().unwrap().value.num_base_lots,
        BaseLots(6)
    );
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn cancel_provide_pulls_the_stale_quote_and_keeps_matching() {
    let (mut market, mine, theirs) = crossing_own_quote();

    let (outcome, fills) =
        place_recording(&mut market, mine, bid_through(SelfTradeBehavior::CancelProvide, 10));
    let outcome = outcome.unwrap();

    // The own ask is cancelled outright without consuming taker size, so the full 10
    // goes on to trade against the stranger at 101.
    assert_eq!(outcome.base_lots_self_traded, BaseLots(10));
    assert_eq!(outcome.base_lots_filled, BaseLots(10));
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].maker_seat, theirs);
    assert_eq!(fills[0].price_in_ticks, Ticks(101));
    assert!(market.book().is_empty(Side::Ask));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn abort_rejects_the_whole_order() {
    let (mut market, mine, _) = crossing_own_quote();
    let book_before: Vec<_> = market.book().iter_side(Side::Ask).map(|e| e.key).collect();

    assert_eq!(
        market.place_order(mine, bid_through(SelfTradeBehavior::Abort, 10), &mut ()),
        Err(EngineError::SelfTradeAborted)
    );

    // Aborting happens before anything is settled, so the book is untouched.
    let book_after: Vec<_> = market.book().iter_side(Side::Ask).map(|e| e.key).collect();
    assert_eq!(book_before, book_after);
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn a_self_trade_against_a_deeper_level_still_reaches_better_liquidity_first() {
    // Own quote sits behind a stranger's better one: the stranger fills first, and only
    // then does the self-trade policy come into play.
    let mut market = market();
    let mine = funded_seat(&mut market, 1);
    let theirs = funded_seat(&mut market, 2);
    rest(&mut market, theirs, Side::Ask, 100, 5);
    rest(&mut market, mine, Side::Ask, 101, 5);

    let (outcome, fills) =
        place_recording(&mut market, mine, bid_through(SelfTradeBehavior::DecrementTake, 10));
    let outcome = outcome.unwrap();

    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].maker_seat, theirs);
    assert_eq!(outcome.base_lots_filled, BaseLots(5));
    assert_eq!(outcome.base_lots_self_traded, BaseLots(5));
    assert!(market.book().is_empty(Side::Ask));
    assert_eq!(market.check_conservation(), Ok(()));
}

#[test]
fn self_trade_handling_is_counted_against_the_match_limit() {
    // Otherwise a trader with many own quotes at the touch could force an unbounded
    // walk and blow the compute budget.
    let mut market = market();
    let mine = funded_seat(&mut market, 1);
    for price in 100..106u64 {
        rest(&mut market, mine, Side::Ask, price, 1);
    }

    let outcome = market
        .place_order(
            mine,
            OrderPacket::Limit {
                side: Side::Bid,
                price_in_ticks: Ticks(110),
                num_base_lots: BaseLots(6),
                self_trade_behavior: SelfTradeBehavior::CancelProvide,
                match_limit: 2,
            },
            &mut (),
        )
        .unwrap();

    assert_eq!(outcome.base_lots_self_traded, BaseLots(2));
    assert_eq!(market.book().len(Side::Ask), 4);
    assert_eq!(market.check_conservation(), Ok(()));
}
