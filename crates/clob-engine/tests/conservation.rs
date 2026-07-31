//! The invariant the engine exists to preserve: value is neither created nor destroyed.
//!
//! Every other property is negotiable. If a sequence of operations can leave the market
//! holding less than it owes, the market can be drained, and no amount of throughput or
//! ergonomics compensates.
//!
//! Two independent checks run after *every* operation, successful or rejected:
//!
//! 1. [`Market::check_conservation`] — the engine's own internal consistency: seat
//!    balances sum to the deposited totals, locked funds correspond exactly to resting
//!    orders, every order has a live owner.
//! 2. A model maintained here that tracks deposits, withdrawals and fee sweeps from the
//!    outside. This catches the case where the engine and its self-check share the same
//!    wrong assumption.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::{FeeSchedule, Market, OrderPacket, SelfTradeBehavior, TraderKey};
use proptest::prelude::*;

const TRADERS: usize = 4;
type TestMarket = Market<24, 24, TRADERS>;

#[derive(Debug, Clone)]
enum Op {
    Deposit { trader: usize, base: u64, quote: u64 },
    Withdraw { trader: usize, base: u64, quote: u64 },
    Limit { trader: usize, side: Side, price: u64, size: u64 },
    PostOnly { trader: usize, side: Side, price: u64, size: u64 },
    Market { trader: usize, side: Side, size: u64 },
    Cancel { trader: usize, pick: usize },
    Reduce { trader: usize, pick: usize, size: u64 },
    CollectFees,
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Bid), Just(Side::Ask)]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..TRADERS, 0u64..500, 0u64..500_000)
            .prop_map(|(trader, base, quote)| Op::Deposit { trader, base, quote }),
        2 => (0..TRADERS, 0u64..200, 0u64..200_000)
            .prop_map(|(trader, base, quote)| Op::Withdraw { trader, base, quote }),
        8 => (0..TRADERS, side(), 95u64..106, 1u64..40)
            .prop_map(|(trader, side, price, size)| Op::Limit { trader, side, price, size }),
        4 => (0..TRADERS, side(), 95u64..106, 1u64..40)
            .prop_map(|(trader, side, price, size)| Op::PostOnly { trader, side, price, size }),
        4 => (0..TRADERS, side(), 1u64..40)
            .prop_map(|(trader, side, size)| Op::Market { trader, side, size }),
        3 => (0..TRADERS, any::<prop::sample::Index>())
            .prop_map(|(trader, i)| Op::Cancel { trader, pick: i.index(usize::MAX) }),
        3 => (0..TRADERS, any::<prop::sample::Index>(), 1u64..40)
            .prop_map(|(trader, i, size)| Op::Reduce { trader, pick: i.index(usize::MAX), size }),
        1 => Just(Op::CollectFees),
    ]
}

/// Deposits and withdrawals as seen from outside the engine.
#[derive(Default, Debug, PartialEq, Eq)]
struct Model {
    base_in_market: u64,
    quote_in_market: u64,
    fees_swept: u64,
}

/// Every id `trader` currently has resting, on either side.
fn resting_ids(market: &TestMarket, trader: u32) -> Vec<clob_book::FIFOOrderId> {
    [Side::Bid, Side::Ask]
        .into_iter()
        .flat_map(|s| market.book().iter_side(s))
        .filter(|e| e.value.trader_index as u32 == trader)
        .map(|e| e.key)
        .collect()
}

fn apply(market: &mut TestMarket, model: &mut Model, seats: &[u32], op: &Op) {
    match *op {
        Op::Deposit { trader, base, quote } => {
            if market
                .deposit(seats[trader], BaseLots(base), QuoteLots(quote))
                .is_ok()
            {
                model.base_in_market += base;
                model.quote_in_market += quote;
            }
        }
        Op::Withdraw { trader, base, quote } => {
            if market
                .withdraw(seats[trader], BaseLots(base), QuoteLots(quote))
                .is_ok()
            {
                model.base_in_market -= base;
                model.quote_in_market -= quote;
            }
        }
        Op::Limit { trader, side, price, size } => {
            let _ = market.place_order(
                seats[trader],
                OrderPacket::Limit {
                    side,
                    price_in_ticks: Ticks(price),
                    num_base_lots: BaseLots(size),
                    self_trade_behavior: SelfTradeBehavior::DecrementTake,
                    match_limit: 8,
                },
                &mut (),
            );
        }
        Op::PostOnly { trader, side, price, size } => {
            let _ = market.place_order(
                seats[trader],
                OrderPacket::post_only(side, Ticks(price), BaseLots(size)),
                &mut (),
            );
        }
        Op::Market { trader, side, size } => {
            // No minimum fill: the minimum-fill path returns Err *after* mutating, on
            // the assumption that the runtime reverts. That assumption is what a
            // Solana instruction gives for free, but it is not true of this test
            // harness, so the random walk stays clear of it. It is covered explicitly
            // in `minimum_fill.rs` instead.
            let _ = market.place_order(
                seats[trader],
                OrderPacket::ImmediateOrCancel {
                    side,
                    price_in_ticks: None,
                    num_base_lots: BaseLots(size),
                    min_base_lots_to_fill: BaseLots::ZERO,
                    self_trade_behavior: SelfTradeBehavior::DecrementTake,
                    match_limit: 8,
                },
                &mut (),
            );
        }
        Op::Cancel { trader, pick } => {
            let ids = resting_ids(market, seats[trader]);
            if !ids.is_empty() {
                let _ = market.cancel_order(seats[trader], &ids[pick % ids.len()]);
            }
        }
        Op::Reduce { trader, pick, size } => {
            let ids = resting_ids(market, seats[trader]);
            if !ids.is_empty() {
                let _ = market.reduce_order(seats[trader], &ids[pick % ids.len()], BaseLots(size));
            }
        }
        Op::CollectFees => {
            if let Ok(swept) = market.collect_fees() {
                model.quote_in_market -= swept.as_u64();
                model.fees_swept += swept.as_u64();
            }
        }
    }
}

fn fresh() -> (std::boxed::Box<TestMarket>, Vec<u32>) {
    let lots = LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap();
    let mut market = TestMarket::new_boxed(lots, FeeSchedule::new(5).unwrap()).unwrap();
    let seats = (0..TRADERS)
        .map(|i| market.claim_seat(TraderKey([i as u8; 32])).unwrap())
        .collect();
    (market, seats)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// The headline property. Nothing a caller can do -- in any order, valid or
    /// rejected -- may leave the market owing more than it holds.
    #[test]
    fn value_is_conserved_through_arbitrary_activity(ops in prop::collection::vec(op(), 1..200)) {
        let (mut market, seats) = fresh();
        let mut model = Model::default();

        for op in &ops {
            apply(&mut market, &mut model, &seats, op);

            prop_assert_eq!(market.check_conservation(), Ok(()), "after {:?}", op);
            prop_assert_eq!(
                market.header().base_lots_deposited.as_u64(),
                model.base_in_market,
                "base diverged from the external model after {:?}", op
            );
            prop_assert_eq!(
                market.header().quote_lots_deposited.as_u64(),
                model.quote_in_market,
                "quote diverged from the external model after {:?}", op
            );
        }
    }

    /// Every lot that went in comes back out. After cancelling all orders and
    /// withdrawing every free balance, the market must hold exactly its unswept fees
    /// and nothing else -- no stranded dust, no locked remainder.
    #[test]
    fn the_market_can_always_be_fully_drained(ops in prop::collection::vec(op(), 1..200)) {
        let (mut market, seats) = fresh();
        let mut model = Model::default();

        for op in &ops {
            apply(&mut market, &mut model, &seats, op);
        }

        for seat in &seats {
            for side in [Side::Bid, Side::Ask] {
                market.cancel_orders_for_seat(*seat, side, u32::MAX).unwrap();
            }
        }
        prop_assert_eq!(market.book().len(Side::Bid) + market.book().len(Side::Ask), 0);

        let mut base_out = 0u64;
        let mut quote_out = 0u64;
        for seat in &seats {
            let (base, quote) = market.withdraw_all(*seat).unwrap();
            base_out += base.as_u64();
            quote_out += quote.as_u64();
        }

        // Nothing is stranded: every seat is empty, so every seat can be released.
        for i in 0..TRADERS {
            prop_assert!(market.release_seat(&TraderKey([i as u8; 32])).is_ok());
        }

        let fees = market.header().unclaimed_quote_lot_fees.as_u64();
        prop_assert_eq!(market.header().base_lots_deposited, BaseLots::ZERO);
        prop_assert_eq!(market.header().quote_lots_deposited.as_u64(), fees);
        prop_assert_eq!(base_out, model.base_in_market);
        prop_assert_eq!(quote_out + fees, model.quote_in_market);
    }

    /// Fees are only ever taken from takers, and only out of value that was already in
    /// the market. Lifetime fees must equal what has been swept plus what is unswept.
    #[test]
    fn fees_are_accounted_exactly(ops in prop::collection::vec(op(), 1..200)) {
        let (mut market, seats) = fresh();
        let mut model = Model::default();

        for op in &ops {
            apply(&mut market, &mut model, &seats, op);

            let header = market.header();
            prop_assert_eq!(
                header.collected_quote_lot_fees.as_u64(),
                model.fees_swept + header.unclaimed_quote_lot_fees.as_u64(),
                "lifetime fees do not reconcile with swept plus unswept"
            );
            prop_assert!(
                header.unclaimed_quote_lot_fees <= header.quote_lots_deposited,
                "unclaimed fees exceed the quote the market holds"
            );
        }
    }
}
