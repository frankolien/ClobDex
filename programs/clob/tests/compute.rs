//! Compute-unit measurements against the compiled SBF binary.
//!
//! Compute is the binding constraint on-chain, and it is the axis this venue has to
//! compete on. These numbers come from Mollusk executing the real binary, so they are
//! what a validator would charge — not an estimate.
//!
//! The tests assert *ceilings*, not exact values, so a refactor that makes something
//! cheaper does not fail the suite while a regression does. Run with
//! `cargo test -p clob-program --test compute -- --nocapture` to print the table.

mod common;

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_client::instruction::{self as sdk, Receipt};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use common::world::World;
use common::*;

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}

/// Runs one instruction and returns the compute units consumed.
fn measure(
    market_account: solana_account::Account,
    signer: Pubkey,
    instruction: &Instruction,
    market: Pubkey,
) -> u64 {
    let result = mollusk().process_instruction(
        instruction,
        &[(market, market_account), (signer, wallet())],
    );
    assert!(
        !result.program_result.is_err(),
        "benchmark instruction failed: {:?}",
        result.program_result
    );
    result.compute_units_consumed
}

/// A market with `depth` resting asks owned by the maker, from tick 100 upward.
fn book_with_depth(depth: u64) -> (Fixture, solana_account::Account) {
    let fixture = Fixture::new();
    let mut account = fixture.market_account(2);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut account, maker, 1_000_000, 1_000_000_000);
    fixture.seat(&mut account, taker, 0, 1_000_000_000);
    if depth > 0 {
        seed_depth(&mut account, maker, Side::Ask, 100, depth, 10);
    }
    (fixture, account)
}

#[test]
fn report() {
    println!("\n  instruction                              accounts       CU");
    println!("  ----------------------------------------------------------");

    // Posting into an empty book: the floor for a maker action.
    let (fixture, account) = book_with_depth(0);
    let post_empty = measure(
        account,
        trader(1),
        &post_only_ix(fixture.market, trader(1), Side::Ask, 100, 10),
        fixture.market,
    );
    println!("  post-only, empty book                           2   {post_empty:>6}");

    // Posting into a book that already has depth: the realistic maker case, where the
    // red-black insert actually has to descend.
    for depth in [16u64, 64] {
        let (fixture, account) = book_with_depth(depth);
        let cu = measure(
            account,
            trader(1),
            &post_only_ix(fixture.market, trader(1), Side::Bid, 50, 10),
            fixture.market,
        );
        println!("  post-only, {depth:>3} resting orders                  2   {cu:>6}");
    }

    // Taking: one price level, then a sweep. The slope between these is the marginal
    // cost of a fill, which is the number that decides how deep a taker can sweep
    // inside one transaction.
    for levels in [1u64, 4, 16] {
        let (fixture, account) = book_with_depth(levels);
        let cu = measure(
            account,
            trader(2),
            &market_order_ix(fixture.market, trader(2), Side::Bid, levels * 10, 64),
            fixture.market,
        );
        println!("  market order, sweeping {levels:>2} level(s)             2   {cu:>6}");
    }

    // Cancelling, single and batched.
    let (fixture, mut account) = book_with_depth(0);
    seed_depth(&mut account, trader(1), Side::Bid, 90, 8, 10);
    let id = newest_order(&account, Side::Bid);
    let cancel_one = measure(
        account.clone(),
        trader(1),
        &cancel_ix(fixture.market, trader(1), id),
        fixture.market,
    );
    println!("  cancel one order                                2   {cancel_one:>6}");

    let cancel_eight = measure(
        account,
        trader(1),
        &cancel_all_ix(fixture.market, trader(1), Side::Bid, 8),
        fixture.market,
    );
    println!("  cancel 8 orders                                 2   {cancel_eight:>6}");

    settlement();
    println!();
}

/// The half of the table the fixture above cannot reach.
///
/// Everything so far writes balances straight into the market, which is right for
/// measuring order entry and wrong for measuring anything that moves money: a deposit is
/// a CPI to the SPL Token program, and that CPI is most of what it costs. These run in
/// the same world the fuzzing campaign uses, with real vaults and real token accounts, so
/// the numbers include the settlement they would include on a validator.
fn settlement() {
    let mut world = World::new(3, 2);
    let addresses = world.addresses();
    let (maker, maker_base, maker_quote) = trader_keys(&world, 0);
    let (taker, taker_base, taker_quote) = trader_keys(&world, 1);
    let stranger = world.traders()[2].wallet;

    let claim = world.measure(&sdk::claim_seat(&addresses, &maker));
    println!("  claim a seat                                    2   {claim:>6}");
    world.execute(&sdk::claim_seat(&addresses, &taker));

    let deposit = world.measure(&sdk::deposit(
        &addresses,
        &maker,
        &maker_base,
        &maker_quote,
        BaseLots(5_000),
        QuoteLots(5_000_000),
    ));
    println!("  deposit                                         7   {deposit:>6}");
    world.execute(&sdk::deposit(
        &addresses,
        &taker,
        &taker_base,
        &taker_quote,
        BaseLots(5_000),
        QuoteLots(5_000_000),
    ));

    let withdraw = world.measure(&sdk::withdraw(
        &addresses,
        &maker,
        &maker_base,
        &maker_quote,
        BaseLots(1),
        QuoteLots(1),
    ));
    println!("  withdraw                                        8   {withdraw:>6}");

    // A maker ladder for the taking instructions to cross.
    let ladder: Vec<_> = (0..4)
        .map(|level| post_only_packet(Side::Ask, 100 + level, 10))
        .collect();
    let batch_places = world.measure(&sdk::batch_update(&addresses, &maker, &[], &ladder));
    println!("  batch: 0 cancels, 4 places                      2   {batch_places:>6}");

    let resting: Vec<_> = world.all_resting(0);
    let batch_both =
        world.measure(&sdk::batch_update(&addresses, &maker, &resting[..4], &ladder));
    println!("  batch: 4 cancels, 4 places                      2   {batch_both:>6}");

    // Crossing with a seat, and crossing without one. The difference is the settlement:
    // a seat already has funds in the market, so a swap pays for two token transfers a
    // resting trader does not.
    let crossing = sdk::limit(Side::Bid, Ticks(103), BaseLots(20));
    let taken = world.measure(&sdk::place_order(&addresses, &taker, &crossing, Receipt::Off));
    println!("  limit, crossing 2 levels (seated)               2   {taken:>6}");

    let swap = world.measure(&sdk::swap(
        &addresses,
        &stranger,
        &world.traders()[2].base,
        &world.traders()[2].quote,
        Side::Bid,
        Ticks(103),
        BaseLots(10),
        BaseLots::ZERO,
        64,
        Receipt::Off,
    ));
    println!("  swap, crossing 1 level (no seat)                8   {swap:>6}");

    let collect = world.measure(&sdk::collect_fees(&addresses, &world.fee_recipient()));
    println!("  collect fees                                    5   {collect:>6}");
}

/// A trader's wallet and its two token accounts.
fn trader_keys(world: &World, index: usize) -> (Pubkey, Pubkey, Pubkey) {
    let trader = &world.traders()[index];
    (trader.wallet, trader.base, trader.quote)
}

#[test]
fn batching_a_refresh_costs_less_than_the_instructions_it_replaces() {
    // The reason batch exists. A maker replacing a two-sided ladder pays for one
    // instruction instead of one per quote, and the per-instruction overhead — loading
    // accounts, re-deriving the seat, re-borrowing the market — is paid once.
    let (fixture, market) = book_with_depth(16);
    let maker = trader(1);

    let resting: Vec<_> = market_of(&market)
        .book()
        .iter_side(Side::Ask)
        .map(|entry| entry.key)
        .take(4)
        .collect();

    let batched = measure(
        market.clone(),
        maker,
        &batch_ix(
            fixture.market,
            maker,
            &resting,
            &[
                post_only_packet(Side::Ask, 200, 1),
                post_only_packet(Side::Ask, 201, 1),
                post_only_packet(Side::Ask, 202, 1),
                post_only_packet(Side::Ask, 203, 1),
            ],
        ),
        fixture.market,
    );

    // What the same work costs as separate instructions: one cancel plus one place,
    // times four.
    let single_cancel = measure(
        market.clone(),
        maker,
        &cancel_ix(fixture.market, maker, resting[0]),
        fixture.market,
    );
    let single_place = measure(
        market.clone(),
        maker,
        &post_only_ix(fixture.market, maker, Side::Ask, 200, 1),
        fixture.market,
    );
    let separately = 4 * (single_cancel + single_place);

    println!("  4 cancels + 4 places, batched      {batched:>6} CU");
    println!("  the same, as 8 instructions        {separately:>6} CU");
    assert!(
        batched < separately,
        "batching cost {batched} CU against {separately} separately — it must be cheaper \
         than the instructions it replaces, or there is no reason for it to exist"
    );
}

#[test]
fn a_maker_action_leaves_room_to_batch() {
    // A market maker cancel-replaces continuously, so a single quote update has to be
    // cheap enough that several fit in one 1.4M CU transaction alongside the signature
    // and account-loading overhead. 50k gives room for well over a dozen.
    let (fixture, account) = book_with_depth(64);
    let cu = measure(
        account,
        trader(1),
        &post_only_ix(fixture.market, trader(1), Side::Bid, 50, 10),
        fixture.market,
    );
    assert!(cu < 50_000, "post-only cost {cu} CU into a 64-order book");
}

#[test]
fn a_single_fill_leaves_room_to_sweep() {
    let (fixture, account) = book_with_depth(1);
    let cu = measure(
        account,
        trader(2),
        &market_order_ix(fixture.market, trader(2), Side::Bid, 10, 8),
        fixture.market,
    );
    assert!(cu < 50_000, "single fill cost {cu} CU");
}

#[test]
fn sweeping_scales_linearly_rather_than_quadratically() {
    // The property that matters for deep sweeps: cost per level must stay flat. A
    // quadratic term would cap sweep depth far below the match_limit a client sets.
    let (fixture, account) = book_with_depth(4);
    let four = measure(
        account,
        trader(2),
        &market_order_ix(fixture.market, trader(2), Side::Bid, 40, 64),
        fixture.market,
    );

    let (fixture, account) = book_with_depth(16);
    let sixteen = measure(
        account,
        trader(2),
        &market_order_ix(fixture.market, trader(2), Side::Bid, 160, 64),
        fixture.market,
    );

    // Four times the levels must cost well under four times the *marginal* budget plus
    // a fixed overhead. Stated as a generous bound so it fails on a real regression
    // rather than on noise.
    let per_level_at_4 = four / 4;
    let per_level_at_16 = sixteen / 16;
    assert!(
        per_level_at_16 < per_level_at_4 * 2,
        "per-level cost grew from {per_level_at_4} to {per_level_at_16} CU"
    );
}

#[test]
fn a_full_sweep_fits_in_one_transaction() {
    // 1.4M CU is the per-transaction ceiling. A taker must be able to clear meaningful
    // depth without splitting across transactions.
    let (fixture, account) = book_with_depth(64);
    let cu = measure(
        account,
        trader(2),
        &market_order_ix(fixture.market, trader(2), Side::Bid, 640, 128),
        fixture.market,
    );
    assert!(cu < 1_400_000, "64-level sweep cost {cu} CU");
    println!("64-level sweep: {cu} CU");
}

/// Finding a trader's seat must not get linearly more expensive as the market fills.
///
/// Every order-entry instruction begins by resolving a wallet to a seat, so if that
/// lookup were a scan, the cost of trading would rise with the number of people who have
/// ever traded — and a market would get more expensive precisely as it succeeded. It
/// would also be a griefing vector: claiming seats is cheap and permissionless.
///
/// Measured across a fifteen-fold increase in occupancy, the ceiling here is a factor of
/// two. A scan would blow through it long before the table was full.
#[test]
fn seat_lookup_does_not_scale_with_the_number_of_seats() {
    let cost = |seats: u8| {
        let fixture = Fixture::new();
        let mut account = fixture.market_account(2);
        for id in 0..seats {
            fixture.seat(&mut account, trader(id), 1_000_000, 1_000_000_000);
        }
        seed_depth(&mut account, trader(0), Side::Ask, 100, 8, 10);
        measure(
            account,
            trader(0),
            &post_only_ix(fixture.market, trader(0), Side::Bid, 50, 10),
            fixture.market,
        )
    };

    let (few, many) = (cost(2), cost(30));
    assert!(
        many < few * 2,
        "posting cost {few} CU with 2 seats and {many} with 30, which is not a lookup that scales"
    );
}
