//! The conserve-funds campaign: arbitrary instruction sequences against the real binary.
//!
//! The engine already has property tests for conservation and they are thorough. They run
//! against a `Market` in memory, where a deposit is a number going up and the instruction
//! layer does not exist. Everything between a transaction and that number is untested by
//! them: the discriminant, the reader, the account list, the signer checks, and the token
//! CPI that actually moves the money.
//!
//! This closes that. Mollusk executes the compiled SBF binary, the vaults are real SPL
//! token accounts, and the market starts empty — every byte of state the campaign reaches
//! was produced by an instruction the program ran.
//!
//! # The claim
//!
//! After every instruction, landed or rejected:
//!
//! 1. The market is internally consistent.
//! 2. **The vault holds exactly what the market says it owes.** Books that balance are
//!    not the same as money that is there.
//! 3. No token was created or destroyed.
//!
//! And at the end of every sequence, the invariant the roadmap names: cancel everything,
//! withdraw everything, and every atom that went in comes back out.
//!
//! # Running it longer
//!
//! The default is sized to run on every `cargo test` — a campaign nobody runs finds
//! nothing. For a real one:
//!
//! ```text
//! PROPTEST_CASES=5000 cargo test --test fuzz_campaign --release
//! ```
//!
//! A failure prints the shrunk sequence: proptest reduces it to the shortest run of
//! instructions that still breaks the invariant, which is usually short enough to read.

mod common;

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};
use clob_client::instruction::{self as sdk, Receipt};
use clob_engine::{OrderPacket, PostOnlyRejection};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

use common::world::World;

/// Wallets in the market. Three is the smallest number that produces all three
/// relationships that matter: two traders on opposite sides, and a third whose orders are
/// somebody else's to neither of them.
const TRADERS: usize = 3;

/// Taker fee for the campaign. Non-zero, because a fee is value that leaves a trader's
/// balance without leaving the market, and that is exactly the kind of movement an
/// accounting bug hides in.
const TAKER_FEE_BPS: u64 = 5;

/// Longest sequence the generator will produce.
///
/// This is what bounds book depth — a book only gets as deep as one sequence has time to
/// build it — so it is the number to raise for deeper states, not the case count.
const OPS_PER_CASE: usize = 150;

/// Sequences run when `PROPTEST_CASES` is not set. Small enough to belong in the ordinary
/// test run; `PROPTEST_CASES` is how a real campaign is scaled.
const DEFAULT_CASES: u32 = 12;

/// One instruction, before it knows which market it is for.
#[derive(Debug, Clone)]
enum Op {
    ClaimSeat { trader: usize },
    EvictSeat { trader: usize },
    Deposit { trader: usize, base: u64, quote: u64 },
    Withdraw { trader: usize, base: u64, quote: u64 },
    Limit { trader: usize, side: Side, price: u64, size: u64 },
    PostOnly { trader: usize, side: Side, price: u64, size: u64, slide: bool },
    Ioc { trader: usize, side: Side, size: u64 },
    Cancel { trader: usize, pick: usize },
    CancelAll { trader: usize, side: Side, limit: u32 },
    Reduce { trader: usize, pick: usize, size: u64 },
    Batch { trader: usize, cancels: usize, orders: Vec<(Side, u64, u64)> },
    Swap { trader: usize, side: Side, price: u64, size: u64 },
    CollectFees,
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Bid), Just(Side::Ask)]
}

/// Where takers aim: a narrow band, so orders actually meet.
///
/// A uniform price over a wide range produces a book that never crosses, and a campaign
/// that never crosses never tests matching, fees, or settlement — the three places value
/// moves.
fn crossing_price() -> impl Strategy<Value = u64> {
    95u64..106
}

/// Where makers quote: wide enough that most of it rests instead of trading.
///
/// Two bands rather than one, because one band gives a book that is never deep. Measured
/// on a single-band version, the book peaked at ten orders across both sides, so a
/// wind-down never faced anything a bounded cancel-all would have to loop over — and the
/// depth a venue has to survive is exactly the depth nothing had tested.
fn resting_price() -> impl Strategy<Value = u64> {
    80u64..121
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        1 => (0..TRADERS).prop_map(|trader| Op::ClaimSeat { trader }),
        1 => (0..TRADERS).prop_map(|trader| Op::EvictSeat { trader }),
        4 => (0..TRADERS, 0u64..500, 0u64..500_000)
            .prop_map(|(trader, base, quote)| Op::Deposit { trader, base, quote }),
        2 => (0..TRADERS, 0u64..200, 0u64..200_000)
            .prop_map(|(trader, base, quote)| Op::Withdraw { trader, base, quote }),
        6 => (0..TRADERS, side(), crossing_price(), 1u64..40)
            .prop_map(|(trader, side, price, size)| Op::Limit { trader, side, price, size }),
        8 => (0..TRADERS, side(), resting_price(), 1u64..40, any::<bool>())
            .prop_map(|(trader, side, price, size, slide)| Op::PostOnly {
                trader, side, price, size, slide
            }),
        3 => (0..TRADERS, side(), 1u64..40)
            .prop_map(|(trader, side, size)| Op::Ioc { trader, side, size }),
        3 => (0..TRADERS, any::<prop::sample::Index>())
            .prop_map(|(trader, i)| Op::Cancel { trader, pick: i.index(usize::MAX) }),
        2 => (0..TRADERS, side(), 1u32..24)
            .prop_map(|(trader, side, limit)| Op::CancelAll { trader, side, limit }),
        2 => (0..TRADERS, any::<prop::sample::Index>(), 1u64..40)
            .prop_map(|(trader, i, size)| Op::Reduce { trader, pick: i.index(usize::MAX), size }),
        5 => (
            0..TRADERS,
            0usize..5,
            prop::collection::vec((side(), resting_price(), 1u64..40), 0..5),
        )
            .prop_map(|(trader, cancels, orders)| Op::Batch { trader, cancels, orders }),
        4 => (0..TRADERS, side(), crossing_price(), 1u64..40)
            .prop_map(|(trader, side, price, size)| Op::Swap { trader, side, price, size }),
        1 => Just(Op::CollectFees),
    ]
}

/// Builds the instruction an op describes and runs it. Returns whether it landed.
fn apply(world: &mut World, op: &Op) -> bool {
    let addresses = world.addresses();
    let keys = |world: &World, index: usize| {
        let trader = &world.traders()[index];
        (trader.wallet, trader.base, trader.quote)
    };

    match op {
        Op::ClaimSeat { trader } => {
            let (wallet, _, _) = keys(world, *trader);
            world.execute(&sdk::claim_seat(&addresses, &wallet))
        }
        Op::EvictSeat { trader } => {
            let (wallet, _, _) = keys(world, *trader);
            world.execute(&sdk::evict_seat(&addresses, &wallet))
        }
        Op::Deposit { trader, base, quote } => {
            let (wallet, token_base, token_quote) = keys(world, *trader);
            world.execute(&sdk::deposit(
                &addresses,
                &wallet,
                &token_base,
                &token_quote,
                BaseLots(*base),
                QuoteLots(*quote),
            ))
        }
        Op::Withdraw { trader, base, quote } => {
            let (wallet, token_base, token_quote) = keys(world, *trader);
            world.execute(&sdk::withdraw(
                &addresses,
                &wallet,
                &token_base,
                &token_quote,
                BaseLots(*base),
                QuoteLots(*quote),
            ))
        }
        Op::Limit { trader, side, price, size } => {
            let (wallet, _, _) = keys(world, *trader);
            let packet = sdk::limit(*side, Ticks(*price), BaseLots(*size));
            world.execute(&sdk::place_order(&addresses, &wallet, &packet, Receipt::Off))
        }
        Op::PostOnly { trader, side, price, size, slide } => {
            let (wallet, _, _) = keys(world, *trader);
            let rejection = match slide {
                true => PostOnlyRejection::Slide,
                false => PostOnlyRejection::Reject,
            };
            let packet = sdk::post_only(*side, Ticks(*price), BaseLots(*size), rejection);
            world.execute(&sdk::place_order(&addresses, &wallet, &packet, Receipt::Off))
        }
        Op::Ioc { trader, side, size } => {
            let (wallet, _, _) = keys(world, *trader);
            let packet = sdk::market_order(*side, BaseLots(*size), 16);
            world.execute(&sdk::place_order(&addresses, &wallet, &packet, Receipt::Off))
        }
        Op::Cancel { trader, pick } => {
            let (wallet, _, _) = keys(world, *trader);
            let id = pick_order(world, *trader, *pick);
            world.execute(&sdk::cancel_order(&addresses, &wallet, &id))
        }
        Op::CancelAll { trader, side, limit } => {
            let (wallet, _, _) = keys(world, *trader);
            world.execute(&sdk::cancel_all_orders(&addresses, &wallet, *side, *limit))
        }
        Op::Reduce { trader, pick, size } => {
            let (wallet, _, _) = keys(world, *trader);
            let id = pick_order(world, *trader, *pick);
            world.execute(&sdk::reduce_order(&addresses, &wallet, &id, BaseLots(*size)))
        }
        Op::Batch { trader, cancels, orders } => {
            let (wallet, _, _) = keys(world, *trader);
            let resting = world.all_resting(*trader);
            let cancels: Vec<FIFOOrderId> =
                resting.into_iter().take(*cancels).collect();
            let packets: Vec<OrderPacket> = orders
                .iter()
                .map(|(side, price, size)| {
                    sdk::post_only(
                        *side,
                        Ticks(*price),
                        BaseLots(*size),
                        PostOnlyRejection::Reject,
                    )
                })
                .collect();
            world.execute(&sdk::batch_update(&addresses, &wallet, &cancels, &packets))
        }
        Op::Swap { trader, side, price, size } => {
            let (wallet, token_base, token_quote) = keys(world, *trader);
            world.execute(&sdk::swap(
                &addresses,
                &wallet,
                &token_base,
                &token_quote,
                *side,
                Ticks(*price),
                BaseLots(*size),
                BaseLots::ZERO,
                16,
                Receipt::Off,
            ))
        }
        Op::CollectFees => {
            let recipient = world.fee_recipient();
            world.execute(&sdk::collect_fees(&addresses, &recipient))
        }
    }
}

/// Claims and funds a seat for every trader, through the same instructions the campaign
/// generates.
///
/// Not left to chance. Almost every instruction needs a seat with something in it, so a
/// sequence that has not happened to claim one yet is a sequence spent being rejected —
/// measured on an early version of this campaign, half the sequences never produced a
/// single fill, which means half of them never reached matching, fees or settlement at
/// all.
///
/// Seat churn is still generated: `ClaimSeat` and `EvictSeat` stay in the mix, so the
/// campaign still reaches markets where a seat has been taken away. It just no longer
/// spends most of its length getting to the starting line.
fn bootstrap(world: &mut World) {
    for trader in 0..TRADERS {
        assert!(apply(world, &Op::ClaimSeat { trader }), "seat {trader}");
        assert!(
            apply(
                world,
                &Op::Deposit {
                    trader,
                    base: 2_000,
                    quote: 2_000_000,
                }
            ),
            "funding {trader}"
        );
    }
}

/// One of a trader's resting orders, or an ID for one that never existed.
///
/// The second case is deliberate: cancelling an order that is not there is the most
/// likely thing a confused client does, and it has to be refused rather than accepted
/// against somebody else's order.
///
/// Built with `from_encoded` rather than `new`, so the sequence number is whatever the
/// generator produced — including values whose high bit sets the side tag. `new` would
/// assert those away, and an ID arriving over the wire has had no such courtesy applied
/// to it.
fn pick_order(world: &World, trader: usize, pick: usize) -> FIFOOrderId {
    let resting = world.all_resting(trader);
    match resting.is_empty() {
        true => FIFOOrderId::from_encoded(Ticks(100), pick as u64),
        false => resting[pick % resting.len()],
    }
}

/// Proptest's own `PROPTEST_CASES` decides the length of a campaign; this only supplies a
/// default small enough to belong in the ordinary test run.
fn campaign() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var("PROPTEST_CASES").is_err() {
        config.cases = DEFAULT_CASES;
    }
    config
}

proptest! {
    #![proptest_config(campaign())]

    /// Nothing any sequence of instructions can do makes the market lie about what it
    /// holds, or leaves it holding less than it owes.
    #[test]
    fn value_survives_arbitrary_instruction_sequences(
        ops in prop::collection::vec(op(), 1..OPS_PER_CASE)
    ) {
        let mut world = World::new(TRADERS, TAKER_FEE_BPS);
        prop_assert_eq!(world.check(), Ok(()), "a fresh market is already broken");
        bootstrap(&mut world);

        for (step, op) in ops.iter().enumerate() {
            apply(&mut world, op);
            // After every instruction, not just the ones that landed. A rejection that
            // left something behind is the worst kind of bug: the caller is told nothing
            // happened.
            if let Err(broken) = world.check() {
                prop_assert!(false, "step {step} ({op:?}) broke an invariant: {broken}");
            }
        }
    }

    /// Cancel everything, withdraw everything, sweep the fees — and every atom that went
    /// in has come back out.
    ///
    /// The invariant the roadmap names, and the one a venue is actually judged on. A
    /// market that conserves value while it runs but cannot be emptied has stranded
    /// somebody's money, which from the outside is indistinguishable from losing it.
    #[test]
    fn the_market_can_always_be_wound_down(
        ops in prop::collection::vec(op(), 1..OPS_PER_CASE)
    ) {
        let mut world = World::new(TRADERS, TAKER_FEE_BPS);
        bootstrap(&mut world);
        for op in &ops {
            apply(&mut world, op);
        }

        world.drain();

        // Nothing resting, nothing owed, nothing left in the vaults.
        prop_assert_eq!(world.book_len(), 0, "orders survived the wind-down");
        let header = *world.market().header();
        prop_assert_eq!(
            header.base_lots_deposited.as_u64(),
            0,
            "the market still owes base"
        );
        prop_assert_eq!(
            header.quote_lots_deposited.as_u64(),
            0,
            "the market still owes quote"
        );
        prop_assert_eq!(world.vault_atoms(), (0, 0), "the vaults are not empty");

        // And the money is back where it came from: the traders' wallets, plus whatever
        // the venue earned.
        let (base_start, quote_start) = world.starting_totals();
        let mut base_returned = 0u64;
        let mut quote_returned = 0u64;
        for index in 0..TRADERS {
            let (base, quote) = world.wallet_atoms(index);
            base_returned += base;
            quote_returned += quote;
        }
        prop_assert_eq!(base_returned, base_start, "base atoms went missing");
        prop_assert_eq!(
            quote_returned + world.collected_fee_atoms(),
            quote_start,
            "quote atoms went missing"
        );
    }
}

/// A fuzzer where nothing lands proves nothing, and looks exactly like one that works.
///
/// So this drives the same instruction builders through a sequence chosen to succeed,
/// and insists that each one does. If an account list is wrong or a discriminant moves,
/// the campaign above would quietly reject everything and still pass; this fails.
#[test]
fn every_instruction_the_campaign_builds_can_land() {
    let mut world = World::new(TRADERS, TAKER_FEE_BPS);
    let landed = |world: &mut World, op: Op| {
        assert!(apply(world, &op), "{op:?} should have landed");
        assert_eq!(world.check(), Ok(()), "after {op:?}");
    };

    landed(&mut world, Op::ClaimSeat { trader: 0 });
    landed(&mut world, Op::ClaimSeat { trader: 1 });
    landed(&mut world, Op::Deposit { trader: 0, base: 500, quote: 500_000 });
    landed(&mut world, Op::Deposit { trader: 1, base: 500, quote: 500_000 });

    // A maker rests, a taker crosses it. This is the only way the fee counter moves, so
    // it is the step that proves the campaign can reach settlement at all.
    landed(&mut world, Op::PostOnly { trader: 0, side: Side::Ask, price: 101, size: 10, slide: false });
    landed(&mut world, Op::Limit { trader: 1, side: Side::Bid, price: 101, size: 10 });
    assert!(
        world.market().header().collected_quote_lot_fees.as_u64() > 0,
        "a crossing order charged no fee, so nothing actually traded"
    );

    landed(&mut world, Op::PostOnly { trader: 0, side: Side::Ask, price: 102, size: 20, slide: false });
    landed(&mut world, Op::Reduce { trader: 0, pick: 0, size: 5 });
    landed(&mut world, Op::Cancel { trader: 0, pick: 0 });
    landed(&mut world, Op::PostOnly { trader: 0, side: Side::Bid, price: 98, size: 10, slide: true });
    landed(&mut world, Op::CancelAll { trader: 0, side: Side::Bid, limit: 16 });

    // The market-maker cycle, and the newest instruction in the program.
    landed(
        &mut world,
        Op::Batch {
            trader: 0,
            cancels: 0,
            orders: vec![(Side::Ask, 103, 5), (Side::Bid, 97, 5)],
        },
    );
    landed(
        &mut world,
        Op::Batch {
            trader: 0,
            cancels: 2,
            orders: vec![(Side::Ask, 104, 5), (Side::Bid, 96, 5)],
        },
    );

    // A wallet with no seat, trading in one instruction.
    landed(&mut world, Op::Swap { trader: 2, side: Side::Bid, price: 105, size: 3 });

    landed(&mut world, Op::CollectFees);
    assert!(world.collected_fee_atoms() > 0, "fees were not swept anywhere");

    landed(&mut world, Op::Ioc { trader: 1, side: Side::Ask, size: 2 });
    landed(&mut world, Op::Withdraw { trader: 1, base: 1, quote: 1 });

    // An empty seat is anybody's to reclaim, which is the last instruction left.
    landed(&mut world, Op::ClaimSeat { trader: 2 });
    landed(&mut world, Op::EvictSeat { trader: 2 });

    // And the whole thing still winds down.
    world.drain();
    assert_eq!(world.book_len(), 0);
    assert_eq!(world.vault_atoms(), (0, 0));
}

/// A campaign that reaches nothing tests nothing, and looks exactly like one that works.
///
/// Every op can be rejected for a perfectly good reason — no seat, no funds, nothing
/// resting to cancel — and a mix that is rejected most of the time still satisfies every
/// invariant above, because an instruction that does nothing breaks nothing. The
/// campaign's productivity has to be asserted rather than assumed.
///
/// It was assumed once. An earlier version of this file left seat claiming to the
/// generator, and measured: barely half the instructions landed, the book never got past
/// ten orders, and two sequences in five never produced a single fill. It passed.
///
/// The floors below sit well under what is measured, so ordinary drift in the strategy
/// does not fail the build — but a change that guts the campaign does.
#[test]
fn the_campaign_reaches_states_worth_testing() {
    /// Sequences to sample. Fewer than the campaign runs, because this measures the
    /// generator rather than the program.
    const SEQUENCES: usize = 8;
    /// Instructions per sequence. Depth is bounded by this, not by the number of
    /// sequences: a book can only get as deep as one run has time to build it.
    const OPS: usize = 150;

    let mut runner = TestRunner::deterministic();
    let strategy = op();

    let (mut attempted, mut landed, mut traded, mut deepest) = (0usize, 0usize, 0usize, 0usize);
    for _ in 0..SEQUENCES {
        let mut world = World::new(TRADERS, TAKER_FEE_BPS);
        bootstrap(&mut world);
        for _ in 0..OPS {
            let op = strategy
                .new_tree(&mut runner)
                .expect("the strategy should always produce an op")
                .current();
            attempted += 1;
            if apply(&mut world, &op) {
                landed += 1;
            }
            deepest = deepest.max(world.book_len());
        }
        if world.market().header().collected_quote_lot_fees.as_u64() > 0 {
            traded += 1;
        }
    }

    assert!(
        landed * 100 / attempted >= 60,
        "only {landed} of {attempted} instructions landed — the campaign is mostly testing rejection"
    );
    assert!(
        traded * 4 >= SEQUENCES * 3,
        "only {traded} of {SEQUENCES} sequences produced a fill — most never reached settlement"
    );
    assert!(
        deepest >= 8,
        "the book never got deeper than {deepest} orders — nothing exercised a wind-down at depth"
    );
}
