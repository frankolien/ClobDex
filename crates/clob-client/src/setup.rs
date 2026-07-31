//! Creating a market from nothing.
//!
//! The program's `InitializeMarket` validates an account it is handed; it does not
//! allocate one. Allocation needs a `CreateAccount` signed by the payer *and* by the
//! new account's keypair, which a program cannot produce — and folding it in would mean
//! the same instruction both chooses the size and checks it, which is not a check.
//!
//! So a market takes six instructions, and this builds all six in the right order.
//!
//! # What the caller still owes
//!
//! Rent. Both amounts are parameters rather than constants because rent is a cluster
//! parameter the client reads from an RPC, and hard-coding today's value into an SDK is
//! how a deploy breaks eighteen months later. [`SizeClass::account_len`] and
//! [`TOKEN_ACCOUNT_LEN`] give the sizes to price.

use clob_book::LotConfig;
use clob_program::state::SizeClass;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_system_interface::instruction as system;

use crate::address::vault_signer;
use crate::instruction::{MarketAddresses, TOKEN_PROGRAM_ID, initialize_market};

/// Bytes in an SPL token account.
///
/// Part of the SPL Token program's fixed on-chain layout rather than an implementation
/// detail of any crate version, which is why it is stated here instead of pulling in a
/// dependency for one number. `tests/setup.rs` checks it against `spl_token`.
pub const TOKEN_ACCOUNT_LEN: u64 = 165;

/// `TokenInstruction::InitializeAccount3` — sets an account's mint and owner without
/// needing the rent sysvar or the owner's signature.
const INITIALIZE_ACCOUNT_3: u8 = 18;

/// What a new market needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CreateMarketParams {
    /// The ClobDex program.
    pub program_id: Pubkey,
    /// Pays rent for all three new accounts. Must sign.
    pub payer: Pubkey,
    /// Address of the market account. A fresh keypair; must sign.
    pub market: Pubkey,
    /// Address of the base vault. A fresh keypair; must sign.
    pub base_vault: Pubkey,
    /// Address of the quote vault. A fresh keypair; must sign.
    pub quote_vault: Pubkey,
    /// Base token mint.
    pub base_mint: Pubkey,
    /// Quote token mint.
    pub quote_mint: Pubkey,
    /// May change the fee recipient. Must sign. Cannot touch trader funds.
    pub authority: Pubkey,
    /// Receives swept fees.
    pub fee_recipient: Pubkey,
    /// Book and seat capacities.
    pub size_class: SizeClass,
    /// Tick and lot geometry.
    pub lot_config: LotConfig,
    /// Taker fee in basis points.
    pub taker_fee_bps: u64,
    /// Rent-exempt lamports for the market account.
    pub market_rent_lamports: u64,
    /// Rent-exempt lamports for each vault.
    pub vault_rent_lamports: u64,
}

/// The instructions that create a market, plus the addresses to use afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketSetup {
    /// In order. All six belong in one transaction so a half-created market cannot
    /// exist: an allocated but uninitialised market account is owned by the program and
    /// has no authority, so nobody could finish or reclaim it.
    pub instructions: Vec<Instruction>,
    /// Addresses for every later instruction on this market.
    pub addresses: MarketAddresses,
    /// Every key that must sign the transaction.
    pub signers: Vec<Pubkey>,
}

/// Builds the full market-creation sequence.
pub fn create_market(params: &CreateMarketParams) -> MarketSetup {
    let (vault_signer_address, _) = vault_signer(&params.program_id, &params.market);
    let addresses = MarketAddresses {
        program_id: params.program_id,
        market: params.market,
        base_vault: params.base_vault,
        quote_vault: params.quote_vault,
        vault_signer: vault_signer_address,
        token_program: TOKEN_PROGRAM_ID,
    };

    let mut instructions = vec![system::create_account(
        &params.payer,
        &params.market,
        params.market_rent_lamports,
        params.size_class.account_len() as u64,
        &params.program_id,
    )];

    for (vault, mint) in [
        (params.base_vault, params.base_mint),
        (params.quote_vault, params.quote_mint),
    ] {
        instructions.push(system::create_account(
            &params.payer,
            &vault,
            params.vault_rent_lamports,
            TOKEN_ACCOUNT_LEN,
            &TOKEN_PROGRAM_ID,
        ));
        // Both vaults are owned by the market's PDA, which is what makes them
        // unspendable by anyone but this program.
        instructions.push(initialize_token_account(&vault, &mint, &vault_signer_address));
    }

    instructions.push(initialize_market(
        &addresses,
        &params.base_mint,
        &params.quote_mint,
        &params.authority,
        &params.fee_recipient,
        params.size_class,
        &params.lot_config,
        params.taker_fee_bps,
    ));

    let mut signers = vec![
        params.payer,
        params.market,
        params.base_vault,
        params.quote_vault,
    ];
    if !signers.contains(&params.authority) {
        signers.push(params.authority);
    }

    MarketSetup {
        instructions,
        addresses,
        signers,
    }
}

/// `InitializeAccount3`, hand-encoded.
///
/// One byte of tag and a 32-byte owner. Written out rather than pulled from `spl-token`,
/// which would add a large dependency to an SDK for a fixed five-line encoding; the test
/// suite checks it against the real thing so the shortcut cannot rot.
fn initialize_token_account(account: &Pubkey, mint: &Pubkey, owner: &Pubkey) -> Instruction {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CreateMarketParams {
        CreateMarketParams {
            program_id: Pubkey::new_from_array([1u8; 32]),
            payer: Pubkey::new_from_array([2u8; 32]),
            market: Pubkey::new_from_array([3u8; 32]),
            base_vault: Pubkey::new_from_array([4u8; 32]),
            quote_vault: Pubkey::new_from_array([5u8; 32]),
            base_mint: Pubkey::new_from_array([6u8; 32]),
            quote_mint: Pubkey::new_from_array([7u8; 32]),
            authority: Pubkey::new_from_array([8u8; 32]),
            fee_recipient: Pubkey::new_from_array([9u8; 32]),
            size_class: SizeClass::Small,
            lot_config: LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
            taker_fee_bps: 2,
            market_rent_lamports: 1_000_000,
            vault_rent_lamports: 2_000_000,
        }
    }

    #[test]
    fn the_sequence_allocates_before_it_initialises() {
        let setup = create_market(&params());
        assert_eq!(setup.instructions.len(), 6);

        // Market allocation first, market initialisation last: everything the last
        // instruction validates has to exist by the time it runs.
        assert_eq!(setup.instructions[0].program_id, solana_system_interface::program::ID);
        assert_eq!(setup.instructions[5].program_id, params().program_id);
        // Vault allocation always immediately precedes its own initialisation.
        assert_eq!(setup.instructions[1].program_id, solana_system_interface::program::ID);
        assert_eq!(setup.instructions[2].program_id, TOKEN_PROGRAM_ID);
    }

    #[test]
    fn the_market_account_is_allocated_at_its_declared_size() {
        // A wrong size here is rejected by the program rather than silently truncating
        // the book, so the two have to agree.
        let setup = create_market(&params());
        let space = u64::from_le_bytes(setup.instructions[0].data[12..20].try_into().unwrap());
        assert_eq!(space, SizeClass::Small.account_len() as u64);
    }

    #[test]
    fn both_vaults_are_owned_by_the_market_pda() {
        let setup = create_market(&params());
        let expected = vault_signer(&params().program_id, &params().market).0;

        for index in [2, 4] {
            assert_eq!(setup.instructions[index].data[0], INITIALIZE_ACCOUNT_3);
            assert_eq!(&setup.instructions[index].data[1..33], expected.as_ref());
        }
    }

    #[test]
    fn every_new_account_signs() {
        let setup = create_market(&params());
        for key in [params().market, params().base_vault, params().quote_vault] {
            assert!(setup.signers.contains(&key), "{key} must sign");
        }
        assert!(setup.signers.contains(&params().authority));
    }

    #[test]
    fn an_authority_that_is_also_the_payer_is_not_listed_twice() {
        let mut p = params();
        p.authority = p.payer;
        let setup = create_market(&p);

        assert_eq!(setup.signers.len(), 4);
    }
}
