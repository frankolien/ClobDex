//! Market creation and fee collection.

use clob_book::LotConfig;
use clob_engine::FeeSchedule;
use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::{ProgramError, ProgramResult};
use pinocchio::instruction::cpi::{Seed, Signer};
use pinocchio_token::instructions::Transfer;

use super::{at, expect_address, expect_market_account, expect_signer, split_market};
use crate::dispatch_market;
use crate::error::{ClobError, map_engine};
use crate::instruction::Reader;
use crate::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
    VAULT_SIGNER_SEED, split_initialized,
};

/// Accounts, in order: market, base mint, quote mint, base vault, quote vault, vault
/// signer, authority (signer), fee recipient.
///
/// The market account must already exist, be owned by this program, and be exactly the
/// right size for its class. Allocation is the client's job — it needs a
/// `CreateAccount` from the system program with the payer's signature, and doing it
/// here would mean this instruction could not also be the one that validates the size.
pub fn initialize(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let size_class = SizeClass::from_u64(reader.u64()?)?;
    let lot_config = LotConfig {
        base_lots_per_base_unit: reader.u64()?,
        tick_size_in_quote_lots_per_base_unit: reader.u64()?,
        base_atoms_per_base_lot: reader.u64()?,
        quote_atoms_per_quote_lot: reader.u64()?,
    };
    let fees = FeeSchedule {
        taker_fee_bps: reader.u64()?,
    };
    let vault_signer_bump = reader.u8()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let base_mint = at(rest, 0)?;
    let quote_mint = at(rest, 1)?;
    let base_vault = at(rest, 2)?;
    let quote_vault = at(rest, 3)?;
    let vault_signer = at(rest, 4)?;
    let authority = at(rest, 5)?;
    let fee_recipient = at(rest, 6)?;
    expect_signer(authority)?;

    // The vault signer owns both vaults, so proving it is ours proves the vaults cannot
    // be drained by anyone else. Verified once here; every later instruction compares
    // against the addresses this handler records.
    let market_address = *market_account.address();
    let expected_signer = Address::derive_address(
        &[VAULT_SIGNER_SEED, market_address.as_ref()],
        Some(vault_signer_bump),
        program_id,
    );
    if vault_signer.address() != &expected_signer {
        return Err(ClobError::InvalidVaultSigner.into());
    }

    if market_account.data_len() != size_class.account_len() {
        return Err(ClobError::MarketAccountTooSmall.into());
    }

    let mut data = market_account.try_borrow_mut()?;
    let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);
    let header: &mut MarketAccountHeader = bytemuck::try_from_bytes_mut(header_bytes)
        .map_err(|_| ProgramError::from(ClobError::MarketDataUnaligned))?;

    if header.discriminator != 0 {
        return Err(ClobError::MarketAlreadyInitialized.into());
    }

    *header = MarketAccountHeader {
        discriminator: MARKET_DISCRIMINATOR,
        version: MARKET_VERSION,
        size_class: size_class as u64,
        vault_signer_bump: vault_signer_bump as u64,
        base_mint: to_bytes(base_mint.address()),
        quote_mint: to_bytes(quote_mint.address()),
        base_vault: to_bytes(base_vault.address()),
        quote_vault: to_bytes(quote_vault.address()),
        authority: to_bytes(authority.address()),
        fee_recipient: to_bytes(fee_recipient.address()),
    };

    // The account arrives zeroed and a zeroed market is already a valid empty market,
    // so this writes only the configuration and never walks the book arenas. It must
    // also never build a Market by value: at the Large size class that is 606 KiB on a
    // 4 KiB stack.
    dispatch_market!(size_class, market_bytes, |market| {
        map_engine(market.initialize(lot_config, fees))?;
    });
    Ok(())
}

/// Accounts, in order: market, quote vault, fee recipient token account, vault signer,
/// token program.
///
/// Permissionless: anyone may pay the compute to move fees to the recipient the market
/// already names, and there is nothing to gain by doing so. Requiring the authority's
/// signature would only add a liveness dependency.
pub fn collect_fees(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let quote_vault = at(rest, 0)?;
    let recipient = at(rest, 1)?;
    let vault_signer = at(rest, 2)?;

    let market_address = *market_account.address();
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;

    expect_address(quote_vault, &header.quote_vault, ClobError::VaultMismatch)?;
    expect_address(
        recipient,
        &header.fee_recipient,
        ClobError::FeeRecipientMismatch,
    )?;

    let size_class = SizeClass::from_u64(header.size_class)?;
    let quote_atoms_per_lot = dispatch_market!(size_class, market_bytes, |market| {
        market.lot_config().quote_atoms_per_quote_lot
    });
    let lots = dispatch_market!(size_class, market_bytes, |market| {
        map_engine(market.collect_fees())?
    });

    let amount = lots
        .as_u64()
        .checked_mul(quote_atoms_per_lot)
        .ok_or(ClobError::AmountOverflow)?;
    let bump = [header.vault_signer_bump as u8];
    drop(data);

    if amount > 0 {
        let seeds = [
            Seed::from(VAULT_SIGNER_SEED),
            Seed::from(market_address.as_ref()),
            Seed::from(&bump[..]),
        ];
        Transfer::new(quote_vault, recipient, vault_signer, amount)
            .invoke_signed(&[Signer::from(&seeds[..])])?;
    }
    Ok(())
}

#[inline(always)]
fn to_bytes(address: &Address) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(address.as_ref());
    bytes
}
