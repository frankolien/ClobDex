//! The streaming path, driven end to end without a network.
//!
//! Ground truth is the engine. Each scenario runs orders through `clob-engine` with a
//! fill observer attached, so what happened is known exactly; the account bytes on
//! either side are then fed through the correlator and the pipeline — which have no
//! access to that observer — and the derived trades are compared against it.
//!
//! What this covers is everything between "an update arrived" and "a delta was derived".
//! What it cannot cover is the endpoint itself, which is why the endpoint is the only
//! thing on the other side of the `Source` trait.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::instruction::{self, MarketAddresses, Receipt};
use clob_engine::{FeeSchedule, FillObserver, Market, OrderPacket, TraderKey};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};
use clob_stream::correlate::{Correlator, PENDING_CAPACITY};
use clob_stream::pipeline::{self, Outcome};
use clob_stream::source::{RawInstruction, Source, Update};
use solana_pubkey::Pubkey;

type TestMarket = Market<128, 128, 32>;

const PROGRAM: Pubkey = Pubkey::new_from_array([9u8; 32]);
const MARKET: Pubkey = Pubkey::new_from_array([1u8; 32]);

fn wallet(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

/// A market account laid out exactly as the program leaves it.
struct Chain {
    data: Vec<u8>,
}

impl Chain {
    fn new(taker_fee_bps: u64) -> Self {
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
        Self { data }
    }

    fn market_mut(&mut self) -> &mut TestMarket {
        let market_bytes = &mut self.data[HEADER_LEN..];
        bytemuck::from_bytes_mut::<TestMarket>(&mut market_bytes[..TestMarket::SIZE_IN_BYTES])
    }

    fn bytes(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// Seats a wallet and funds it generously.
    fn seat(&mut self, key: Pubkey) -> u32 {
        let market = self.market_mut();
        let seat = market.claim_seat(TraderKey(key.to_bytes())).unwrap();
        market
            .deposit(seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
            .unwrap();
        seat
    }
}

/// Collects the fills the engine actually produced.
#[derive(Default)]
struct Truth {
    fills: Vec<(u64, u64)>,
}

impl FillObserver for Truth {
    fn on_fill(&mut self, fill: &clob_engine::Fill) {
        self.fills
            .push((fill.price_in_ticks.as_u64(), fill.base_lots_filled.as_u64()));
    }
}

/// Builds the raw instruction the SDK would have produced for an order.
fn order_instruction(trader: Pubkey, packet: &OrderPacket) -> RawInstruction {
    let addresses = MarketAddresses::new(PROGRAM, MARKET, wallet(20), wallet(21));
    let built = instruction::place_order(&addresses, &trader, packet, Receipt::Off);
    RawInstruction {
        program_id: built.program_id,
        accounts: built.accounts.iter().map(|meta| meta.pubkey).collect(),
        data: built.data,
    }
}

fn signature(byte: u8) -> [u8; 64] {
    [byte; 64]
}

/// Runs a taker order and returns (before, after, instruction, what really happened).
fn cross(taker: Pubkey, packet: OrderPacket, chain: &mut Chain) -> (Vec<u8>, Vec<u8>, RawInstruction, Truth) {
    let before = chain.bytes();
    let mut truth = Truth::default();
    let seat = chain
        .market_mut()
        .traders()
        .index_of(&TraderKey(taker.to_bytes()));
    chain
        .market_mut()
        .place_order(seat, packet, &mut truth)
        .unwrap();
    let after = chain.bytes();
    (before, after, order_instruction(taker, &packet), truth)
}

// -------------------------------------------------------------------------------------

#[test]
fn a_fill_between_two_wallets_is_derived_from_the_bytes_alone() {
    let (maker, taker) = (wallet(2), wallet(3));
    let mut chain = Chain::new(2);
    chain.seat(maker);
    chain.seat(taker);

    // The maker rests an ask, then the taker crosses it.
    let maker_seat = chain.market_mut().traders().index_of(&TraderKey(maker.to_bytes()));
    chain
        .market_mut()
        .place_order(
            maker_seat,
            instruction::limit(Side::Ask, Ticks(100), BaseLots(50)),
            &mut Truth::default(),
        )
        .unwrap();

    let (before, after, ix, truth) = cross(
        taker,
        instruction::limit(Side::Bid, Ticks(100), BaseLots(30)),
        &mut chain,
    );
    assert_eq!(truth.fills, vec![(100, 30)], "the engine really did fill");

    let mut correlator = Correlator::new();
    correlator.seed(MARKET, before);
    let change = correlator
        .accept(Update::Transaction {
            slot: 7,
            signature: signature(1),
            succeeded: true,
            instructions: vec![ix],
        })
        .or_else(|| {
            correlator.accept(Update::Account {
                slot: 7,
                market: MARKET,
                data: after,
                signature: Some(signature(1)),
            })
        })
        .expect("the pair completed");

    let Outcome::Derived(derived) = pipeline::process(&change, &PROGRAM).unwrap() else {
        panic!("expected a derived change");
    };
    let trades: Vec<(u64, u64)> = derived
        .delta
        .trades
        .iter()
        .map(|t| (t.price_in_ticks.as_u64(), t.base_lots.as_u64()))
        .collect();

    assert_eq!(trades, truth.fills, "derived tape matches what the engine did");
    assert!(pipeline::reconciles(&derived), "fees agree with the trades");
}

#[test]
fn a_self_trade_is_not_reported_as_volume() {
    // The case that only ownership can distinguish: liquidity leaves the side opposite
    // the taker exactly as in a fill, because that is why it crossed.
    let solo = wallet(2);
    let mut chain = Chain::new(2);
    chain.seat(solo);

    let seat = chain.market_mut().traders().index_of(&TraderKey(solo.to_bytes()));
    chain
        .market_mut()
        .place_order(
            seat,
            instruction::limit(Side::Ask, Ticks(100), BaseLots(50)),
            &mut Truth::default(),
        )
        .unwrap();

    let (before, after, ix, truth) = cross(
        solo,
        instruction::limit(Side::Bid, Ticks(100), BaseLots(30)),
        &mut chain,
    );
    assert!(truth.fills.is_empty(), "no value changed hands");

    let mut correlator = Correlator::new();
    correlator.seed(MARKET, before);
    correlator.accept(Update::Transaction {
        slot: 8,
        signature: signature(2),
        succeeded: true,
        instructions: vec![ix],
    });
    let change = correlator
        .accept(Update::Account {
            slot: 8,
            market: MARKET,
            data: after,
            signature: Some(signature(2)),
        })
        .expect("the pair completed");

    let Outcome::Derived(derived) = pipeline::process(&change, &PROGRAM).unwrap() else {
        panic!("expected a derived change");
    };
    assert!(
        derived.delta.trades.is_empty(),
        "a self-trade must never be reported as volume: {:?}",
        derived.delta.trades
    );
    // The liquidity did leave the book, and saying otherwise would be equally wrong.
    assert!(!derived.delta.removals.is_empty(), "the removal is still recorded");
    assert!(pipeline::reconciles(&derived), "no fee was charged and none is implied");
}

#[test]
fn the_halves_pair_in_either_order() {
    // The cluster does not promise that a transaction arrives before the account write
    // it caused, and either order has to produce the same change.
    for transaction_first in [true, false] {
        let mut correlator = Correlator::new();
        correlator.seed(MARKET, vec![1u8; 4]);

        let account = Update::Account {
            slot: 5,
            market: MARKET,
            data: vec![2u8; 4],
            signature: Some(signature(3)),
        };
        let transaction = Update::Transaction {
            slot: 5,
            signature: signature(3),
            succeeded: true,
            instructions: Vec::new(),
        };

        let (first, second) = match transaction_first {
            true => (transaction, account),
            false => (account, transaction),
        };
        assert!(correlator.accept(first).is_none(), "one half is not enough");

        let change = correlator.accept(second).expect("the second half completes it");
        assert_eq!(change.before.as_deref(), Some(&[1u8; 4][..]));
        assert_eq!(change.after, vec![2u8; 4]);
        assert_eq!(correlator.pending(), 0, "nothing was left behind");
    }
}

#[test]
fn the_first_update_for_a_market_only_establishes_a_baseline() {
    // Otherwise every restart would publish the entire resting book as newly posted.
    let mut correlator = Correlator::new();
    correlator.accept(Update::Transaction {
        slot: 1,
        signature: signature(4),
        succeeded: true,
        instructions: Vec::new(),
    });
    let change = correlator
        .accept(Update::Account {
            slot: 1,
            market: MARKET,
            data: vec![3u8; 4],
            signature: Some(signature(4)),
        })
        .expect("a change is produced");

    assert!(change.before.is_none(), "there was nothing to diff against");
    // The market is still reported, so it becomes visible before it has traded — but
    // nothing is derived from a state with nothing to compare it to.
    assert!(
        matches!(
            pipeline::process(&change, &PROGRAM),
            Err(_) | Ok(Outcome::Baseline { .. })
        ),
        "a first sighting is a baseline, never a derived change"
    );
}

#[test]
fn a_failed_transaction_is_dropped_rather_than_buffered() {
    // It wrote nothing, so no account update is coming and the pairing never completes.
    let mut correlator = Correlator::new();
    assert!(
        correlator
            .accept(Update::Transaction {
                slot: 2,
                signature: signature(5),
                succeeded: false,
                instructions: Vec::new(),
            })
            .is_none()
    );
    assert_eq!(correlator.pending(), 0, "a failed transaction holds no memory");
}

#[test]
fn an_unattributed_snapshot_updates_the_baseline_without_deriving() {
    let mut correlator = Correlator::new();
    assert!(
        correlator
            .accept(Update::Account {
                slot: 3,
                market: MARKET,
                data: vec![4u8; 4],
                signature: None,
            })
            .is_none(),
        "nothing can be derived from a state that arrived unexplained"
    );
    assert_eq!(correlator.latest(&MARKET), Some(&[4u8; 4][..]));
}

#[test]
fn unmatched_halves_cannot_grow_without_bound() {
    // A stream that never delivers counterparts must cost a bounded amount of memory.
    let mut correlator = Correlator::new();
    for i in 0..(PENDING_CAPACITY + 500) {
        let mut sig = [0u8; 64];
        sig[..8].copy_from_slice(&(i as u64).to_le_bytes());
        correlator.accept(Update::Transaction {
            slot: i as u64,
            signature: sig,
            succeeded: true,
            instructions: Vec::new(),
        });
    }
    assert_eq!(correlator.pending(), PENDING_CAPACITY);
}

#[test]
fn the_tip_advances_on_every_kind_of_update() {
    // Slot updates are the only thing that arrives while no market is trading, and
    // without them the stream looks stalled.
    let mut correlator = Correlator::new();
    correlator.accept(Update::Slot { slot: 42 });
    assert_eq!(correlator.tip(), 42);

    correlator.accept(Update::Slot { slot: 41 });
    assert_eq!(correlator.tip(), 42, "the tip never goes backwards");
}

#[tokio::test]
async fn a_replay_source_drives_the_correlator() {
    let mut source = clob_stream::source::Replay::new([
        Update::Slot { slot: 1 },
        Update::Account {
            slot: 1,
            market: MARKET,
            data: vec![5u8; 4],
            signature: None,
        },
    ]);
    let mut correlator = Correlator::new();
    while let Some(update) = source.next().await {
        correlator.accept(update);
    }

    assert_eq!(correlator.tip(), 1);
    assert_eq!(correlator.latest(&MARKET), Some(&[5u8; 4][..]));
}
