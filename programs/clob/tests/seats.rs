//! Seat lifecycle, and the denial of service that eviction exists to prevent.

mod common;

use clob_book::{BaseLots, QuoteLots, Side};
use clob_client::instruction as sdk;
use clob_client::state::MarketState;
use clob_engine::TraderKey;
use mollusk_svm::result::Check;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use common::*;

fn trader(id: u8) -> Pubkey {
    Pubkey::new_from_array([100 + id; 32])
}

fn evict_ix(market: Pubkey, target: Pubkey) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(target, false),
        ],
        data: vec![11u8],
    }
}

#[test]
fn an_empty_seat_can_be_evicted_by_anyone() {
    // No signature from the evicted trader, and none needed: an empty seat costs its
    // owner nothing to lose, and requiring one would leave the market's liveness in the
    // hands of someone who has already stopped participating.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let idle = trader(1);
    fixture.seat(&mut market, idle, 0, 0);
    assert_eq!(market_of(&market).traders().len(), 1);

    let result = mollusk().process_and_validate_instruction(
        &evict_ix(fixture.market, idle),
        &[(fixture.market, market), (idle, wallet())],
        &[Check::success()],
    );

    assert_eq!(market_of(&result.resulting_accounts[0].1).traders().len(), 0);
}

#[test]
fn a_seat_holding_funds_cannot_be_evicted() {
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let funded = trader(1);
    fixture.seat(&mut market, funded, 1, 0);

    let result = mollusk().process_instruction(
        &evict_ix(fixture.market, funded),
        &[(fixture.market, market), (funded, wallet())],
    );

    assert!(result.program_result.is_err(), "a funded seat must survive eviction");
}

#[test]
fn a_seat_with_a_resting_order_cannot_be_evicted() {
    // Covered by the balance rule rather than a separate check: posting locks funds, so
    // a live quote always means a non-zero balance. Worth pinning, because if that
    // stopped being true a maker's orders could be orphaned.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    let maker = trader(1);
    fixture.seat(&mut market, maker, 100, 0);
    seed_depth(&mut market, maker, Side::Ask, 100, 1, 100);

    // Every lot the maker owns is now locked behind the quote, so free balances are zero.
    let state = market_of(&market).traders().state(
        market_of(&market).seat_index(&TraderKey(maker.to_bytes())),
    ).unwrap();
    assert_eq!(state.base_lots_free, BaseLots::ZERO);
    assert_eq!(state.quote_lots_free, QuoteLots::ZERO);

    let result = mollusk().process_instruction(
        &evict_ix(fixture.market, maker),
        &[(fixture.market, market), (maker, wallet())],
    );

    assert!(
        result.program_result.is_err(),
        "locked funds must still count as occupancy"
    );
}

#[test]
fn evicting_a_seat_that_does_not_exist_is_refused() {
    let fixture = Fixture::new();
    let market = fixture.market_account(0);

    let result = mollusk().process_instruction(
        &evict_ix(fixture.market, trader(9)),
        &[(fixture.market, market), (trader(9), wallet())],
    );

    assert!(result.program_result.is_err());
}

#[test]
fn a_full_table_can_be_reopened_by_evicting_squatters() {
    // The denial of service this instruction exists to prevent. Thirty-two wallets
    // claim every seat on a small market and never trade; without eviction no maker
    // could ever join, permanently.
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    for i in 0..32u8 {
        fixture.seat(&mut market, trader(i), 0, 0);
    }

    let newcomer = Pubkey::new_from_array([200u8; 32]);
    let mollusk = mollusk();
    let blocked = mollusk.process_instruction(
        &claim_seat_ix(fixture.market, newcomer),
        &[(fixture.market, market.clone()), (newcomer, wallet())],
    );
    assert!(blocked.program_result.is_err(), "the table should be full");

    // The SDK names the squatters rather than making a caller guess.
    let state = MarketState::decode(&market.data).unwrap();
    assert!(state.seats_are_full());
    assert_eq!(state.evictable_seats().len(), 32);

    let victim = Pubkey::new_from_array(state.evictable_seats()[0].0);
    let evicted = mollusk.process_and_validate_instruction(
        &evict_ix(fixture.market, victim),
        &[(fixture.market, market), (victim, wallet())],
        &[Check::success()],
    );

    // And now a real maker can get in.
    let joined = mollusk.process_and_validate_instruction(
        &claim_seat_ix(fixture.market, newcomer),
        &[
            (fixture.market, evicted.resulting_accounts[0].1.clone()),
            (newcomer, wallet()),
        ],
        &[Check::success()],
    );
    let after = market_of(&joined.resulting_accounts[0].1);
    assert_eq!(after.traders().len(), 32);
    assert_ne!(after.seat_index(&TraderKey(newcomer.to_bytes())), clob_engine::NO_SEAT);
}

#[test]
fn a_trader_with_a_balance_is_never_listed_as_evictable() {
    let fixture = Fixture::new();
    let mut market = fixture.market_account(0);
    fixture.seat(&mut market, trader(1), 0, 0);
    fixture.seat(&mut market, trader(2), 500, 0);
    seed_depth(&mut market, trader(2), Side::Ask, 100, 1, 100);

    let state = MarketState::decode(&market.data).unwrap();
    let evictable = state.evictable_seats();

    assert_eq!(evictable.len(), 1);
    assert_eq!(evictable[0], TraderKey(trader(1).to_bytes()));
    assert!(!state.seats_are_full());
    assert_eq!(state.seat_capacity(), 32);
}

#[test]
fn the_sdk_builder_matches_the_hand_written_instruction() {
    let fixture = Fixture::new();
    let addresses = sdk_addresses(&fixture);
    let built = sdk::evict_seat(&addresses, &trader(1));
    let hand = evict_ix(fixture.market, trader(1));

    assert_eq!(built.data, hand.data);
    assert_eq!(built.program_id, hand.program_id);
    assert_eq!(
        built.accounts.iter().map(|a| (a.pubkey, a.is_signer)).collect::<Vec<_>>(),
        hand.accounts.iter().map(|a| (a.pubkey, a.is_signer)).collect::<Vec<_>>()
    );
}

