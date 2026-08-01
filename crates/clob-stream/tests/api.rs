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
use clob_stream::api::view::{MarketSummary, Window};
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
