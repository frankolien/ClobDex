//! Cancelling and placing in one instruction.
//!
//! The market-maker cycle. A maker refreshing a two-sided ladder does it continuously,
//! and doing it as separate instructions costs a transaction per quote update.
//!
//! The interesting behaviour is not the happy path — it is what happens when a quote got
//! hit between the maker deciding to replace it and the replacement landing, which is
//! the normal case at any meaningful volume.

mod common;

use clob_book::{BaseLots, Side};
use solana_pubkey::Pubkey;

use common::*;

#[test]
fn a_batch_cancels_then_places() {
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);

    // A resting ladder.
    let mut market = fixture.run(
        &[
            post_only_ix(fixture.market, maker, Side::Ask, 105, 10),
            post_only_ix(fixture.market, maker, Side::Ask, 106, 10),
        ],
        market,
        maker,
    );

    let stale: Vec<_> = market_of(&market)
        .book()
        .iter_side(Side::Ask)
        .map(|entry| entry.key)
        .collect();
    assert_eq!(stale.len(), 2);

    // Replace both with a tighter ladder, in one instruction.
    market = fixture.run(
        &[batch_ix(
            fixture.market,
            maker,
            &stale,
            &[
                post_only_packet(Side::Ask, 103, 10),
                post_only_packet(Side::Ask, 104, 10),
            ],
        )],
        market,
        maker,
    );

    let prices: Vec<u64> = market_of(&market)
        .book()
        .iter_side(Side::Ask)
        .map(|entry| entry.key.price_in_ticks.as_u64())
        .collect();
    assert_eq!(prices, vec![103, 104], "the old ladder went, the new one rests");
}

#[test]
fn cancelling_an_order_that_already_filled_is_not_an_error() {
    // The case the whole design turns on. A maker decides to replace a quote; the quote
    // gets hit before the replacement lands. Treating that as a failure would revert the
    // refresh exactly when the maker most needs it to succeed.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);
    fixture.seat(&mut market, taker, 1_000_000, 1_000_000_000);

    let mut market = fixture.run(
        &[post_only_ix(fixture.market, maker, Side::Ask, 105, 10)],
        market,
        maker,
    );
    let hit = newest_order(&market, Side::Ask);

    // The taker sweeps it.
    market = fixture.run(
        &[market_order_ix(fixture.market, taker, Side::Bid, 10, 64)],
        market,
        taker,
    );
    assert_eq!(market_of(&market).book().iter_side(Side::Ask).count(), 0);

    // The maker replaces an order that is no longer there.
    market = fixture.run(
        &[batch_ix(
            fixture.market,
            maker,
            &[hit],
            &[post_only_packet(Side::Ask, 104, 10)],
        )],
        market,
        maker,
    );

    let prices: Vec<u64> = market_of(&market)
        .book()
        .iter_side(Side::Ask)
        .map(|entry| entry.key.price_in_ticks.as_u64())
        .collect();
    assert_eq!(prices, vec![104], "the replacement rests despite the stale cancel");
}

#[test]
fn a_batch_cannot_cancel_someone_elses_order() {
    // Tolerating a missing order must not become tolerating any order. A maker that
    // could cancel another's quotes could clear the book before taking it.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let owner = trader(1);
    let thief = trader(2);
    fixture.seat(&mut market, owner, 1_000_000, 1_000_000_000);
    fixture.seat(&mut market, thief, 1_000_000, 1_000_000_000);

    let market = fixture.run(
        &[post_only_ix(fixture.market, owner, Side::Ask, 105, 10)],
        market,
        owner,
    );
    let victim = newest_order(&market, Side::Ask);

    fixture.expect_failure(
        &[batch_ix(fixture.market, thief, &[victim], &[])],
        market,
        thief,
    );
}

#[test]
fn a_rejected_order_fails_the_whole_batch() {
    // Half a ladder is worse than none: the maker would have to read the book back to
    // discover what it actually owns, which is the round trip batching exists to avoid.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    // Enough for one order, not two.
    fixture.seat(&mut market, maker, 10, 1_000_000_000);

    fixture.expect_failure(
        &[batch_ix(
            fixture.market,
            maker,
            &[],
            &[
                post_only_packet(Side::Ask, 105, 10),
                post_only_packet(Side::Ask, 106, 10),
            ],
        )],
        market,
        maker,
    );
}

#[test]
fn cancels_run_before_places_so_a_full_refresh_funds_itself() {
    // A maker with exactly one order's worth of capital must be able to move that order.
    // Placing first would fail on its own locked balance.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market, maker, 10, 1_000_000_000);

    let market = fixture.run(
        &[post_only_ix(fixture.market, maker, Side::Ask, 105, 10)],
        market,
        maker,
    );
    let resting = newest_order(&market, Side::Ask);

    let market = fixture.run(
        &[batch_ix(
            fixture.market,
            maker,
            &[resting],
            &[post_only_packet(Side::Ask, 103, 10)],
        )],
        market,
        maker,
    );

    let prices: Vec<u64> = market_of(&market)
        .book()
        .iter_side(Side::Ask)
        .map(|entry| entry.key.price_in_ticks.as_u64())
        .collect();
    assert_eq!(prices, vec![103], "all the capital moved to the new quote");
}

#[test]
fn an_empty_batch_is_a_no_op_rather_than_a_failure() {
    // A maker with nothing to change should not have to special-case that.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);

    let market = fixture.run(&[batch_ix(fixture.market, maker, &[], &[])], market, maker);
    assert_eq!(market_of(&market).book().iter_side(Side::Ask).count(), 0);
}

#[test]
fn a_batch_matches_against_the_book_like_any_order() {
    // Batching changes when orders are submitted, never how they match.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);
    fixture.seat(&mut market, taker, 1_000_000, 1_000_000_000);

    let market = fixture.run(
        &[post_only_ix(fixture.market, maker, Side::Ask, 105, 10)],
        market,
        maker,
    );

    let packet = clob_engine::OrderPacket::Limit {
        side: Side::Bid,
        price_in_ticks: clob_book::Ticks(105),
        num_base_lots: BaseLots(10),
        self_trade_behavior: clob_engine::SelfTradeBehavior::DecrementTake,
        match_limit: 64,
    };
    let market = fixture.run(
        &[batch_ix(fixture.market, taker, &[], &[packet])],
        market,
        taker,
    );

    assert_eq!(
        market_of(&market).book().iter_side(Side::Ask).count(),
        0,
        "the batched order crossed and consumed the resting ask"
    );
}

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}
