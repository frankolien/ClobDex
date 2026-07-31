//! Event emission: it happens, it costs what it costs, and it cannot be forged.
//!
//! # What these tests do and do not prove
//!
//! The payload *encoding* is unit-tested byte by byte in [`clob_program::event`].
//!
//! Emission is proven here indirectly but soundly: `invoke_signed` fails if the signer,
//! seeds or accounts are wrong, and a failed CPI reverts the whole instruction. So a
//! receipt-form order that succeeds is one whose `LogEvent` call was accepted by the
//! runtime with a valid program-derived signature.
//!
//! Reading the bytes back out of a transaction record is covered separately, in
//! `event_roundtrip.rs`. Mollusk 0.14 cannot do it — its `inner_instructions` field is
//! behind a feature whose dependency does not resolve — so those tests run under
//! LiteSVM instead.

mod common;

use clob_book::Side;
use clob_engine::MatchStop;
use mollusk_svm::result::Check;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use common::*;

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}

/// A market with `depth` resting asks and a funded taker.
fn seeded(depth: u64) -> (Fixture, solana_account::Account) {
    let fixture = Fixture::new();
    let mut account = fixture.market_account(0);
    fixture.seat(&mut account, trader(1), 1_000_000, 1_000_000_000);
    fixture.seat(&mut account, trader(2), 0, 1_000_000_000);
    if depth > 0 {
        seed_depth(&mut account, trader(1), Side::Ask, 100, depth, 10);
    }
    (fixture, account)
}

/// Accounts a receipt-form order needs beyond the market and signer.
fn receipt_accounts(market: solana_account::Account, taker: Pubkey, fixture: &Fixture)
    -> Vec<(Pubkey, solana_account::Account)>
{
    let (authority, _) = log_authority();
    vec![
        (fixture.market, market),
        (taker, wallet()),
        (authority, wallet()),
        (PROGRAM_ID, mollusk_svm::program::create_program_account_loader_v3(&PROGRAM_ID)),
    ]
}

#[test]
fn the_plain_form_emits_nothing_and_still_trades() {
    // A market maker cancel-replaces continuously and already knows what it submitted;
    // it should not pay for a receipt it will not read.
    let (fixture, account) = seeded(1);
    let result = mollusk().process_and_validate_instruction(
        &market_order_ix(fixture.market, trader(2), Side::Bid, 10, 8),
        &[(fixture.market, account), (trader(2), wallet())],
        &[Check::success()],
    );

    assert_eq!(free_balances(&result.resulting_accounts[0].1, trader(2)).0, 10);
}

#[test]
fn the_receipt_form_emits_and_still_trades_correctly() {
    let (fixture, account) = seeded(3);
    let instruction = with_receipt(market_order_ix(fixture.market, trader(2), Side::Bid, 25, 8));

    let result = mollusk().process_and_validate_instruction(
        &instruction,
        &receipt_accounts(account, trader(2), &fixture),
        &[Check::success()],
    );

    // Success means the self-CPI was accepted with a valid PDA signature; a bad signer
    // or bad seeds would have reverted the whole instruction.
    let after = &result.resulting_accounts[0].1;
    assert_eq!(free_balances(after, trader(2)).0, 25);
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}

#[test]
fn a_deep_sweep_emits_without_overflowing_the_buffer() {
    // Beyond the buffer the per-fill detail is dropped, but the aggregate must stay
    // exact or an indexer reading the summary would be wrong rather than incomplete.
    let (fixture, account) = seeded(40);
    let instruction = with_receipt(market_order_ix(fixture.market, trader(2), Side::Bid, 400, 64));

    let result = mollusk().process_and_validate_instruction(
        &instruction,
        &receipt_accounts(account, trader(2), &fixture),
        &[Check::success()],
    );

    let after = &result.resulting_accounts[0].1;
    assert_eq!(free_balances(after, trader(2)).0, 400);
    assert!(market_of(after).book().is_empty(Side::Ask));
    assert_eq!(market_of(after).check_conservation(), Ok(()));
}

#[test]
fn a_user_cannot_forge_an_event() {
    // The whole security argument for the log authority: passing the right address is
    // not enough, it has to actually sign, and only this program can sign for its PDA.
    let (authority, bump) = log_authority();
    let fixture = Fixture::new();

    let forged = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(authority, false)],
        data: vec![9u8, bump, 1, 0, 0, 0],
    };

    let result = mollusk().process_instruction(
        &forged,
        &[(authority, wallet()), (fixture.market, fixture.market_account(0))],
    );

    assert!(result.program_result.is_err(), "unsigned log authority must be rejected");
}

#[test]
fn a_signed_impostor_is_rejected() {
    // An attacker signing with their own keypair instead of the real PDA.
    let impostor = Pubkey::new_from_array([200u8; 32]);
    let (_, bump) = log_authority();

    let forged = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(impostor, true)],
        data: vec![9u8, bump, 1, 0, 0, 0],
    };

    let result = mollusk().process_instruction(&forged, &[(impostor, wallet())]);

    assert!(result.program_result.is_err(), "wrong signer must be rejected");
}

#[test]
fn a_wrong_bump_is_rejected() {
    let (authority, bump) = log_authority();
    let wrong = if bump == 0 { 1 } else { bump - 1 };

    let forged = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(authority, true)],
        data: vec![9u8, wrong, 1, 0, 0, 0],
    };

    let result = mollusk().process_instruction(&forged, &[(authority, wallet())]);

    assert!(result.program_result.is_err(), "wrong bump must be rejected");
}

#[test]
fn the_receipt_costs_what_it_costs() {
    // Emitting is not free, and the number belongs where a caller can see it rather
    // than in a design doc.
    let (fixture, account) = seeded(1);
    let plain = mollusk().process_instruction(
        &market_order_ix(fixture.market, trader(2), Side::Bid, 10, 8),
        &[(fixture.market, account.clone()), (trader(2), wallet())],
    );

    let instruction = with_receipt(market_order_ix(fixture.market, trader(2), Side::Bid, 10, 8));
    let receipt = mollusk().process_instruction(
        &instruction,
        &receipt_accounts(account, trader(2), &fixture),
    );

    assert!(!plain.program_result.is_err());
    assert!(!receipt.program_result.is_err());
    println!(
        "single fill: {} CU plain, {} CU with receipt (+{})",
        plain.compute_units_consumed,
        receipt.compute_units_consumed,
        receipt.compute_units_consumed - plain.compute_units_consumed
    );
    // The gap is the CPI. That it is non-zero is the evidence the plain form really
    // does skip emission rather than emitting silently.
    assert!(receipt.compute_units_consumed > plain.compute_units_consumed);
    assert_eq!(MatchStop::FullyFilled as u8, 0, "stop codes are wire format");
}
