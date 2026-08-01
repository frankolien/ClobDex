//! The shapes the API puts on the wire, built from real registry state.
//!
//! These test the projections rather than the HTTP plumbing: a handler here is one line
//! that hands a registry to one of these constructors, and it is the constructor that can
//! read the wrong field or drop one. Building the state through the engine — claim a
//! seat, deposit, place orders — means the numbers asserted below were produced the same
//! way the chain produces them, not assembled by hand to match.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::state::MarketState;
use clob_engine::{FeeSchedule, Market, OrderPacket, PostOnlyRejection, TraderKey};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};
use clob_stream::api::view::{MarketSummary, Message, TraderView, Window, levels_of};
use clob_stream::candle;
use clob_stream::registry::Registry;
use clob_stream::store::StoredTrade;
use solana_pubkey::Pubkey;

type TestMarket = Market<128, 128, 32>;

const MARKET: Pubkey = Pubkey::new_from_array([1u8; 32]);
const BASE_MINT: [u8; 32] = [9u8; 32];
const QUOTE_MINT: [u8; 32] = [8u8; 32];

/// A market with two seats and a two-sided book.
///
/// Every quantity below is distinct, so a projection reading the wrong field gets a wrong
/// answer rather than a plausible one.
struct Fixture {
    data: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let mut data = vec![0u8; SizeClass::Small.account_len()];
        let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);

        *bytemuck::from_bytes_mut::<MarketAccountHeader>(header_bytes) = MarketAccountHeader {
            discriminator: MARKET_DISCRIMINATOR,
            version: MARKET_VERSION,
            size_class: SizeClass::Small as u64,
            base_mint: BASE_MINT,
            quote_mint: QUOTE_MINT,
            ..Default::default()
        };

        bytemuck::from_bytes_mut::<TestMarket>(&mut market_bytes[..TestMarket::SIZE_IN_BYTES])
            .initialize(
                LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
                FeeSchedule::new(7).unwrap(),
            )
            .unwrap();
        Self { data }
    }

    fn market(&mut self) -> &mut TestMarket {
        bytemuck::from_bytes_mut(&mut self.data[HEADER_LEN..HEADER_LEN + TestMarket::SIZE_IN_BYTES])
    }

    fn seat(&mut self, id: u8, base: u64, quote: u64) -> u32 {
        let market = self.market();
        let seat = market.claim_seat(TraderKey([id; 32])).unwrap();
        market
            .deposit(seat, BaseLots(base), QuoteLots(quote))
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

    fn state(&self) -> MarketState {
        MarketState::decode(&self.data).unwrap()
    }
}

/// A registry holding one seeded market, as the API sees it.
fn seeded(fixture: &Fixture, slot: u64) -> std::sync::Arc<Registry> {
    let registry = Registry::new();
    registry.seed(MARKET, fixture.state(), slot);
    registry
}

fn summary(registry: &Registry) -> MarketSummary {
    let mut summaries = registry.map_markets(MarketSummary::new);
    assert_eq!(summaries.len(), 1, "the fixture seeds exactly one market");
    summaries.pop().unwrap()
}

#[test]
fn a_summary_carries_what_a_markets_table_renders() {
    let mut fixture = Fixture::new();
    let maker = fixture.seat(1, 5_000, 9_000_000);
    let other = fixture.seat(2, 3_000, 1_000_000);
    fixture.rest(maker, Side::Bid, 98, 10);
    fixture.rest(other, Side::Bid, 97, 5);
    fixture.rest(other, Side::Ask, 102, 7);

    let summary = summary(&seeded(&fixture, 500));

    assert_eq!(summary.market, MARKET.to_string());
    assert_eq!(summary.slot, 500);
    assert_eq!(summary.base_mint, Pubkey::new_from_array(BASE_MINT).to_string());
    assert_eq!(summary.quote_mint, Pubkey::new_from_array(QUOTE_MINT).to_string());
    assert_eq!(summary.taker_fee_bps, 7);

    assert_eq!(summary.best_bid_in_ticks, Some(98));
    assert_eq!(summary.best_ask_in_ticks, Some(102));
    assert_eq!(summary.spread_in_ticks, Some(4));
    assert_eq!(summary.mid_price_in_ticks, Some(100));

    assert_eq!(summary.bid_orders, 2);
    assert_eq!(summary.ask_orders, 1);
    assert_eq!(summary.seats, 2);
    assert_eq!(summary.base_lots_deposited, 8_000);
    assert_eq!(summary.quote_lots_deposited, 10_000_000);

    // Immutable after creation, so a client can cache it against the address and format
    // every price this API returns without another call.
    assert_eq!(summary.lots.base_lots_per_base_unit, 1_000);
    assert_eq!(summary.lots.tick_size_in_quote_lots_per_base_unit, 1_000);
    assert_eq!(summary.lots.base_atoms_per_base_lot, 1_000_000);
    assert_eq!(summary.lots.quote_atoms_per_quote_lot, 1);
}

#[test]
fn an_empty_book_reports_no_price_rather_than_a_price_of_zero() {
    // A market with no liquidity is an ordinary state for a new listing. Reporting zero
    // would put it in a markets table at a price of nothing, above or below every real
    // market depending on the sort.
    let fixture = Fixture::new();
    let summary = summary(&seeded(&fixture, 1));

    assert_eq!(summary.best_bid_in_ticks, None);
    assert_eq!(summary.best_ask_in_ticks, None);
    assert_eq!(summary.spread_in_ticks, None);
    assert_eq!(summary.mid_price_in_ticks, None);
    assert_eq!(summary.last_price_in_ticks, None);
    assert_eq!(summary.bid_orders, 0);
    assert_eq!(summary.ask_orders, 0);
}

#[test]
fn a_one_sided_book_reports_the_side_it_has_and_no_spread() {
    // Half a market is not a market: a spread or a midpoint computed from one side would
    // be invented, and a chart drawn from it would show a price nobody quoted.
    let mut fixture = Fixture::new();
    let maker = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(maker, Side::Bid, 98, 10);

    let summary = summary(&seeded(&fixture, 1));

    assert_eq!(summary.best_bid_in_ticks, Some(98));
    assert_eq!(summary.best_ask_in_ticks, None);
    assert_eq!(summary.spread_in_ticks, None);
    assert_eq!(summary.mid_price_in_ticks, None);
}

/// The wallet behind a fixture seat. `Fixture::seat` claims with `TraderKey([id; 32])`.
fn wallet(id: u8) -> Pubkey {
    Pubkey::new_from_array([id; 32])
}

fn position(registry: &Registry, trader: &Pubkey) -> Option<TraderView> {
    let view = registry.market(&MARKET).expect("the fixture seeds MARKET");
    TraderView::new(&MARKET, trader, &view)
}

#[test]
fn a_position_separates_what_is_free_from_what_is_committed() {
    // The distinction a dashboard exists to show. A wallet that deposited and then quoted
    // still owns everything it deposited; only some of it can be withdrawn or respent,
    // and a balance reported as one number matches neither the vault nor the wallet.
    let mut fixture = Fixture::new();
    let mine = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(mine, Side::Ask, 102, 700);
    fixture.rest(mine, Side::Bid, 98, 10);

    let position = position(&seeded(&fixture, 7), &wallet(1)).expect("seat 1 holds a seat");

    assert_eq!(position.seat, mine);
    assert_eq!(position.trader, wallet(1).to_string());
    assert_eq!(position.slot, 7);

    // 700 base lots are behind the resting ask, the rest is free.
    assert_eq!(position.base_lots_locked, 700);
    assert_eq!(position.base_lots_free, 5_000 - 700);

    // The bid commits quote: 10 base lots at 98 ticks.
    assert_eq!(position.quote_lots_locked, 980);
    assert_eq!(position.quote_lots_free, 9_000_000 - 980);
}

#[test]
fn a_position_lists_only_its_own_orders() {
    // Orders name a seat, and every seat's orders sit in the same two trees. Filtering on
    // the wrong field would show one trader another's book and offer to cancel it.
    let mut fixture = Fixture::new();
    let mine = fixture.seat(1, 5_000, 9_000_000);
    let theirs = fixture.seat(2, 5_000, 9_000_000);
    fixture.rest(mine, Side::Bid, 98, 10);
    fixture.rest(theirs, Side::Bid, 97, 11);
    fixture.rest(theirs, Side::Ask, 103, 12);
    fixture.rest(mine, Side::Ask, 102, 13);

    let registry = seeded(&fixture, 1);
    let mine = position(&registry, &wallet(1)).unwrap();
    let theirs = position(&registry, &wallet(2)).unwrap();

    assert_eq!(mine.orders.len(), 2);
    assert_eq!(theirs.orders.len(), 2);

    let sizes: Vec<u64> = mine.orders.iter().map(|o| o.base_lots).collect();
    assert_eq!(sizes, vec![10, 13], "the bid then the ask");
    assert_eq!(mine.orders[0].side, "bid");
    assert_eq!(mine.orders[0].price_in_ticks, 98);
    assert_eq!(mine.orders[1].side, "ask");
    assert_eq!(mine.orders[1].price_in_ticks, 102);
}

#[test]
fn a_bid_carries_both_the_id_to_cancel_with_and_the_one_to_join_on() {
    // Bids store the complement of the arrival counter, so one ascending comparison gives
    // price-time priority on both sides. A client that had only the decoded number would
    // work perfectly until it cancelled a bid, and then cancel nothing.
    let mut fixture = Fixture::new();
    let mine = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(mine, Side::Bid, 98, 10);
    fixture.rest(mine, Side::Ask, 102, 10);

    let position = position(&seeded(&fixture, 1), &wallet(1)).unwrap();
    let bid = &position.orders[0];
    let ask = &position.orders[1];

    assert_eq!(bid.side, "bid");
    assert_eq!(
        bid.order_sequence_number,
        !bid.sequence_number,
        "a bid stores the complement"
    );
    assert_eq!(ask.side, "ask");
    assert_eq!(
        ask.order_sequence_number, ask.sequence_number,
        "an ask stores the counter as it is"
    );
}

#[test]
fn a_wallet_with_no_seat_has_no_position_rather_than_an_empty_one() {
    // "You have never traded here" and "you are set up and flat" are different answers,
    // and only one of them means a dashboard should offer to claim a seat.
    let mut fixture = Fixture::new();
    fixture.seat(1, 5_000, 9_000_000);

    let registry = seeded(&fixture, 1);
    assert!(position(&registry, &wallet(1)).is_some());
    assert!(position(&registry, &wallet(99)).is_none());
}

#[test]
fn a_seat_that_withdrew_everything_still_has_a_position() {
    let mut fixture = Fixture::new();
    fixture.seat(1, 0, 0);

    let position = position(&seeded(&fixture, 1), &wallet(1)).expect("a claimed seat");
    assert_eq!(position.base_lots_free, 0);
    assert_eq!(position.base_lots_locked, 0);
    assert_eq!(position.quote_lots_free, 0);
    assert_eq!(position.quote_lots_locked, 0);
    assert!(position.orders.is_empty());
}

#[test]
fn an_update_carries_a_book_a_client_can_replace_rather_than_patch() {
    // Without levels on the update, a subscriber's ladder is correct exactly once — at
    // the snapshot — and then silently stops moving while trades keep printing past it.
    // The whole top of book is sent, so a client assigns rather than applies a diff:
    // patching is a second implementation of the book, and one that only goes wrong after
    // some unpredictable sequence is the hardest kind to notice.
    let mut fixture = Fixture::new();
    let maker = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(maker, Side::Bid, 98, 10);
    fixture.rest(maker, Side::Ask, 102, 7);
    let state = fixture.state();

    let snapshot = Message::Snapshot {
        market: MARKET.to_string(),
        slot: 1,
        finalized_through: 0,
        bids: levels_of(&state, Side::Bid, 50),
        asks: levels_of(&state, Side::Ask, 50),
    };
    let update = Message::Update {
        slot: 2,
        trades: Vec::new(),
        bids: levels_of(&state, Side::Bid, 50),
        asks: levels_of(&state, Side::Ask, 50),
        best_bid: Some(98),
        best_ask: Some(102),
        finalized_through: 0,
    };

    let snapshot = serde_json::to_value(&snapshot).unwrap();
    let update = serde_json::to_value(&update).unwrap();

    // The same book described the same way, so a client has one parser for both.
    assert_eq!(snapshot["bids"], update["bids"]);
    assert_eq!(snapshot["asks"], update["asks"]);
    assert_eq!(update["bids"][0]["price_in_ticks"], "98");
    assert_eq!(update["bids"][0]["base_lots"], "10");
    assert_eq!(update["asks"][0]["price_in_ticks"], "102");
    assert_eq!(update["type"], "update");
    assert_eq!(snapshot["type"], "snapshot");
}

#[test]
fn an_order_identity_crosses_the_wire_as_a_string() {
    // The reason quantities are not JSON numbers. A bid's stored sequence number is the
    // complement of the arrival counter, so it sits just below u64::MAX — far above the
    // 2^53 where a double stops holding consecutive integers.
    let mut fixture = Fixture::new();
    let mine = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(mine, Side::Bid, 98, 10);

    let position = position(&seeded(&fixture, 1), &wallet(1)).unwrap();
    let exact = position.orders[0].order_sequence_number;
    assert!(exact > 1 << 53, "a bid identity is near u64::MAX, got {exact}");

    // What a JSON number would have done to it. Compared in i128 rather than by casting
    // back to u64: a u64 -> f64 -> u64 round trip saturates at the top of the range and
    // lands back on the same value, hiding the loss that a JSON parser does not hide.
    assert_ne!(
        i128::from(exact),
        exact as f64 as i128,
        "the nearest double to an order identity must not be that identity"
    );

    let json = serde_json::to_value(&position).unwrap();
    let encoded = json["orders"][0]["order_sequence_number"]
        .as_str()
        .expect("an identity must be a string");
    assert_eq!(encoded, exact.to_string());
    assert_eq!(encoded.parse::<u64>().unwrap(), exact);
}

#[test]
fn money_is_a_string_and_coordinates_are_numbers() {
    // Quoting a slot or a seat would buy no precision — both are bounded far below 2^53 —
    // and would put a conversion at every call site that passes one back as a query
    // parameter.
    let mut fixture = Fixture::new();
    let mine = fixture.seat(1, 5_000, 9_000_000);
    fixture.rest(mine, Side::Bid, 98, 10);

    let position = serde_json::to_value(position(&seeded(&fixture, 7), &wallet(1)).unwrap()).unwrap();
    assert!(position["base_lots_free"].is_string());
    assert!(position["quote_lots_locked"].is_string());
    assert!(position["orders"][0]["price_in_ticks"].is_string());
    assert!(position["slot"].is_number(), "a slot is a coordinate");
    assert!(position["seat"].is_number(), "a seat index is a coordinate");

    let summary = serde_json::to_value(summary(&seeded(&fixture, 7))).unwrap();
    assert!(summary["best_bid_in_ticks"].is_string());
    assert!(summary["base_lots_deposited"].is_string());
    assert!(summary["lots"]["base_atoms_per_base_lot"].is_string());
    assert!(summary["taker_fee_bps"].is_number(), "bps is bounded by 10,000");
    assert!(summary["bid_orders"].is_number(), "a count is a tally");
    assert!(summary["trades_seen"].is_number(), "a count is a tally");
}

#[test]
fn an_absent_price_stays_null_rather_than_becoming_a_quoted_zero() {
    // A quoted "0" would parse to a real price of zero, which is the failure the optional
    // types exist to prevent — now with an extra step for it to hide behind.
    let fixture = Fixture::new();
    let summary = serde_json::to_value(summary(&seeded(&fixture, 1))).unwrap();

    assert!(summary["best_bid_in_ticks"].is_null());
    assert!(summary["spread_in_ticks"].is_null());
    assert!(summary["last_price_in_ticks"].is_null());

    let window = serde_json::to_value(Window::new(&MARKET, 1, 100, &[], false)).unwrap();
    assert!(window["open_in_ticks"].is_null());
    assert!(window["change_in_ticks"].is_null());
    assert!(window["base_lots"].is_string(), "a total of nothing is still money");
    assert_eq!(window["base_lots"], "0");
}

#[test]
fn a_negative_change_keeps_its_sign_as_a_string() {
    let trades = [stored(10, 120, 1), stored(20, 80, 1)];
    let window = serde_json::to_value(Window::new(&MARKET, 1, 20, &trades, false)).unwrap();

    assert_eq!(window["change_in_ticks"], "-40");
}

/// A stored fill, for the window projections.
fn stored(slot: u64, price: u64, size: u64) -> StoredTrade {
    StoredTrade {
        market: MARKET,
        slot,
        signature: [slot as u8; 64],
        price_in_ticks: price,
        base_lots: size,
        quote_lots: price * size,
        maker_seat: 1,
        taker_seat: Some(2),
        maker_order_sequence: slot,
        taker_side_is_bid: true,
    }
}

#[test]
fn a_window_reports_the_range_the_volume_and_the_change() {
    let trades = [
        stored(10, 100, 3),
        stored(20, 130, 1),
        stored(30, 90, 2),
        stored(40, 110, 4),
    ];
    let window = Window::new(&MARKET, 1, 40, &trades, false);

    assert_eq!(window.from_slot, 1);
    assert_eq!(window.to_slot, 40);
    assert_eq!(window.slots, 40, "both ends are inclusive");

    assert_eq!(window.open_in_ticks, Some(100), "first trade, not lowest");
    assert_eq!(window.close_in_ticks, Some(110), "last trade, not highest");
    assert_eq!(window.high_in_ticks, Some(130));
    assert_eq!(window.low_in_ticks, Some(90));
    assert_eq!(window.change_in_ticks, Some(10));

    assert_eq!(window.base_lots, 10);
    assert_eq!(window.quote_lots, 300 + 130 + 180 + 440);
    assert_eq!(window.trades, 4);
    assert!(!window.truncated);
}

#[test]
fn a_window_on_a_falling_market_reports_a_negative_change() {
    // The reason the field is signed. An unsigned change would wrap a fall into an
    // enormous rise, which is the single most misleading number a markets table can show.
    let trades = [stored(10, 120, 1), stored(20, 80, 1)];
    let window = Window::new(&MARKET, 1, 20, &trades, false);

    assert_eq!(window.change_in_ticks, Some(-40));
}

#[test]
fn a_window_with_no_trades_reports_no_price_and_no_volume() {
    let window = Window::new(&MARKET, 1, 216_000, &[], false);

    assert_eq!(window.open_in_ticks, None);
    assert_eq!(window.close_in_ticks, None);
    assert_eq!(window.change_in_ticks, None);
    assert_eq!(window.vwap_in_ticks, None);
    assert_eq!(window.base_lots, 0);
    assert_eq!(window.quote_lots, 0);
    assert_eq!(window.trades, 0);
}

#[test]
fn a_window_agrees_with_a_candle_covering_the_same_trades() {
    // The window reuses the candle aggregation rather than folding trades a second time.
    // This is the claim that makes that worth doing: open, close, high and low mean the
    // same thing in a rolling statistic as they do in a bar on a chart.
    let trades = [
        stored(10, 100, 3),
        stored(11, 130, 1),
        stored(12, 90, 2),
        stored(13, 110, 4),
    ];
    let bars = candle::aggregate(&trades, 1_000);
    assert_eq!(bars.len(), 1, "all four fall in one bucket");

    let window = Window::new(&MARKET, 0, 999, &trades, false);
    assert_eq!(window.open_in_ticks, Some(bars[0].open));
    assert_eq!(window.high_in_ticks, Some(bars[0].high));
    assert_eq!(window.low_in_ticks, Some(bars[0].low));
    assert_eq!(window.close_in_ticks, Some(bars[0].close));
    assert_eq!(window.base_lots, bars[0].base_lots);
    assert_eq!(window.quote_lots, bars[0].quote_lots);
    assert_eq!(window.trades, bars[0].trades);
}

#[test]
fn a_window_collapses_trades_from_slots_far_apart_into_one() {
    // The bucketing that `aggregate` does must not survive into a window: slots 1 and
    // 300,000 belong to the same 24-hour statistic and to different bars.
    let trades = [stored(1, 100, 1), stored(300_000, 200, 1)];
    let summary = candle::summarise(&trades).expect("two trades summarise");

    assert_eq!(summary.trades, 2);
    assert_eq!(summary.open, 100);
    assert_eq!(summary.close, 200);
    assert!(
        candle::aggregate(&trades, 1_000).len() > 1,
        "the same trades are several bars"
    );
}

#[test]
fn summarising_every_market_copies_none_of_them() {
    // The guarantee that makes this endpoint cheap enough to poll: the mapping runs under
    // the read lock and sees a borrow, so adding a market cannot start cloning books.
    let fixture = Fixture::new();
    let registry = seeded(&fixture, 1);
    registry.seed(Pubkey::new_from_array([2u8; 32]), fixture.state(), 1);

    let borrowed: Vec<usize> = registry.map_markets(|_, view| view.state.traders.len());
    assert_eq!(borrowed.len(), 2);
    assert_eq!(registry.map_markets(MarketSummary::new).len(), 2);
}
