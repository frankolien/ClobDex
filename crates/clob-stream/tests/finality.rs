//! Rollbacks.
//!
//! Indexing at `confirmed` sees a trade about a slot sooner than `finalized` does, and
//! accepts that the slot can still be abandoned. A tape that never takes those trades
//! back reports volume that did not happen — which is the one number a venue is most
//! tempted to get wrong, so it is worth proving it cannot happen by accident.

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_client::state::MarketState;
use clob_engine::{FeeSchedule, Market, TraderKey};
use clob_indexer::{BookDelta, Trade};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};
use clob_stream::pipeline::Derived;
use clob_stream::registry::{Event, Registry};
use solana_pubkey::Pubkey;

type TestMarket = Market<128, 128, 32>;

const MARKET: Pubkey = Pubkey::new_from_array([1u8; 32]);
const OTHER_MARKET: Pubkey = Pubkey::new_from_array([2u8; 32]);

/// A decodable market with one seat, so the registry has real state to hold.
fn state() -> MarketState {
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
            clob_book::LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
            FeeSchedule::new(2).unwrap(),
        )
        .unwrap();
    let seat = market.claim_seat(TraderKey([5u8; 32])).unwrap();
    market
        .deposit(seat, BaseLots(1_000), QuoteLots(1_000_000))
        .unwrap();

    MarketState::decode(&data).unwrap()
}

fn trade(slot: u64, price: u64) -> Trade {
    Trade {
        slot,
        price_in_ticks: Ticks(price),
        base_lots: BaseLots(10),
        quote_lots: QuoteLots(price * 10),
        maker_order_id: clob_book::FIFOOrderId::new(Side::Ask, Ticks(price), slot),
        maker_seat: 1,
        taker_seat: Some(2),
        taker_side: Side::Bid,
    }
}

/// A derived change carrying `trades`, as ingest would hand it over.
fn derived(market: Pubkey, slot: u64, trades: Vec<Trade>) -> Derived {
    Derived {
        market,
        slot,
        signature: [slot as u8; 64],
        delta: BookDelta {
            trades,
            ..Default::default()
        },
        state: state(),
    }
}

// -------------------------------------------------------------------------------------

#[test]
fn an_abandoned_slot_takes_its_trades_back() {
    let registry = Registry::new();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);
    registry.apply(derived(MARKET, 11, vec![trade(11, 101)]), true);

    assert_eq!(registry.market(&MARKET).unwrap().tape.len(), 2);

    registry.retract(10);

    let view = registry.market(&MARKET).unwrap();
    assert_eq!(view.tape.len(), 1, "only slot 10's trade goes");
    assert_eq!(view.tape[0].slot, 11, "the surviving slot is untouched");
    assert_eq!(view.trades_retracted, 1);
}

#[test]
fn a_retraction_is_published_rather_than_applied_quietly() {
    // A client that already showed the trade has to be told. Silence looks exactly like
    // a market where nothing happened.
    let registry = Registry::new();
    let mut feed = registry.subscribe();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);

    assert!(matches!(feed.try_recv(), Ok(Event::Change(_))));

    registry.retract(10);
    match feed.try_recv() {
        Ok(Event::Retracted {
            market,
            slot,
            trades,
        }) => {
            assert_eq!(market, MARKET);
            assert_eq!(slot, 10);
            assert_eq!(trades, 1);
        }
        other => panic!("expected a retraction, got something else: {}", matches_name(&other)),
    }
}

#[test]
fn retracting_a_slot_that_produced_nothing_says_nothing() {
    // Most slots are irrelevant to any given market, and a message per dropped slot per
    // subscriber would drown the feed.
    let registry = Registry::new();
    let mut feed = registry.subscribe();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);
    let _ = feed.try_recv();

    registry.retract(999);
    assert!(feed.try_recv().is_err(), "nothing to say about an unrelated slot");
    assert_eq!(registry.market(&MARKET).unwrap().trades_retracted, 0);
}

#[test]
fn a_rollback_only_touches_the_markets_that_traded_in_that_slot() {
    let registry = Registry::new();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);
    registry.apply(derived(OTHER_MARKET, 11, vec![trade(11, 200)]), true);

    registry.retract(10);

    assert!(registry.market(&MARKET).unwrap().tape.is_empty());
    assert_eq!(
        registry.market(&OTHER_MARKET).unwrap().tape.len(),
        1,
        "a market that did not trade in the dropped slot is unaffected"
    );
}

#[test]
fn finality_advances_and_never_goes_backwards() {
    // Slot updates can arrive out of order, and a finality marker that moved backwards
    // would tell a consumer a rooted trade is provisional again.
    let registry = Registry::new();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);

    registry.finalize(50);
    assert_eq!(registry.market(&MARKET).unwrap().finalized_through, 50);

    registry.finalize(40);
    assert_eq!(
        registry.market(&MARKET).unwrap().finalized_through,
        50,
        "a later marker for an earlier slot changes nothing"
    );
}

#[test]
fn retraction_and_publication_are_reported_separately() {
    // Netting them would hide the rollback entirely: two published minus one retracted
    // reads identically to one published and nothing wrong.
    let registry = Registry::new();
    registry.apply(derived(MARKET, 10, vec![trade(10, 100)]), true);
    registry.apply(derived(MARKET, 11, vec![trade(11, 101)]), true);
    registry.retract(10);

    let view = registry.market(&MARKET).unwrap();
    assert_eq!(view.trades_seen, 2, "both were published");
    assert_eq!(view.trades_retracted, 1, "and one was taken back");
}

#[test]
fn a_dead_slot_does_not_advance_the_tip() {
    // Nothing it wrote survived, so counting it as progress would claim coverage the
    // stream does not have.
    use clob_stream::correlate::Correlator;
    use clob_stream::source::{SlotStatus, Update};

    let mut correlator = Correlator::new();
    correlator.accept(Update::Slot {
        slot: 100,
        status: SlotStatus::Confirmed,
    });
    correlator.accept(Update::Slot {
        slot: 101,
        status: SlotStatus::Dead,
    });

    assert_eq!(correlator.tip(), 100);
}

#[test]
fn every_trade_in_a_dropped_slot_goes_not_just_the_first() {
    let registry = Registry::new();
    registry.apply(
        derived(MARKET, 10, vec![trade(10, 100), trade(10, 101), trade(10, 102)]),
        true,
    );
    registry.apply(derived(MARKET, 11, vec![trade(11, 103)]), true);

    registry.retract(10);

    let view = registry.market(&MARKET).unwrap();
    assert_eq!(view.tape.len(), 1);
    assert_eq!(view.trades_retracted, 3);
}

fn matches_name(event: &Result<Event, tokio::sync::broadcast::error::TryRecvError>) -> &'static str {
    match event {
        Ok(Event::Change(_)) => "a change",
        Ok(Event::Retracted { .. }) => "a retraction",
        Err(_) => "nothing",
    }
}
