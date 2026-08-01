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
use clob_stream::api::view::MarketSummary;
use clob_stream::registry::Registry;
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
