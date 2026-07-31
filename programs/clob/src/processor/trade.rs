//! Order entry. These are the compute-critical instructions.
//!
//! The base form of every handler here takes two accounts: the market and the trader's
//! signature. No token accounts, no vaults, no token program — funds were deposited
//! beforehand and settlement happens inside the market account.
//!
//! That is the whole point of pre-funded trading. A venue that moves tokens on every
//! fill needs the taker's two token accounts, the two vaults and the token program on
//! every order, and aggregators price account count into their routing decisions.
//!
//! [`place_order`] additionally accepts two optional accounts that turn on an event
//! receipt. See its documentation for why that is opt-in rather than always on.

use clob_engine::{OrderOutcome, TraderKey};
use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::ProgramResult;
use pinocchio::instruction::cpi::{Seed, Signer, invoke_signed};
use pinocchio::instruction::{InstructionAccount, InstructionView};

use super::{at, expect_market_account, expect_signer, split_market};
use crate::dispatch_market;
use crate::error::map_engine;
use crate::event::{EventBuffer, MAX_EVENT_LEN};
use crate::instruction::{Discriminant, Reader};
use crate::state::{LOG_AUTHORITY_SEED, SizeClass, split_initialized};

/// Accounts: market, trader (signer). Optionally followed by log authority and this
/// program, which turns on the event receipt.
///
/// # Two forms
///
/// The plain form is two accounts and emits nothing. A market maker cancel-replaces
/// continuously and already knows what it submitted, so paying to load two more
/// accounts and run a CPI on every quote update buys it nothing.
///
/// The receipt form appends the log authority and this program, and emits an event
/// carrying the fills. Takers and aggregators want that; makers do not. Making it
/// opt-in by account count keeps the cheap path cheap without a second discriminant.
///
/// The receipt form also carries a trailing log-authority bump byte. A wrong bump
/// derives a different address than the account passed, and the runtime rejects the
/// signed CPI — so it fails loudly rather than emitting a forgeable event.
pub fn place_order(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let packet = reader.order_packet()?;
    let log_bump = reader.optional_u8();

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;
    let trader = at(rest, 0)?;
    expect_signer(trader)?;

    let key = TraderKey(to_bytes(trader.address()));
    let mut events = EventBuffer::new();

    // Scoped so the market borrow is released before the CPI: the runtime rejects a
    // cross-program invocation while an account's data is still borrowed.
    let (outcome, seat) = {
        let mut data = market_account.try_borrow_mut()?;
        let (header, market_bytes) = split_initialized(&mut data)?;
        let size_class = SizeClass::from_u64(header.size_class)?;

        dispatch_market!(size_class, market_bytes, |market| {
            let seat = market.seat_index(&key);
            let outcome = map_engine(market.place_order(seat, packet, &mut events))?;
            (outcome, seat)
        })
    };

    if let (Some(bump), Ok(authority)) = (log_bump, at(rest, 1)) {
        emit(program_id, authority, bump, &events, &outcome, seat)?;
    }
    Ok(())
}

/// Calls back into this program so the event lands in inner instruction data.
fn emit(
    program_id: &Address,
    authority: &AccountView,
    bump: u8,
    events: &EventBuffer,
    outcome: &OrderOutcome,
    seat: u32,
) -> ProgramResult {
    // Discriminant, then the bump the handler re-derives from, then the payload.
    let mut data = [0u8; 2 + MAX_EVENT_LEN];
    data[0] = Discriminant::LogEvent as u8;
    data[1] = bump;
    let len = 2 + events.encode(outcome, seat, &mut data[2..]);

    let metas = [InstructionAccount {
        address: authority.address(),
        is_writable: false,
        is_signer: true,
    }];
    let instruction = InstructionView {
        program_id,
        accounts: &metas,
        data: &data[..len],
    };

    let bump_seed = [bump];
    let seeds = [
        Seed::from(LOG_AUTHORITY_SEED),
        Seed::from(&bump_seed[..]),
    ];
    invoke_signed(&instruction, &[authority], &[Signer::from(&seeds[..])])
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
