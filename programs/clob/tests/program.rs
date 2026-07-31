//! End-to-end tests against the compiled SBF binary.

mod common;

use clob_book::{BaseLots, Side};
use mollusk_svm::result::Check;
use solana_pubkey::Pubkey;

use common::*;

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}

#[test]
fn a_post_only_order_rests_and_locks_the_backing_funds() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market_account, maker, 1_000, 1_000_000);

    let result = mollusk().process_and_validate_instruction(
        &post_only_ix(fixture.market, maker, Side::Ask, 100, 10),
        &[
            (fixture.market, market_account),
            (maker, wallet()),
        ],
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    let book = market_of(after).book();
    assert_eq!(book.len(Side::Ask), 1);
    assert_eq!(book.best_ask().unwrap().value.num_base_lots, BaseLots(10));
    // The base behind the quote moved from free to locked.
    assert_eq!(free_balances(after, maker).0, 990);
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}

#[test]
fn a_taker_lifting_an_offer_settles_inside_its_own_transaction() {
    // The crankless claim, tested end to end: after one transaction the maker's
    // proceeds are already free, with no second step and nobody running a crank.
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut market_account, maker, 1_000, 0);
    fixture.seat(&mut market_account, taker, 0, 1_000_000);
    seed_depth(&mut market_account, maker, Side::Ask, 100, 1, 10);

    let result = mollusk().process_and_validate_instruction(
        &market_order_ix(fixture.market, taker, Side::Bid, 4, 8),
        &[
            (fixture.market, market_account),
            (taker, wallet()),
        ],
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    assert_eq!(free_balances(after, taker).0, 4);
    assert_eq!(free_balances(after, maker).1, 400);
    assert_eq!(
        market_of(after).book().best_ask().unwrap().value.num_base_lots,
        BaseLots(6)
    );
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}

#[test]
fn a_taker_pays_the_fee_and_the_market_keeps_it() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(10);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut market_account, maker, 1_000, 0);
    fixture.seat(&mut market_account, taker, 0, 1_000_000);
    seed_depth(&mut market_account, maker, Side::Ask, 100, 1, 10);

    let result = mollusk().process_and_validate_instruction(
        &market_order_ix(fixture.market, taker, Side::Bid, 10, 8),
        &[(fixture.market, market_account), (taker, wallet())],
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    let header = market_of(after).header();
    // 1_000 quote lots at 10 bps.
    assert_eq!(header.unclaimed_quote_lot_fees.as_u64(), 1);
    assert_eq!(free_balances(after, taker).1, 1_000_000 - 1_001);
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}

#[test]
fn cancelling_returns_the_locked_funds() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market_account, maker, 0, 1_000_000);
    seed_depth(&mut market_account, maker, Side::Bid, 100, 1, 10);
    let id = newest_order(&market_account, Side::Bid);

    let result = mollusk().process_and_validate_instruction(
        &cancel_ix(fixture.market, maker, id),
        &[(fixture.market, market_account), (maker, wallet())],
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    assert!(after_is_empty(after, Side::Bid));
    assert_eq!(free_balances(after, maker).1, 1_000_000);
}

fn after_is_empty(account: &solana_account::Account, side: Side) -> bool {
    market_of(account).book().is_empty(side)
}

#[test]
fn a_stranger_cannot_cancel_someone_elses_order() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    let stranger = trader(2);
    fixture.seat(&mut market_account, maker, 0, 1_000_000);
    fixture.seat(&mut market_account, stranger, 0, 0);
    seed_depth(&mut market_account, maker, Side::Bid, 100, 1, 10);
    let id = newest_order(&market_account, Side::Bid);

    let result = mollusk().process_instruction(
        &cancel_ix(fixture.market, stranger, id),
        &[(fixture.market, market_account), (stranger, wallet())],
    );

    assert!(result.program_result.is_err(), "cancel by a non-owner must fail");
}

#[test]
fn an_unsigned_order_is_rejected() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market_account, maker, 1_000, 0);

    let mut instruction = post_only_ix(fixture.market, maker, Side::Ask, 100, 10);
    instruction.accounts[1].is_signer = false;

    let result = mollusk().process_instruction(
        &instruction,
        &[(fixture.market, market_account), (maker, wallet())],
    );

    assert!(result.program_result.is_err(), "unsigned order must fail");
}

#[test]
fn an_account_owned_by_another_program_is_not_a_market() {
    // Without the ownership check, a caller could pass a look-alike account it controls
    // and have the program credit balances into memory it can rewrite at will.
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    market_account.owner = Pubkey::new_from_array([77u8; 32]);
    let maker = trader(1);

    let result = mollusk().process_instruction(
        &post_only_ix(fixture.market, maker, Side::Ask, 100, 10),
        &[(fixture.market, market_account), (maker, wallet())],
    );

    assert!(result.program_result.is_err(), "foreign-owned account must be rejected");
}

#[test]
fn an_uninitialized_account_is_rejected() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    market_account.data[..8].fill(0); // clear the discriminator
    let maker = trader(1);

    let result = mollusk().process_instruction(
        &post_only_ix(fixture.market, maker, Side::Ask, 100, 10),
        &[(fixture.market, market_account), (maker, wallet())],
    );

    assert!(result.program_result.is_err(), "zeroed discriminator must be rejected");
}

#[test]
fn claiming_a_seat_is_idempotent() {
    let fixture = Fixture::new();
    let market_account = fixture.market_account(0);
    let newcomer = trader(9);

    let mollusk = mollusk();
    let first = mollusk.process_and_validate_instruction(
        &claim_seat_ix(fixture.market, newcomer),
        &[(fixture.market, market_account), (newcomer, wallet())],
        &[Check::success()],
    );
    let once = first.resulting_accounts[0].1.clone();
    assert_eq!(market_of(&once).traders().len(), 1);

    let second = mollusk.process_and_validate_instruction(
        &claim_seat_ix(fixture.market, newcomer),
        &[(fixture.market, once), (newcomer, wallet())],
        &[Check::success()],
    );
    assert_eq!(market_of(&second.resulting_accounts[0].1).traders().len(), 1);
}

#[test]
fn a_sweep_across_many_levels_settles_every_maker() {
    let fixture = Fixture::new();
    let mut market_account = fixture.market_account(0);
    let maker = trader(1);
    let taker = trader(2);
    fixture.seat(&mut market_account, maker, 10_000, 0);
    fixture.seat(&mut market_account, taker, 0, 10_000_000);
    seed_depth(&mut market_account, maker, Side::Ask, 100, 16, 5);

    let result = mollusk().process_and_validate_instruction(
        &market_order_ix(fixture.market, taker, Side::Bid, 80, 32),
        &[(fixture.market, market_account), (taker, wallet())],
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    assert_eq!(free_balances(after, taker).0, 80);
    assert!(market_of(after).book().is_empty(Side::Ask));
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}
