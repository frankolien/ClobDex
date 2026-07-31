//! Atomic swap, against the real SPL Token program.
//!
//! These are the only tests that move actual tokens: everything else writes balances
//! into the market by hand. That matters, because the swap's correctness is not just
//! book-keeping — it has to move the right number of atoms in and out of vaults it does
//! not own outright.

mod common;

use clob_book::Side;
use mollusk_svm::result::Check;
use solana_account::Account;
use solana_pubkey::Pubkey;

use common::*;

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}

/// Everything a swap transaction touches.
struct Scene {
    fixture: Fixture,
    taker: Pubkey,
    taker_base: Pubkey,
    taker_quote: Pubkey,
    accounts: Vec<(Pubkey, Account)>,
}

/// A market with `depth` resting asks from a maker, vaults funded to match, and a taker
/// holding tokens in its own wallet but no seat on the market.
fn scene(depth: u64, size: u64, taker_base_tokens: u64, taker_quote_tokens: u64) -> Scene {
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);

    // The maker's inventory is recorded in the market, so the vault must actually hold
    // the matching atoms or a withdrawal would fail against a real token program.
    let maker = trader(1);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);
    if depth > 0 {
        seed_depth(&mut market, maker, Side::Ask, 100, depth, size);
    }

    let taker = trader(2);
    let taker_base = Pubkey::new_from_array([60u8; 32]);
    let taker_quote = Pubkey::new_from_array([61u8; 32]);

    let config = lot_config();
    let vault_base_atoms = 1_000_000 * config.base_atoms_per_base_lot;
    let vault_quote_atoms = 1_000_000_000 * config.quote_atoms_per_quote_lot;

    let accounts = vec![
        (fixture.market, market),
        (taker, wallet()),
        (
            taker_base,
            token_account(fixture.base_mint, taker, taker_base_tokens),
        ),
        (
            taker_quote,
            token_account(fixture.quote_mint, taker, taker_quote_tokens),
        ),
        (
            fixture.base_vault,
            token_account(fixture.base_mint, fixture.vault_signer, vault_base_atoms),
        ),
        (
            fixture.quote_vault,
            token_account(fixture.quote_mint, fixture.vault_signer, vault_quote_atoms),
        ),
        (fixture.vault_signer, wallet()),
        mollusk_svm_programs_token::token::keyed_account(),
    ];

    Scene {
        fixture,
        taker,
        taker_base,
        taker_quote,
        accounts,
    }
}

fn account_of(accounts: &[(Pubkey, Account)], key: Pubkey) -> &Account {
    &accounts.iter().find(|(k, _)| *k == key).unwrap().1
}

#[test]
fn a_taker_with_no_seat_can_buy_and_be_paid_out_in_one_instruction() {
    // The aggregator case: a wallet that has never touched this market, routed through
    // in a single transaction.
    let scene = scene(4, 10, 0, 10_000_000);
    let config = lot_config();

    let result = mollusk_with_token().process_and_validate_instruction(
        &swap_ix(
            &scene.fixture,
            scene.taker,
            scene.taker_base,
            scene.taker_quote,
            Side::Bid,
            105,
            25,
            0,
            16,
        ),
        &scene.accounts,
        &[Check::success()],
    );

    // 25 base lots arrived in the taker's wallet.
    let base_after = token_balance(account_of(&result.resulting_accounts, scene.taker_base));
    assert_eq!(base_after, 25 * config.base_atoms_per_base_lot);

    // Paid the maker's prices, not the limit: 10@100 + 10@101 + 5@102.
    let spent = 10_000_000
        - token_balance(account_of(&result.resulting_accounts, scene.taker_quote));
    assert_eq!(spent, (1_000 + 1_010 + 510) * config.quote_atoms_per_quote_lot);

    let market = account_of(&result.resulting_accounts, scene.fixture.market);
    assert_eq!(market_of(market).check_conservation(), Ok(()));
}

#[test]
fn a_one_shot_swapper_leaves_no_seat_behind() {
    // Otherwise an aggregator routing strangers through would fill the trader table
    // with empty seats and eventually lock everyone out.
    let scene = scene(2, 10, 0, 10_000_000);
    let before = market_of(account_of(&scene.accounts, scene.fixture.market))
        .traders()
        .len();

    let result = mollusk_with_token().process_and_validate_instruction(
        &swap_ix(
            &scene.fixture,
            scene.taker,
            scene.taker_base,
            scene.taker_quote,
            Side::Bid,
            105,
            10,
            0,
            8,
        ),
        &scene.accounts,
        &[Check::success()],
    );

    let market = account_of(&result.resulting_accounts, scene.fixture.market);
    assert_eq!(market_of(market).traders().len(), before, "seat was not released");
}

#[test]
fn a_swap_returns_the_unspent_input() {
    // The program moves in the most the order could cost. Anything the fills did not
    // consume has to come back, or a taker quoting a wide limit would be silently
    // overcharged.
    let scene = scene(1, 10, 0, 10_000_000);
    let config = lot_config();

    let result = mollusk_with_token().process_and_validate_instruction(
        &swap_ix(
            &scene.fixture,
            scene.taker,
            scene.taker_base,
            scene.taker_quote,
            // Limit far above the book: the input moved in is sized at 200, the fill
            // happens at 100.
            Side::Bid,
            200,
            10,
            0,
            8,
        ),
        &scene.accounts,
        &[Check::success()],
    );

    let spent = 10_000_000
        - token_balance(account_of(&result.resulting_accounts, scene.taker_quote));
    assert_eq!(spent, 1_000 * config.quote_atoms_per_quote_lot);
}

#[test]
fn a_taker_can_sell_into_the_book() {
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market, maker, 1_000_000, 1_000_000_000);
    seed_depth(&mut market, maker, Side::Bid, 100, 2, 10);

    let taker = trader(2);
    let taker_base = Pubkey::new_from_array([60u8; 32]);
    let taker_quote = Pubkey::new_from_array([61u8; 32]);
    let config = lot_config();

    let accounts = vec![
        (fixture.market, market),
        (taker, wallet()),
        (
            taker_base,
            token_account(fixture.base_mint, taker, 20 * config.base_atoms_per_base_lot),
        ),
        (taker_quote, token_account(fixture.quote_mint, taker, 0)),
        (
            fixture.base_vault,
            token_account(fixture.base_mint, fixture.vault_signer, 1_000_000 * config.base_atoms_per_base_lot),
        ),
        (
            fixture.quote_vault,
            token_account(fixture.quote_mint, fixture.vault_signer, 1_000_000_000),
        ),
        (fixture.vault_signer, wallet()),
        mollusk_svm_programs_token::token::keyed_account(),
    ];

    let result = mollusk_with_token().process_and_validate_instruction(
        &swap_ix(&fixture, taker, taker_base, taker_quote, Side::Ask, 99, 20, 0, 8),
        &accounts,
        &[Check::success()],
    );

    // Sold 20 at 100 and 99.
    assert_eq!(token_balance(account_of(&result.resulting_accounts, taker_base)), 0);
    assert_eq!(
        token_balance(account_of(&result.resulting_accounts, taker_quote)),
        (1_000 + 990) * config.quote_atoms_per_quote_lot
    );
}

#[test]
fn a_swap_that_misses_its_minimum_moves_nothing() {
    // The taker's tokens must not be stranded in the vault when the order is rejected.
    let scene = scene(1, 10, 0, 10_000_000);

    let result = mollusk_with_token().process_instruction(
        &swap_ix(
            &scene.fixture,
            scene.taker,
            scene.taker_base,
            scene.taker_quote,
            Side::Bid,
            105,
            50,
            50, // more than the book holds
            8,
        ),
        &scene.accounts,
    );

    assert!(result.program_result.is_err(), "unmet minimum must revert");
}

#[test]
fn a_swap_by_an_existing_maker_returns_only_the_trade() {
    // The naive implementation drains the caller's standing balance. Here the maker
    // holds inventory *and* swaps; only the swap's own proceeds may leave.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let other = trader(1);
    let both = trader(3);
    fixture.seat(&mut market, other, 1_000_000, 1_000_000_000);
    seed_depth(&mut market, other, Side::Ask, 100, 1, 10);
    // `both` keeps a balance on the market that the swap must not touch.
    fixture.seat(&mut market, both, 500, 500_000);

    let both_base = Pubkey::new_from_array([62u8; 32]);
    let both_quote = Pubkey::new_from_array([63u8; 32]);
    let config = lot_config();

    let accounts = vec![
        (fixture.market, market),
        (both, wallet()),
        (both_base, token_account(fixture.base_mint, both, 0)),
        (
            both_quote,
            token_account(fixture.quote_mint, both, 10_000_000),
        ),
        (
            fixture.base_vault,
            token_account(fixture.base_mint, fixture.vault_signer, 1_000_000 * config.base_atoms_per_base_lot),
        ),
        (
            fixture.quote_vault,
            token_account(fixture.quote_mint, fixture.vault_signer, 1_000_000_000),
        ),
        (fixture.vault_signer, wallet()),
        mollusk_svm_programs_token::token::keyed_account(),
    ];

    let result = mollusk_with_token().process_and_validate_instruction(
        &swap_ix(&fixture, both, both_base, both_quote, Side::Bid, 105, 10, 0, 8),
        &accounts,
        &[Check::success()],
    );

    let market_after = account_of(&result.resulting_accounts, fixture.market);
    // The standing balance is untouched: still 500 base and 500_000 quote on the seat.
    assert_eq!(free_balances(market_after, both), (500, 500_000));
    // And the swap's 10 base lots went to the wallet, not to the seat.
    assert_eq!(
        token_balance(account_of(&result.resulting_accounts, both_base)),
        10 * config.base_atoms_per_base_lot
    );
    assert_eq!(market_of(market_after).check_conservation(), Ok(()));
}

#[test]
fn the_swap_costs_what_it_costs() {
    let scene = scene(4, 10, 0, 10_000_000);
    let result = mollusk_with_token().process_instruction(
        &swap_ix(
            &scene.fixture,
            scene.taker,
            scene.taker_base,
            scene.taker_quote,
            Side::Bid,
            105,
            25,
            0,
            16,
        ),
        &scene.accounts,
    );

    assert!(!result.program_result.is_err());
    println!(
        "swap, 3 levels, 8 accounts: {} CU",
        result.compute_units_consumed
    );
    assert!(result.compute_units_consumed < 100_000);
}

// ---------------------------------------------------------------------------------
// Deposit and withdraw, also against the real token program
// ---------------------------------------------------------------------------------

#[test]
fn depositing_and_withdrawing_moves_the_exact_atoms() {
    let fixture = Fixture::new();
    let market = fixture.market_account(0);
    let trader = trader(4);
    let trader_base = Pubkey::new_from_array([70u8; 32]);
    let trader_quote = Pubkey::new_from_array([71u8; 32]);
    let config = lot_config();

    let accounts = vec![
        (fixture.market, market),
        (trader, wallet()),
        (
            trader_base,
            token_account(fixture.base_mint, trader, 50 * config.base_atoms_per_base_lot),
        ),
        (trader_quote, token_account(fixture.quote_mint, trader, 9_000)),
        (
            fixture.base_vault,
            token_account(fixture.base_mint, fixture.vault_signer, 0),
        ),
        (
            fixture.quote_vault,
            token_account(fixture.quote_mint, fixture.vault_signer, 0),
        ),
        (fixture.vault_signer, wallet()),
        mollusk_svm_programs_token::token::keyed_account(),
    ];

    let mollusk = mollusk_with_token();
    let deposited = mollusk.process_and_validate_instruction(
        &funds_ix(&fixture, trader, trader_base, trader_quote, false, 30, 5_000),
        &accounts,
        &[Check::success()],
    );

    // Lots convert to atoms exactly, so the vault holds precisely what was credited.
    assert_eq!(
        token_balance(account_of(&deposited.resulting_accounts, fixture.base_vault)),
        30 * config.base_atoms_per_base_lot
    );
    assert_eq!(
        token_balance(account_of(&deposited.resulting_accounts, trader_quote)),
        4_000
    );
    let market_after = account_of(&deposited.resulting_accounts, fixture.market);
    assert_eq!(free_balances(market_after, trader), (30, 5_000));
    assert_eq!(market_of(market_after).check_conservation(), Ok(()));

    let withdrawn = mollusk.process_and_validate_instruction(
        &funds_ix(&fixture, trader, trader_base, trader_quote, true, 30, 5_000),
        &deposited.resulting_accounts,
        &[Check::success()],
    );

    // Everything came back, and the vaults are empty again.
    assert_eq!(
        token_balance(account_of(&withdrawn.resulting_accounts, trader_base)),
        50 * config.base_atoms_per_base_lot
    );
    assert_eq!(
        token_balance(account_of(&withdrawn.resulting_accounts, trader_quote)),
        9_000
    );
    assert_eq!(
        token_balance(account_of(&withdrawn.resulting_accounts, fixture.base_vault)),
        0
    );
}

#[test]
fn a_vault_that_is_not_the_market_s_own_is_rejected() {
    // Without this check a caller could name a token account it controls as the vault
    // and have the program credit a deposit that never arrived.
    let fixture = Fixture::new();
    let trader = trader(4);
    let trader_base = Pubkey::new_from_array([70u8; 32]);
    let trader_quote = Pubkey::new_from_array([71u8; 32]);
    let impostor = Pubkey::new_from_array([99u8; 32]);
    let config = lot_config();

    let mut instruction = funds_ix(&fixture, trader, trader_base, trader_quote, false, 1, 0);
    instruction.accounts[4].pubkey = impostor;

    let result = mollusk_with_token().process_instruction(
        &instruction,
        &[
            (fixture.market, fixture.market_account(0)),
            (trader, wallet()),
            (
                trader_base,
                token_account(fixture.base_mint, trader, config.base_atoms_per_base_lot),
            ),
            (trader_quote, token_account(fixture.quote_mint, trader, 0)),
            (impostor, token_account(fixture.base_mint, trader, 0)),
            (
                fixture.quote_vault,
                token_account(fixture.quote_mint, fixture.vault_signer, 0),
            ),
            mollusk_svm_programs_token::token::keyed_account(),
        ],
    );

    assert!(result.program_result.is_err(), "a foreign vault must be rejected");
}
