//! Decoding account bytes nobody vouched for.
//!
//! An indexer subscribes by program owner and decodes whatever arrives. It does not get
//! to assume the bytes were written by a correct program — a version skew, a corrupt
//! read, or a future instruction is enough to hand it something the engine would never
//! have produced. Panicking on that is a denial of service on the observer.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::state::MarketState;
use clob_engine::{FeeSchedule, Fill, FillObserver, Market, OrderPacket, TraderKey};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};

type TestMarket = Market<128, 128, 32>;

struct Ignore;
impl FillObserver for Ignore {
    fn on_fill(&mut self, _: &Fill) {}
}

/// A well-formed market with one resting ask, as the program would leave it.
fn account() -> Vec<u8> {
    let mut data = vec![0u8; SizeClass::Small.account_len()];
    let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);

    *bytemuck::from_bytes_mut::<MarketAccountHeader>(header_bytes) = MarketAccountHeader {
        discriminator: MARKET_DISCRIMINATOR,
        version: MARKET_VERSION,
        size_class: SizeClass::Small as u64,
        ..Default::default()
    };

    let market =
        bytemuck::from_bytes_mut::<TestMarket>(&mut market_bytes[..TestMarket::SIZE_IN_BYTES]);
    market
        .initialize(
            LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
            FeeSchedule::new(2).unwrap(),
        )
        .unwrap();

    let seat = market.claim_seat(TraderKey([1u8; 32])).unwrap();
    market
        .deposit(seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
        .unwrap();
    market
        .place_order(
            seat,
            OrderPacket::Limit {
                side: Side::Ask,
                price_in_ticks: Ticks(100),
                num_base_lots: BaseLots(10),
                self_trade_behavior: Default::default(),
                match_limit: 64,
            },
            &mut Ignore,
        )
        .unwrap();
    data
}

/// Overwrites one `u64` of the lot config, which sits at the very start of the market
/// header. This is what a hostile account looks like: structurally valid, arithmetically
/// impossible.
fn corrupt_lot_config(data: &mut [u8], field: usize, value: u64) {
    let offset = HEADER_LEN + field * 8;
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn a_zero_lot_config_is_refused_rather_than_decoded() {
    // base_lots_per_base_unit divides in quote_lots_per_base_lot_per_tick. Decoding a
    // zero there produces a state that panics the first time anyone prices anything.
    for field in 0..4 {
        let mut data = account();
        corrupt_lot_config(&mut data, field, 0);

        assert!(
            MarketState::decode(&data).is_err(),
            "a zero in lot config field {field} must not decode"
        );
    }
}

#[test]
fn a_tick_size_that_breaks_exactness_is_refused() {
    // The invariant every fill's quote value depends on: tick_size must divide by
    // base_lots_per_base_unit. Without it, quote values silently round.
    let mut data = account();
    corrupt_lot_config(&mut data, 1, 1_001);

    assert!(MarketState::decode(&data).is_err());
}

#[test]
fn a_well_formed_account_still_decodes() {
    // The check must reject the impossible without rejecting the ordinary.
    let state = MarketState::decode(&account()).expect("a real market decodes");
    assert_eq!(state.side(Side::Ask).len(), 1);
}

#[test]
fn pricing_a_decoded_market_never_panics() {
    // Whatever survived decoding must be safe to use. This is the property that matters:
    // the indexer prices every fill it derives, on every account it is handed.
    let state = MarketState::decode(&account()).unwrap();

    for size in [0u64, 1, u64::MAX] {
        let _ = state.quote_sweep(Side::Ask, BaseLots(size));
        let _ = state.depth_at_or_better(Side::Ask, Ticks(size));
    }
    let _ = state.mid_price_in_ticks();
    let _ = state.spread_in_ticks();
}

#[test]
fn arbitrary_bytes_never_panic_the_decoder() {
    // Length, alignment and discriminator are all attacker-controlled in the sense that
    // nothing upstream guarantees them.
    for len in [0usize, 1, 7, 8, HEADER_LEN, HEADER_LEN + 1, 1_000, 100_000] {
        let _ = MarketState::decode(&vec![0u8; len]);
        let _ = MarketState::decode(&vec![0xffu8; len]);
    }

    // A valid header with a nonsense size class, and a valid size class on a short body.
    let mut data = account();
    data[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let _ = MarketState::decode(&data);

    let truncated = account();
    for cut in [HEADER_LEN, HEADER_LEN + 100, truncated.len() - 1] {
        let _ = MarketState::decode(&truncated[..cut]);
    }
}
