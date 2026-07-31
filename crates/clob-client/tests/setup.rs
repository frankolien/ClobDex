//! The hand-rolled SPL Token bits match the real program's encoding.
//!
//! `setup.rs` writes `InitializeAccount3` by hand and states the token account size as
//! a constant, rather than depending on `spl-token` — a large dependency for an SDK to
//! carry for one instruction and one number. These tests are what stop that shortcut
//! from rotting: `spl-token` is a dev-dependency here and nowhere else.

use clob_book::LotConfig;
use clob_client::setup::{CreateMarketParams, TOKEN_ACCOUNT_LEN, create_market};
use clob_program::state::SizeClass;
use solana_pubkey::Pubkey;

fn key(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn params() -> CreateMarketParams {
    CreateMarketParams {
        program_id: key(1),
        payer: key(2),
        market: key(3),
        base_vault: key(4),
        quote_vault: key(5),
        base_mint: key(6),
        quote_mint: key(7),
        authority: key(8),
        fee_recipient: key(9),
        size_class: SizeClass::Small,
        lot_config: LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
        taker_fee_bps: 2,
        market_rent_lamports: 1_000_000,
        vault_rent_lamports: 2_000_000,
    }
}

#[test]
fn the_token_account_size_matches_spl_token() {
    use spl_token::solana_program::program_pack::Pack;
    assert_eq!(TOKEN_ACCOUNT_LEN as usize, spl_token::state::Account::LEN);
}

#[test]
fn the_hand_rolled_initialize_matches_spl_tokens_own_encoder() {
    let p = params();
    let setup = create_market(&p);
    let vault_signer = clob_client::address::vault_signer(&p.program_id, &p.market).0;

    let expected = spl_token::instruction::initialize_account3(
        &spl_token::ID,
        &p.base_vault,
        &p.base_mint,
        &vault_signer,
    )
    .unwrap();

    // Index 2 is the base vault's initialisation.
    let built = &setup.instructions[2];
    assert_eq!(built.data, expected.data, "instruction data must match byte for byte");
    assert_eq!(built.program_id, expected.program_id);
    assert_eq!(
        built.accounts.iter().map(|a| a.pubkey).collect::<Vec<_>>(),
        expected.accounts.iter().map(|a| a.pubkey).collect::<Vec<_>>()
    );
}

#[test]
fn the_token_program_id_matches_spl_token() {
    assert_eq!(clob_client::TOKEN_PROGRAM_ID, spl_token::ID);
}
