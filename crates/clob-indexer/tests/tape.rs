//! The derived tape against ground truth.
//!
//! Ground truth here is the engine itself: every scenario runs an order through
//! [`clob_engine`] with a fill observer attached, so what actually happened is known
//! exactly. The tape is then derived from the before and after snapshots alone — no
//! access to that observer — and compared.
//!
//! That is the whole claim being tested. If the diff can reconstruct the engine's own
//! fills from nothing but two book states and the instruction, an indexer does not need
//! events, and the opt-in receipt stays an optimisation rather than a requirement.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::decode::ClobInstruction;
use clob_client::state::MarketState;
use clob_engine::{
    Fill, FeeSchedule, Market, OrderPacket, PostOnlyRejection, SelfTradeBehavior, TraderKey,
};
use clob_indexer::{ObservedInstruction, RemovalReason, derive};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};

type TestMarket = Market<128, 128, 32>;

fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

/// A market account backed by owned bytes, so snapshots can be taken by cloning.
struct Fixture {
    data: Vec<u8>,
}

impl Fixture {
    fn new(taker_fee_bps: u64) -> Self {
        let mut data = vec![0u8; SizeClass::Small.account_len()];
        let (header, body) = data.split_at_mut(HEADER_LEN);
        *bytemuck::from_bytes_mut::<MarketAccountHeader>(header) = MarketAccountHeader {
            discriminator: MARKET_DISCRIMINATOR,
            version: MARKET_VERSION,
            size_class: SizeClass::Small as u64,
            ..Default::default()
        };
        bytemuck::from_bytes_mut::<TestMarket>(&mut body[..TestMarket::SIZE_IN_BYTES])
            .initialize(lot_config(), FeeSchedule::new(taker_fee_bps).unwrap())
            .unwrap();
        Self { data }
    }

    fn market(&mut self) -> &mut TestMarket {
        bytemuck::from_bytes_mut(&mut self.data[HEADER_LEN..HEADER_LEN + TestMarket::SIZE_IN_BYTES])
    }

    fn snapshot(&self) -> MarketState {
        MarketState::decode(&self.data).expect("fixture should decode")
    }

    fn seat(&mut self, id: u8) -> u32 {
        let market = self.market();
        let seat = market.claim_seat(TraderKey([id; 32])).unwrap();
        market
            .deposit(seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
            .unwrap();
        seat
    }

    fn rest(&mut self, seat: u32, side: Side, price: u64, size: u64) {
        self.market()
            .place_order(
                seat,
                OrderPacket::PostOnly {
                    side,
                    price_in_ticks: Ticks(price),
                    num_base_lots: BaseLots(size),
                    rejection: PostOnlyRejection::Reject,
                },
                &mut (),
            )
            .unwrap();
    }

    /// Runs a packet, returning the snapshots either side of it and the engine's own
    /// account of what happened.
    fn apply(&mut self, seat: u32, packet: OrderPacket) -> (MarketState, MarketState, Vec<Fill>) {
        let before = self.snapshot();
        let mut fills: Vec<Fill> = Vec::new();
        self.market().place_order(seat, packet, &mut fills).unwrap();
        (before, self.snapshot(), fills)
    }
}

/// The instruction an observer would have decoded for a `place_order`, with the seat
/// the transaction's accounts identify as the submitter.
fn placed(packet: OrderPacket, seat: u32) -> ObservedInstruction {
    ObservedInstruction::new(
        ClobInstruction::PlaceOrder {
            packet,
            receipt: false,
        },
        seat,
    )
}

#[test]
fn a_single_fill_is_reconstructed_exactly() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    fixture.rest(maker, Side::Ask, 100, 10);

    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(4), 8);
    let (before, after, fills) = fixture.apply(taker, packet);
    let delta = derive(&before, &after, &[placed(packet, taker)], 42);

    assert_eq!(delta.trades.len(), 1);
    assert_eq!(delta.trades[0].price_in_ticks, Ticks(100));
    assert_eq!(delta.trades[0].base_lots, BaseLots(4));
    assert_eq!(delta.trades[0].quote_lots, QuoteLots(400));
    assert_eq!(delta.trades[0].taker_side, Side::Bid);
    assert_eq!(delta.trades[0].maker_seat, maker);
    assert_eq!(delta.trades[0].slot, 42);

    // Identical to what the engine reported, which the derivation never saw.
    assert_eq!(delta.trades[0].base_lots, fills[0].base_lots_filled);
    assert_eq!(delta.trades[0].quote_lots, fills[0].quote_lots_filled);
    assert_eq!(delta.trades[0].maker_order_id, fills[0].maker_order_id);
}

#[test]
fn a_multi_level_sweep_matches_the_engine_fill_for_fill() {
    let mut fixture = Fixture::new(2);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    for (price, size) in [(100u64, 5u64), (101, 5), (102, 5)] {
        fixture.rest(maker, Side::Ask, price, size);
    }

    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(12), 16);
    let (before, after, fills) = fixture.apply(taker, packet);
    let delta = derive(&before, &after, &[placed(packet, taker)], 7);

    assert_eq!(delta.trades.len(), fills.len());
    for (derived, actual) in delta.trades.iter().zip(&fills) {
        assert_eq!(derived.price_in_ticks, actual.price_in_ticks);
        assert_eq!(derived.base_lots, actual.base_lots_filled);
        assert_eq!(derived.quote_lots, actual.quote_lots_filled);
        assert_eq!(derived.maker_order_id, actual.maker_order_id);
    }
    // And in the order the taker consumed them.
    assert_eq!(
        delta.trades.iter().map(|t| t.price_in_ticks.as_u64()).collect::<Vec<_>>(),
        vec![100, 101, 102]
    );
    assert!(delta.fees_reconcile(2), "derived tape disagrees with the fee counter");
}

#[test]
fn a_taker_selling_walks_bids_downward() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    for price in [100u64, 99, 98] {
        fixture.rest(maker, Side::Bid, price, 5);
    }

    let packet = clob_client::instruction::market_order(Side::Ask, BaseLots(12), 16);
    let (before, after, _) = fixture.apply(taker, packet);
    let delta = derive(&before, &after, &[placed(packet, taker)], 1);

    assert_eq!(
        delta.trades.iter().map(|t| t.price_in_ticks.as_u64()).collect::<Vec<_>>(),
        vec![100, 99, 98]
    );
    assert!(delta.trades.iter().all(|t| t.taker_side == Side::Ask));
}

#[test]
fn a_crossing_limit_order_reports_both_its_fills_and_its_remainder() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    fixture.rest(maker, Side::Ask, 100, 4);

    let packet = clob_client::instruction::limit(Side::Bid, Ticks(100), BaseLots(10));
    let (before, after, _) = fixture.apply(taker, packet);
    let delta = derive(&before, &after, &[placed(packet, taker)], 1);

    assert_eq!(delta.trades.len(), 1);
    assert_eq!(delta.trades[0].base_lots, BaseLots(4));
    assert_eq!(delta.posted.len(), 1);
    assert_eq!(delta.posted[0].base_lots, BaseLots(6));
    assert_eq!(delta.posted[0].seat, taker);
}

#[test]
fn a_cancel_is_not_a_trade() {
    // The distinction the whole attribution scheme exists for. Counting cancels as
    // volume is the single easiest way for a venue's reported numbers to be wrong.
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    fixture.rest(maker, Side::Bid, 100, 10);

    let before = fixture.snapshot();
    let id = before.best_bid().unwrap().id;
    fixture.market().cancel_order(maker, &id).unwrap();
    let after = fixture.snapshot();

    let delta = derive(
        &before,
        &after,
        &[ObservedInstruction::new(ClobInstruction::CancelOrder { order_id: id }, maker)],
        1,
    );

    assert!(delta.trades.is_empty(), "a cancel must never appear on the tape");
    assert_eq!(delta.removals.len(), 1);
    assert_eq!(delta.removals[0].reason, RemovalReason::Cancelled);
    assert_eq!(delta.removals[0].base_lots, BaseLots(10));
}

#[test]
fn a_cancel_all_is_attributed_without_naming_ids() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    for price in [100u64, 99, 98] {
        fixture.rest(maker, Side::Bid, price, 5);
    }

    let before = fixture.snapshot();
    fixture
        .market()
        .cancel_orders_for_seat(maker, Side::Bid, u32::MAX)
        .unwrap();
    let after = fixture.snapshot();

    let delta = derive(
        &before,
        &after,
        &[ObservedInstruction::new(
            ClobInstruction::CancelAllOrders {
                side: Side::Bid,
                limit: u32::MAX,
            },
            maker,
        )],
        1,
    );

    assert!(delta.trades.is_empty());
    assert_eq!(delta.removals.len(), 3);
    assert!(delta.removals.iter().all(|r| r.reason == RemovalReason::Cancelled));
}

#[test]
fn a_reduce_shows_up_as_a_partial_removal() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    fixture.rest(maker, Side::Ask, 100, 10);

    let before = fixture.snapshot();
    let id = before.best_ask().unwrap().id;
    fixture.market().reduce_order(maker, &id, BaseLots(4)).unwrap();
    let after = fixture.snapshot();

    let delta = derive(
        &before,
        &after,
        &[ObservedInstruction::new(
            ClobInstruction::ReduceOrder {
                order_id: id,
                base_lots: BaseLots(4),
            },
            maker,
        )],
        1,
    );

    assert!(delta.trades.is_empty());
    assert_eq!(delta.removals[0].base_lots, BaseLots(4), "only the removed part");
    assert_eq!(delta.removals[0].reason, RemovalReason::Cancelled);
}

#[test]
fn a_self_trade_removes_liquidity_without_creating_volume() {
    // Recording this as a trade would let anyone inflate a market's reported volume by
    // crossing their own quotes, at no cost, forever.
    let mut fixture = Fixture::new(0);
    let mine = fixture.seat(1);
    fixture.rest(mine, Side::Ask, 100, 10);

    let packet = OrderPacket::Limit {
        side: Side::Bid,
        price_in_ticks: Ticks(100),
        num_base_lots: BaseLots(10),
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        match_limit: 8,
    };
    let (before, after, fills) = fixture.apply(mine, packet);
    let delta = derive(&before, &after, &[placed(packet, mine)], 1);

    assert!(fills.is_empty(), "the engine reports no fill either");
    assert!(delta.trades.is_empty());
    assert_eq!(delta.removals.len(), 1);
    assert_eq!(delta.removals[0].reason, RemovalReason::SelfTraded);
    assert_eq!(delta.fees_earned, QuoteLots::ZERO);
}

#[test]
fn a_transaction_that_cancels_and_takes_at_once_separates_the_two() {
    // A market maker replacing a quote does exactly this, so getting it wrong would
    // mis-tape routine activity rather than an edge case.
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let other = fixture.seat(2);
    fixture.rest(other, Side::Ask, 100, 10);
    fixture.rest(maker, Side::Bid, 90, 5);

    let before = fixture.snapshot();
    let stale = before
        .bids
        .iter()
        .find(|o| o.trader_index == maker)
        .unwrap()
        .id;

    // Pull the stale bid, then lift the offer.
    fixture.market().cancel_order(maker, &stale).unwrap();
    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(6), 8);
    fixture.market().place_order(maker, packet, &mut ()).unwrap();
    let after = fixture.snapshot();

    let delta = derive(
        &before,
        &after,
        &[
            ObservedInstruction::new(ClobInstruction::CancelOrder { order_id: stale }, maker),
            placed(packet, maker),
        ],
        1,
    );

    assert_eq!(delta.trades.len(), 1, "the lift is a trade");
    assert_eq!(delta.trades[0].base_lots, BaseLots(6));
    assert_eq!(delta.trades[0].maker_seat, other);
    assert_eq!(delta.removals.len(), 1, "the pull is not");
    assert_eq!(delta.removals[0].order_id, stale);
    assert_eq!(delta.removals[0].reason, RemovalReason::Cancelled);
}

#[test]
fn a_fill_names_the_taker_that_caused_it() {
    // Both sides of a fill, so a trader's own history can be built by asking for the
    // trades where it was the maker or the taker. Without this a retail UI can only show
    // the half of a user's activity they did not initiate.
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    fixture.rest(maker, Side::Ask, 100, 10);

    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(4), 8);
    let (before, after, _) = fixture.apply(taker, packet);
    let delta = derive(&before, &after, &[placed(packet, taker)], 1);

    assert_eq!(delta.trades.len(), 1);
    assert_eq!(delta.trades[0].maker_seat, maker);
    assert_eq!(delta.trades[0].taker_seat, Some(taker));
    assert_ne!(maker, taker, "otherwise the assertion above proves nothing");
}

#[test]
fn two_takers_on_one_side_leave_the_fill_unattributed() {
    // Liquidity leaving is all a diff sees. With two takers crossing the same side, the
    // book cannot say which of them consumed a given order, and naming the first would
    // file one trader's fill under the other's name.
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let first = fixture.seat(2);
    let second = fixture.seat(3);
    fixture.rest(maker, Side::Ask, 100, 10);

    let before = fixture.snapshot();
    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(3), 8);
    fixture.market().place_order(first, packet, &mut ()).unwrap();
    fixture
        .market()
        .place_order(second, packet, &mut ())
        .unwrap();
    let after = fixture.snapshot();

    let delta = derive(
        &before,
        &after,
        &[placed(packet, first), placed(packet, second)],
        1,
    );

    assert_eq!(delta.trades.len(), 1, "one resting order, so one fill");
    assert_eq!(delta.trades[0].base_lots, BaseLots(6), "both takes");
    assert_eq!(
        delta.trades[0].taker_seat, None,
        "unattributed, not attributed to whichever came first"
    );
}

#[test]
fn a_taker_whose_seat_did_not_resolve_leaves_the_fill_unattributed() {
    // The seat comes from the transaction's account keys, and a reader that could not
    // resolve them says so. The volume is still real, so the trade stands — only the
    // name is missing.
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    fixture.rest(maker, Side::Ask, 100, 10);

    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(4), 8);
    let (before, after, _) = fixture.apply(taker, packet);
    let anonymous = ObservedInstruction::anonymous(ClobInstruction::PlaceOrder {
        packet,
        receipt: false,
    });
    let delta = derive(&before, &after, &[anonymous], 1);

    assert_eq!(delta.trades.len(), 1, "the fill still happened");
    assert_eq!(delta.trades[0].base_lots, BaseLots(4));
    assert_eq!(delta.trades[0].taker_seat, None);
}

#[test]
fn an_untouched_market_produces_nothing() {
    let mut fixture = Fixture::new(0);
    let maker = fixture.seat(1);
    fixture.rest(maker, Side::Ask, 100, 10);

    let snapshot = fixture.snapshot();
    let delta = derive(&snapshot, &snapshot, &[], 1);

    assert!(delta.is_empty());
    assert_eq!(delta.fees_earned, QuoteLots::ZERO);
}

#[test]
fn fee_reconciliation_catches_a_tape_that_does_not_add_up() {
    let mut fixture = Fixture::new(10);
    let maker = fixture.seat(1);
    let taker = fixture.seat(2);
    fixture.rest(maker, Side::Ask, 100, 20);

    let packet = clob_client::instruction::market_order(Side::Bid, BaseLots(20), 8);
    let (before, after, _) = fixture.apply(taker, packet);
    let mut delta = derive(&before, &after, &[placed(packet, taker)], 1);

    assert!(delta.fees_reconcile(10));

    // Drop a fill, as a missed instruction would.
    delta.trades.clear();
    assert!(
        !delta.fees_reconcile(10),
        "an empty tape against a non-zero fee must not reconcile"
    );
}
