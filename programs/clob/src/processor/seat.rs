//! Seat claiming and moving tokens in and out.

use clob_engine::TraderKey;
use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::ProgramResult;
use pinocchio::instruction::cpi::{Seed, Signer};
use pinocchio_token::instructions::Transfer;

use super::{at, expect_address, expect_market_account, expect_signer, split_market};
use crate::dispatch_market;
use crate::error::{ClobError, map_engine};
use crate::instruction::Reader;
use crate::state::{SizeClass, VAULT_SIGNER_SEED, split_initialized};

/// Accounts: market, trader (signer).
///
/// Idempotent, so a client can call it unconditionally before its first order rather
/// than reading the market to find out whether it already has a seat.
pub fn claim(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let trader = at(rest, 0)?;
    expect_signer(trader)?;
    let key = TraderKey(to_bytes(trader.address()));

    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        map_engine(market.claim_seat(key))?;
    });
    Ok(())
}

/// Accounts: market, trader to evict (no signature required).
///
/// # Why this needs no signature
///
/// A seat table is finite, and a seat is free to claim. Without eviction, thirty-two
/// wallets could claim every seat on a small market, never trade, and permanently lock
/// out every maker — the seats would be held by exactly the people who have stopped
/// participating and are least likely to sign anything.
///
/// So eviction is permissionless, and safe because it only ever succeeds on a seat
/// holding nothing at all. Losing an empty seat costs its owner nothing: claiming is
/// idempotent and free, and `deposit` re-claims on the way in. There is no window to
/// exploit either, since claiming and funding happen in one transaction.
///
/// Note that "empty" covers resting orders too: posting locks funds, so a seat with a
/// live quote has a non-zero balance and cannot be evicted.
///
/// # Errors
///
/// [`EngineError::SeatNotFound`](clob_engine::EngineError::SeatNotFound) if the trader
/// has no seat, or [`EngineError::SeatNotEmpty`](clob_engine::EngineError::SeatNotEmpty)
/// if it still holds funds or orders.
pub fn evict(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let key = TraderKey(to_bytes(at(rest, 0)?.address()));

    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        map_engine(market.release_seat(&key))?;
    });
    Ok(())
}

/// Accounts: market, trader (signer), trader base token account, trader quote token
/// account, base vault, quote vault, token program.
///
/// Amounts are in *lots*, not atoms. Taking atoms would mean rounding down to whole
/// lots and stranding the remainder in the vault, where it would belong to nobody and
/// break the reconciliation between vault balance and recorded deposits. Lots convert
/// to atoms exactly.
pub fn deposit(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let base_lots = reader.base_lots()?;
    let quote_lots = reader.quote_lots()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let trader = at(rest, 0)?;
    let trader_base = at(rest, 1)?;
    let trader_quote = at(rest, 2)?;
    let base_vault = at(rest, 3)?;
    let quote_vault = at(rest, 4)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    expect_address(base_vault, &header.base_vault, ClobError::VaultMismatch)?;
    expect_address(quote_vault, &header.quote_vault, ClobError::VaultMismatch)?;

    let size_class = SizeClass::from_u64(header.size_class)?;
    let (base_atoms, quote_atoms) = dispatch_market!(size_class, market_bytes, |market| {
        let seat = map_engine(market.claim_seat(key))?;
        let config = *market.lot_config();
        map_engine(market.deposit(seat, base_lots, quote_lots))?;
        (
            config
                .base_atoms(base_lots)
                .ok_or(ClobError::AmountOverflow)?,
            config
                .quote_atoms(quote_lots)
                .ok_or(ClobError::AmountOverflow)?,
        )
    });
    drop(data);

    // Balances are credited before the transfer, so a failed transfer reverts the whole
    // instruction and the credit with it.
    if base_atoms.as_u64() > 0 {
        Transfer::new(trader_base, base_vault, trader, base_atoms.as_u64())
        .invoke()?;
    }
    if quote_atoms.as_u64() > 0 {
        Transfer::new(trader_quote, quote_vault, trader, quote_atoms.as_u64())
        .invoke()?;
    }
    Ok(())
}

/// Accounts: market, trader (signer), trader base token account, trader quote token
/// account, base vault, quote vault, vault signer, token program.
///
/// Only free balances can leave. Funds locked behind resting orders are untouchable
/// until those orders are cancelled, or a maker could withdraw the collateral out from
/// under a quote that is still live.
pub fn withdraw(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let base_lots = reader.base_lots()?;
    let quote_lots = reader.quote_lots()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let trader = at(rest, 0)?;
    let trader_base = at(rest, 1)?;
    let trader_quote = at(rest, 2)?;
    let base_vault = at(rest, 3)?;
    let quote_vault = at(rest, 4)?;
    let vault_signer = at(rest, 5)?;
    expect_signer(trader)?;

    let market_address = *market_account.address();
    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    expect_address(base_vault, &header.base_vault, ClobError::VaultMismatch)?;
    expect_address(quote_vault, &header.quote_vault, ClobError::VaultMismatch)?;
    let bump = [header.vault_signer_bump as u8];

    let size_class = SizeClass::from_u64(header.size_class)?;
    let (base_atoms, quote_atoms) = dispatch_market!(size_class, market_bytes, |market| {
        let seat = market.seat_index(&key);
        let config = *market.lot_config();
        map_engine(market.withdraw(seat, base_lots, quote_lots))?;
        (
            config
                .base_atoms(base_lots)
                .ok_or(ClobError::AmountOverflow)?,
            config
                .quote_atoms(quote_lots)
                .ok_or(ClobError::AmountOverflow)?,
        )
    });
    drop(data);

    let seeds = [
        Seed::from(VAULT_SIGNER_SEED),
        Seed::from(market_address.as_ref()),
        Seed::from(&bump[..]),
    ];
    let signer = Signer::from(&seeds[..]);

    if base_atoms.as_u64() > 0 {
        Transfer::new(base_vault, trader_base, vault_signer, base_atoms.as_u64())
        .invoke_signed(&[signer])?;
    }
    if quote_atoms.as_u64() > 0 {
        let signer = Signer::from(&seeds[..]);
        Transfer::new(quote_vault, trader_quote, vault_signer, quote_atoms.as_u64())
        .invoke_signed(&[signer])?;
    }
    Ok(())
}

#[inline(always)]
fn to_bytes(address: &Address) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(address.as_ref());
    bytes
}
