//! Order entry. These are the compute-critical instructions.
//!
//! Every handler here takes exactly two accounts: the market and the trader's
//! signature. No token accounts, no vaults, no token program — because funds were
//! already deposited and settlement happens inside the market account.
//!
//! That is the whole point of pre-funded trading. A venue that moves tokens on every
//! fill needs the taker's two token accounts, the two vaults and the token program on
//! every order, and aggregators price account count into their routing decisions.

use clob_engine::TraderKey;
use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::ProgramResult;

use super::{at, expect_market_account, expect_signer, split_market};
use crate::dispatch_market;
use crate::error::map_engine;
use crate::instruction::Reader;
use crate::state::{SizeClass, split_initialized};

/// Accounts: market, trader (signer).
pub fn place_order(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let packet = reader.order_packet()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;
    let trader = at(rest, 0)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        let seat = market.seat_index(&key);
        map_engine(market.place_order(seat, packet, &mut ()))?;
    });
    Ok(())
}

/// Accounts: market, trader (signer).
pub fn cancel_order(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let order_id = reader.order_id()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;
    let trader = at(rest, 0)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        let seat = market.seat_index(&key);
        map_engine(market.cancel_order(seat, &order_id))?;
    });
    Ok(())
}

/// Accounts: market, trader (signer).
pub fn reduce_order(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let order_id = reader.order_id()?;
    let base_lots = reader.base_lots()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;
    let trader = at(rest, 0)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        let seat = market.seat_index(&key);
        map_engine(market.reduce_order(seat, &order_id, base_lots))?;
    });
    Ok(())
}

/// Accounts: market, trader (signer).
///
/// The caller supplies the bound. An unbounded cancel-all on a deep book can exceed the
/// compute budget and revert, leaving a maker unable to pull quotes at exactly the
/// moment it most needs to; a bounded call always makes progress.
pub fn cancel_all(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let side = reader.side()?;
    let limit = reader.u32()?;

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;
    let trader = at(rest, 0)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut data = market_account.try_borrow_mut()?;
    let (header, market_bytes) = split_initialized(&mut data)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    dispatch_market!(size_class, market_bytes, |market| {
        let seat = market.seat_index(&key);
        map_engine(market.cancel_orders_for_seat(seat, side, limit))?;
    });
    Ok(())
}

#[inline(always)]
fn to_bytes(address: &Address) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(address.as_ref());
    bytes
}
