//! Shared setup for the behavioural test suites.

#![allow(dead_code)]

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::{FeeSchedule, Fill, Market, OrderPacket, SeatIndex, TraderKey};

pub type TestMarket = Market<32, 32, 8>;

/// SOL/USDC-shaped geometry: one base lot is 0.001 SOL, one tick is $0.001.
pub fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

/// A market with the given taker fee, in basis points.
pub fn market_with_fee(taker_fee_bps: u64) -> std::boxed::Box<TestMarket> {
    TestMarket::new_boxed(lot_config(), FeeSchedule::new(taker_fee_bps).unwrap()).unwrap()
}

/// A fee-free market, so tests about matching are not also tests about rounding.
pub fn market() -> std::boxed::Box<TestMarket> {
    market_with_fee(0)
}

/// Claims a seat identified by `id` and funds it generously.
pub fn funded_seat(market: &mut TestMarket, id: u8) -> SeatIndex {
    let seat = market.claim_seat(TraderKey([id; 32])).unwrap();
    market
        .deposit(seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
        .unwrap();
    seat
}

/// Claims a seat with exactly the balances given.
pub fn seat_with(market: &mut TestMarket, id: u8, base: u64, quote: u64) -> SeatIndex {
    let seat = market.claim_seat(TraderKey([id; 32])).unwrap();
    market
        .deposit(seat, BaseLots(base), QuoteLots(quote))
        .unwrap();
    seat
}

/// Rests a plain limit order that is expected to post in full.
pub fn rest(
    market: &mut TestMarket,
    seat: SeatIndex,
    side: Side,
    price: u64,
    size: u64,
) -> FIFOOrderId {
    let outcome = market
        .place_order(
            seat,
            OrderPacket::post_only(side, Ticks(price), BaseLots(size)),
            &mut (),
        )
        .expect("order should post");
    outcome.order_id.expect("post-only always rests")
}

/// Collects fills while placing an order.
pub fn place_recording(
    market: &mut TestMarket,
    seat: SeatIndex,
    packet: OrderPacket,
) -> (clob_engine::Result<clob_engine::OrderOutcome>, Vec<Fill>) {
    let mut fills = Vec::new();
    let outcome = market.place_order(seat, packet, &mut fills);
    (outcome, fills)
}

/// Free balances for a seat, as `(base, quote)`.
pub fn free(market: &TestMarket, seat: SeatIndex) -> (u64, u64) {
    let state = market.traders().state(seat).unwrap();
    (
        state.base_lots_free.as_u64(),
        state.quote_lots_free.as_u64(),
    )
}

/// Locked balances for a seat, as `(base, quote)`.
pub fn locked(market: &TestMarket, seat: SeatIndex) -> (u64, u64) {
    let state = market.traders().state(seat).unwrap();
    (
        state.base_lots_locked.as_u64(),
        state.quote_lots_locked.as_u64(),
    )
}

/// The prices resting on `side`, best first.
pub fn prices(market: &TestMarket, side: Side) -> Vec<u64> {
    market
        .book()
        .iter_side(side)
        .map(|e| e.key.price_in_ticks.as_u64())
        .collect()
}
