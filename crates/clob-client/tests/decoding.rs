//! Decoding market accounts and trade events.
//!
//! Both decoders are checked against the producer rather than against a fixture: market
//! state is built with the engine and read back, and events are written by the
//! program's own [`EventBuffer`] and read back. A fixture would freeze whatever the
//! encoder did on the day it was captured, including a mistake.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::event;
use clob_client::state::{DecodeError, MarketState};
use clob_engine::{FeeSchedule, FillObserver, Market, MatchStop, OrderPacket, TraderKey};
use clob_program::event::{EventBuffer, MAX_LOGGED_FILLS};
use clob_program::instruction::Discriminant;
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};

type TestMarket = Market<128, 128, 32>;

fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

/// Builds a market account exactly as the program would leave it.
fn account(taker_fee_bps: u64, build: impl FnOnce(&mut TestMarket)) -> Vec<u8> {
    let mut data = vec![0u8; SizeClass::Small.account_len()];
    let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);

    *bytemuck::from_bytes_mut::<MarketAccountHeader>(header_bytes) = MarketAccountHeader {
        discriminator: MARKET_DISCRIMINATOR,
        version: MARKET_VERSION,
        size_class: SizeClass::Small as u64,
        base_mint: [7u8; 32],
        quote_mint: [8u8; 32],
        ..Default::default()
    };

    let market =
        bytemuck::from_bytes_mut::<TestMarket>(&mut market_bytes[..TestMarket::SIZE_IN_BYTES]);
    market
        .initialize(lot_config(), FeeSchedule::new(taker_fee_bps).unwrap())
        .unwrap();
    build(market);
    data
}

/// A ladder: bids at 100/99/98 and asks at 102/103, two orders on the best bid.
fn laddered() -> Vec<u8> {
    account(2, |market| {
        let a = market.claim_seat(TraderKey([1u8; 32])).unwrap();
        let b = market.claim_seat(TraderKey([2u8; 32])).unwrap();
        market
            .deposit(a, BaseLots(100_000), QuoteLots(100_000_000))
            .unwrap();
        market
            .deposit(b, BaseLots(100_000), QuoteLots(100_000_000))
            .unwrap();

        for (seat, price, size) in [
            (a, 100u64, 10u64),
            (b, 100, 5),
            (a, 99, 20),
            (a, 98, 30),
        ] {
            market
                .place_order(
                    seat,
                    OrderPacket::post_only(Side::Bid, Ticks(price), BaseLots(size)),
                    &mut (),
                )
                .unwrap();
        }
        for (price, size) in [(102u64, 7u64), (103, 12)] {
            market
                .place_order(
                    b,
                    OrderPacket::post_only(Side::Ask, Ticks(price), BaseLots(size)),
                    &mut (),
                )
                .unwrap();
        }
    })
}

#[test]
fn a_market_decodes_into_owned_size_agnostic_state() {
    let state = MarketState::decode(&laddered()).unwrap();

    assert_eq!(state.size_class, SizeClass::Small);
    assert_eq!(state.account.base_mint, [7u8; 32]);
    assert_eq!(state.lot_config(), &lot_config());
    assert_eq!(state.fees().taker_fee_bps, 2);
    assert_eq!(state.bids.len(), 4);
    assert_eq!(state.asks.len(), 2);
    assert_eq!(state.traders.len(), 2);
}

#[test]
fn orders_arrive_in_priority_order() {
    let state = MarketState::decode(&laddered()).unwrap();

    let bid_prices: Vec<u64> = state
        .bids
        .iter()
        .map(|o| o.price_in_ticks().as_u64())
        .collect();
    let ask_prices: Vec<u64> = state
        .asks
        .iter()
        .map(|o| o.price_in_ticks().as_u64())
        .collect();

    assert_eq!(bid_prices, vec![100, 100, 99, 98]);
    assert_eq!(ask_prices, vec![102, 103]);
    // Within the best bid, the earlier order comes first.
    assert!(state.bids[0].sequence_number() < state.bids[1].sequence_number());
    assert_eq!(state.best_bid().unwrap().num_base_lots, BaseLots(10));
}

#[test]
fn level_two_aggregates_orders_at_the_same_price() {
    // The view a UI renders: a price and a size, not a list of orders.
    let state = MarketState::decode(&laddered()).unwrap();
    let levels = state.level_two(Side::Bid, 10);

    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0].price_in_ticks, Ticks(100));
    assert_eq!(levels[0].base_lots, BaseLots(15), "10 + 5 at the touch");
    assert_eq!(levels[0].order_count, 2);
    assert_eq!(levels[1].base_lots, BaseLots(20));
    assert_eq!(levels[2].base_lots, BaseLots(30));
}

#[test]
fn level_two_respects_its_depth_bound() {
    let state = MarketState::decode(&laddered()).unwrap();
    assert_eq!(state.level_two(Side::Bid, 2).len(), 2);
    assert_eq!(state.level_two(Side::Bid, 0).len(), 0);
}

#[test]
fn spread_and_mid_read_off_the_touch() {
    let state = MarketState::decode(&laddered()).unwrap();
    assert_eq!(state.spread_in_ticks(), Some(2));
    assert_eq!(state.mid_price_in_ticks(), Some(101));
}

#[test]
fn a_crossed_book_has_a_midpoint_but_no_spread() {
    // The engine will not produce a crossed book — matching runs before posting, so a
    // crossing order fills instead of resting. But an indexer feeds this decoder
    // whatever bytes it reads, and an accessor that panics on an impossible state is
    // still a crash. So the state is built directly rather than through the engine.
    let mut state = MarketState::decode(&laddered()).unwrap();
    state.bids[0].id = clob_book::FIFOOrderId::new(Side::Bid, Ticks(105), 0);
    state.asks[0].id = clob_book::FIFOOrderId::new(Side::Ask, Ticks(100), 1);

    assert_eq!(state.spread_in_ticks(), None, "a negative spread is not a spread");
    assert_eq!(state.mid_price_in_ticks(), Some(102), "an average is still an average");
}

#[test]
fn extreme_prices_do_not_panic_the_accessors() {
    let mut state = MarketState::decode(&laddered()).unwrap();
    state.bids[0].id = clob_book::FIFOOrderId::new(Side::Bid, Ticks(u64::MAX), 0);
    state.asks[0].id = clob_book::FIFOOrderId::new(Side::Ask, Ticks(u64::MAX), 1);

    // The midpoint would overflow a u64 sum, so it reports nothing rather than wrapping.
    assert_eq!(state.mid_price_in_ticks(), None);
    assert_eq!(state.spread_in_ticks(), Some(0));
}

#[test]
fn depth_and_sweep_pricing_match_the_book() {
    let state = MarketState::decode(&laddered()).unwrap();

    assert_eq!(state.depth_at_or_better(Side::Bid, Ticks(99)), BaseLots(35));
    assert_eq!(state.depth_at_or_better(Side::Bid, Ticks(101)), BaseLots::ZERO);

    // Sweeping 15 asks: all 7 at 102, then 8 of the 12 at 103.
    let sweep = state.quote_sweep(Side::Ask, BaseLots(15)).unwrap();
    assert_eq!(sweep.base_lots, BaseLots(15));
    assert_eq!(sweep.quote_lots, QuoteLots(7 * 102 + 8 * 103));
    assert_eq!(sweep.worst_price_in_ticks, Ticks(103));

    // Stopping exactly at the top level does not reach into the next one.
    let touch = state.quote_sweep(Side::Ask, BaseLots(7)).unwrap();
    assert_eq!(touch.worst_price_in_ticks, Ticks(102));

    // The book holds 19; more than that cannot be priced.
    assert_eq!(state.quote_sweep(Side::Ask, BaseLots(19)).unwrap().base_lots, BaseLots(19));
    assert_eq!(state.quote_sweep(Side::Ask, BaseLots(20)), None);
}

#[test]
fn bad_accounts_are_rejected_with_a_reason() {
    assert_eq!(MarketState::decode(&[0u8; 8]), Err(DecodeError::TooShort));

    let mut zeroed = vec![0u8; SizeClass::Small.account_len()];
    assert_eq!(MarketState::decode(&zeroed), Err(DecodeError::NotAMarket));

    let mut data = laddered();
    data[8..16].copy_from_slice(&99u64.to_le_bytes());
    assert_eq!(
        MarketState::decode(&data),
        Err(DecodeError::VersionMismatch {
            found: 99,
            expected: MARKET_VERSION
        })
    );

    let mut data = laddered();
    data[16..24].copy_from_slice(&7u64.to_le_bytes());
    assert_eq!(MarketState::decode(&data), Err(DecodeError::UnknownSizeClass(7)));

    // A correctly-tagged but truncated account.
    zeroed[..8].copy_from_slice(&MARKET_DISCRIMINATOR.to_le_bytes());
    zeroed[8..16].copy_from_slice(&MARKET_VERSION.to_le_bytes());
    zeroed.truncate(HEADER_LEN + 16);
    assert!(matches!(
        MarketState::decode(&zeroed),
        Err(DecodeError::Truncated { .. })
    ));
}

// ---------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------

/// Wraps an encoded payload the way the program emits it: discriminant, bump, payload.
fn as_instruction_data(buffer: &EventBuffer, outcome: &clob_engine::OrderOutcome, seat: u32) -> Vec<u8> {
    let mut payload = vec![0u8; clob_program::event::MAX_EVENT_LEN];
    let len = buffer.encode(outcome, seat, &mut payload);

    let mut data = vec![Discriminant::LogEvent as u8, 255];
    data.extend_from_slice(&payload[..len]);
    data
}

fn fill(seat: u32, price: u64, size: u64) -> clob_engine::Fill {
    clob_engine::Fill {
        maker_order_id: clob_book::FIFOOrderId::new(Side::Ask, Ticks(price), seat as u64),
        maker_seat: seat,
        taker_seat: 42,
        price_in_ticks: Ticks(price),
        base_lots_filled: BaseLots(size),
        quote_lots_filled: QuoteLots(size * price),
        fee_in_quote_lots: QuoteLots(1),
        maker_base_lots_remaining: BaseLots(3),
    }
}

#[test]
fn an_event_written_by_the_program_decodes_here() {
    let mut buffer = EventBuffer::new();
    buffer.on_fill(&fill(11, 100, 10));
    buffer.on_fill(&fill(12, 101, 5));

    let outcome = clob_engine::OrderOutcome {
        base_lots_filled: BaseLots(15),
        quote_lots_filled: QuoteLots(1_505),
        fee_in_quote_lots: QuoteLots(2),
        ..clob_engine::OrderOutcome::empty(MatchStop::FullyFilled)
    };

    let decoded = event::decode(&as_instruction_data(&buffer, &outcome, 42)).unwrap();

    assert_eq!(decoded.taker_seat, 42);
    assert_eq!(decoded.stop, MatchStop::FullyFilled);
    assert_eq!(decoded.base_lots_filled, BaseLots(15));
    assert_eq!(decoded.quote_lots_filled, QuoteLots(1_505));
    assert_eq!(decoded.fee_in_quote_lots, QuoteLots(2));
    assert!(!decoded.truncated());
    assert_eq!(decoded.fills.len(), 2);
    assert_eq!(decoded.fills[0].maker_seat, 11);
    assert_eq!(decoded.fills[0].price_in_ticks, Ticks(100));
    assert_eq!(decoded.fills[1].quote_lots_filled, QuoteLots(505));
    assert_eq!(decoded.fills[1].maker_base_lots_remaining, BaseLots(3));
}

#[test]
fn nothing_posted_means_no_order_id() {
    // A zero sequence number is a perfectly real order id, so the posted size is what
    // signals absence — a decoder keying off the id would invent an order.
    let outcome = clob_engine::OrderOutcome::empty(MatchStop::BookEmpty);
    let decoded = event::decode(&as_instruction_data(&EventBuffer::new(), &outcome, 1)).unwrap();

    assert_eq!(decoded.order_id, None);
    assert_eq!(decoded.stop, MatchStop::BookEmpty);
}

#[test]
fn a_resting_remainder_carries_its_id() {
    let id = clob_book::FIFOOrderId::new(Side::Bid, Ticks(100), 0);
    let outcome = clob_engine::OrderOutcome {
        order_id: Some(id),
        base_lots_posted: BaseLots(5),
        ..clob_engine::OrderOutcome::empty(MatchStop::BookEmpty)
    };

    let decoded = event::decode(&as_instruction_data(&EventBuffer::new(), &outcome, 1)).unwrap();

    assert_eq!(decoded.order_id, Some(id));
    assert_eq!(decoded.base_lots_posted, BaseLots(5));
    assert_eq!(decoded.order_id.unwrap().side(), Side::Bid);
}

#[test]
fn truncation_is_visible_and_the_totals_are_not_affected() {
    let mut buffer = EventBuffer::new();
    for i in 0..MAX_LOGGED_FILLS + 5 {
        buffer.on_fill(&fill(i as u32, 100, 1));
    }
    let outcome = clob_engine::OrderOutcome {
        base_lots_filled: BaseLots((MAX_LOGGED_FILLS + 5) as u64),
        ..clob_engine::OrderOutcome::empty(MatchStop::FullyFilled)
    };

    let decoded = event::decode(&as_instruction_data(&buffer, &outcome, 1)).unwrap();

    assert!(decoded.truncated());
    assert_eq!(decoded.fills_seen as usize, MAX_LOGGED_FILLS + 5);
    assert_eq!(decoded.fills.len(), MAX_LOGGED_FILLS);
    assert_eq!(
        decoded.base_lots_filled,
        BaseLots((MAX_LOGGED_FILLS + 5) as u64)
    );
}

#[test]
fn malformed_payloads_are_refused_rather_than_guessed_at() {
    let outcome = clob_engine::OrderOutcome::empty(MatchStop::BookEmpty);
    let good = as_instruction_data(&EventBuffer::new(), &outcome, 1);

    // Not an event at all.
    assert_eq!(event::decode(&[]), Err(event::EventError::NotAnEvent));
    assert_eq!(event::decode(&[4, 0, 0]), Err(event::EventError::NotAnEvent));

    // Right discriminant, short payload.
    assert_eq!(
        event::decode(&good[..20]),
        Err(event::EventError::TooShort)
    );

    // A version this decoder does not know. Reading it anyway is how a schema change
    // becomes silent data corruption in an index.
    let mut wrong_version = good.clone();
    wrong_version[2] = 99;
    assert_eq!(
        event::decode(&wrong_version),
        Err(event::EventError::UnknownVersion(99))
    );

    let mut wrong_kind = good.clone();
    wrong_kind[3] = 77;
    assert_eq!(
        event::decode(&wrong_kind),
        Err(event::EventError::UnknownKind(77))
    );

    let mut wrong_stop = good.clone();
    wrong_stop[4] = 200;
    assert_eq!(
        event::decode(&wrong_stop),
        Err(event::EventError::UnknownStop(200))
    );

    // Claims more fills than it carries.
    let mut lying = good.clone();
    let count_at = 2 + 60;
    lying[count_at..count_at + 4].copy_from_slice(&99u32.to_le_bytes());
    assert!(matches!(
        event::decode(&lying),
        Err(event::EventError::FillCountMismatch { declared: 99, .. })
    ));
}

#[test]
fn decode_all_picks_events_out_of_a_mixed_instruction_list() {
    // What an indexer actually does: walk every inner instruction and keep the ones
    // addressed to this program's event sink.
    let outcome = clob_engine::OrderOutcome::empty(MatchStop::BookEmpty);
    let event_data = as_instruction_data(&EventBuffer::new(), &outcome, 7);
    let token_transfer = vec![3u8, 0, 0, 0, 0];

    let instructions: Vec<&[u8]> = vec![&token_transfer, &event_data, &token_transfer];
    let decoded = event::decode_all(instructions).unwrap();

    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].taker_seat, 7);
}

// -------------------------------------------------------------------------------------
// Seat indices
//
// An observer resolves the wallet that signed a transaction into a seat index, then
// compares that index against the owner of every order the transaction consumed. If the
// two ever disagree, a self-trade reads as a fill and reported volume becomes something
// anyone can inflate by crossing their own quotes. These check they agree.
// -------------------------------------------------------------------------------------

#[test]
fn a_seat_index_matches_the_orders_that_wallet_owns() {
    let wallets = [TraderKey([1u8; 32]), TraderKey([2u8; 32])];
    let state = MarketState::decode(&laddered()).unwrap();

    for wallet in wallets {
        let seat = state.seat_of(&wallet).expect("wallet holds a seat");

        // Every order carrying this index must be reachable from this wallet, and the
        // wallet must own at least one — otherwise the join proves nothing.
        let owned: Vec<_> = [Side::Bid, Side::Ask]
            .iter()
            .flat_map(|side| state.side(*side))
            .filter(|order| order.trader_index == seat)
            .collect();
        assert!(!owned.is_empty(), "{wallet:?} has no orders at seat {seat}");
    }

    // Two wallets, two distinct seats. A shared index would make them one trader.
    assert_ne!(
        state.seat_of(&wallets[0]).unwrap(),
        state.seat_of(&wallets[1]).unwrap()
    );
}

#[test]
fn every_resting_order_belongs_to_a_claimed_seat() {
    let state = MarketState::decode(&laddered()).unwrap();
    let claimed: Vec<u32> = state.traders.iter().map(|seat| seat.index).collect();

    for side in [Side::Bid, Side::Ask] {
        for order in state.side(side) {
            assert!(
                claimed.contains(&order.trader_index),
                "order {:?} points at seat {} which nobody holds",
                order.id,
                order.trader_index
            );
        }
    }
}

#[test]
fn a_wallet_without_a_seat_resolves_to_nothing() {
    // The observer treats an unresolved trader as "cannot tell", not as seat zero —
    // which would silently attribute someone else's liquidity to them.
    let state = MarketState::decode(&laddered()).unwrap();
    assert_eq!(state.seat_of(&TraderKey([99u8; 32])), None);
}
