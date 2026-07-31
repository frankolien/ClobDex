//! The three SPL Token instructions this CLI needs.
//!
//! Hand-encoded for the same reason [`clob_client::setup`] hand-encodes
//! `InitializeAccount3`: `spl-token` re-exports its own `solana_program`, so its
//! `Pubkey` is a different type from ours and every call site would need a conversion.
//! Three fixed encodings are cheaper than that, and the tests below check all three
//! against the real program so the shortcut cannot rot.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use clob_client::instruction::TOKEN_PROGRAM_ID;

/// Bytes in an SPL mint account.
pub const MINT_LEN: u64 = 82;

const INITIALIZE_MINT_2: u8 = 20;
const INITIALIZE_ACCOUNT_3: u8 = 18;
const MINT_TO: u8 = 7;

/// Sets a freshly allocated account up as a mint.
///
/// No freeze authority: a freezable test token would let the mint authority halt a
/// market, which is not the thing being tested here.
pub fn initialize_mint(mint: &Pubkey, authority: &Pubkey, decimals: u8) -> Instruction {
    let mut data = Vec::with_capacity(35);
    data.push(INITIALIZE_MINT_2);
    data.push(decimals);
    data.extend_from_slice(authority.as_ref());
    data.push(0); // COption::None

    Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// Sets a freshly allocated account up as a token account.
pub fn initialize_account(account: &Pubkey, mint: &Pubkey, owner: &Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(33);
    data.push(INITIALIZE_ACCOUNT_3);
    data.extend_from_slice(owner.as_ref());

    Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new_readonly(*mint, false),
        ],
        data,
    }
}

/// Mints `amount` atoms into `account`.
pub fn mint_to(mint: &Pubkey, account: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(MINT_TO);
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*account, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_token::solana_program::program_pack::Pack;
    use spl_token::solana_program::pubkey::Pubkey as SplPubkey;

    fn key(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn spl_key(byte: u8) -> SplPubkey {
        SplPubkey::new_from_array([byte; 32])
    }

    #[test]
    fn the_mint_size_matches_spl_token() {
        assert_eq!(MINT_LEN as usize, spl_token::state::Mint::LEN);
    }

    #[test]
    fn initialize_mint_matches_spl_token() {
        let ours = initialize_mint(&key(1), &key(2), 9);
        let theirs =
            spl_token::instruction::initialize_mint2(&spl_token::ID, &spl_key(1), &spl_key(2), None, 9)
                .unwrap();

        assert_eq!(ours.data, theirs.data);
        assert_eq!(ours.accounts.len(), theirs.accounts.len());
    }

    #[test]
    fn initialize_account_matches_spl_token() {
        let ours = initialize_account(&key(1), &key(2), &key(3));
        let theirs = spl_token::instruction::initialize_account3(
            &spl_token::ID,
            &spl_key(1),
            &spl_key(2),
            &spl_key(3),
        )
        .unwrap();

        assert_eq!(ours.data, theirs.data);
        assert_eq!(ours.accounts.len(), theirs.accounts.len());
    }

    #[test]
    fn mint_to_matches_spl_token() {
        let ours = mint_to(&key(1), &key(2), &key(3), 42);
        let theirs = spl_token::instruction::mint_to(
            &spl_token::ID,
            &spl_key(1),
            &spl_key(2),
            &spl_key(3),
            &[],
            42,
        )
        .unwrap();

        assert_eq!(ours.data, theirs.data);
        assert_eq!(ours.accounts.len(), theirs.accounts.len());
    }

    #[test]
    fn our_token_program_id_is_the_real_one() {
        assert_eq!(TOKEN_PROGRAM_ID.to_bytes(), spl_token::ID.to_bytes());
    }
}
