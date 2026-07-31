//! Instruction dispatch and the account checks every handler shares.

mod log;
mod market;
mod seat;
mod swap;
mod trade;

use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::{ProgramError, ProgramResult};

use crate::error::ClobError;
use crate::instruction::{Discriminant, Reader};

/// Entrypoint body: reads the discriminant and hands off.
pub fn process(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (&tag, rest) = data
        .split_first()
        .ok_or(ClobError::InstructionDataTooShort)?;
    let mut reader = Reader::new(rest);

    match Discriminant::parse(tag)? {
        Discriminant::InitializeMarket => market::initialize(program_id, accounts, &mut reader),
        Discriminant::CollectFees => market::collect_fees(program_id, accounts),
        Discriminant::ClaimSeat => seat::claim(program_id, accounts),
        Discriminant::Deposit => seat::deposit(program_id, accounts, &mut reader),
        Discriminant::Withdraw => seat::withdraw(program_id, accounts, &mut reader),
        Discriminant::PlaceOrder => trade::place_order(program_id, accounts, &mut reader),
        Discriminant::CancelOrder => trade::cancel_order(program_id, accounts, &mut reader),
        Discriminant::ReduceOrder => trade::reduce_order(program_id, accounts, &mut reader),
        Discriminant::CancelAllOrders => trade::cancel_all(program_id, accounts, &mut reader),
        Discriminant::LogEvent => log::log_event(program_id, accounts, rest),
        Discriminant::Swap => swap::swap(program_id, accounts, &mut reader),
    }
}

/// Splits the account slice into the market and everything after it.
///
/// The market is always account zero, so every handler starts the same way.
///
/// # Errors
///
/// [`ProgramError::NotEnoughAccountKeys`].
pub(crate) fn split_market(
    accounts: &mut [AccountView],
) -> Result<(&mut AccountView, &mut [AccountView]), ProgramError> {
    accounts
        .split_first_mut()
        .ok_or(ProgramError::NotEnoughAccountKeys)
}

/// Borrows an account by position.
///
/// # Errors
///
/// [`ProgramError::NotEnoughAccountKeys`].
pub(crate) fn at(accounts: &[AccountView], index: usize) -> Result<&AccountView, ProgramError> {
    accounts.get(index).ok_or(ProgramError::NotEnoughAccountKeys)
}

/// Checks that the market account is writable and owned by this program.
///
/// Ownership is the check that matters: without it a caller could pass a
/// look-alike account it controls and have the program credit balances in memory it
/// can rewrite at will.
///
/// # Errors
///
/// [`ProgramError::InvalidAccountOwner`] or [`ProgramError::InvalidArgument`].
pub(crate) fn expect_market_account(
    market: &AccountView,
    program_id: &Address,
) -> Result<(), ProgramError> {
    if !market.owned_by(program_id) {
        return Err(ProgramError::InvalidAccountOwner);
    }
    if !market.is_writable() {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(())
}

/// Checks that an account signed the transaction.
///
/// # Errors
///
/// [`ProgramError::MissingRequiredSignature`].
pub(crate) fn expect_signer(account: &AccountView) -> Result<(), ProgramError> {
    if account.is_signer() {
        Ok(())
    } else {
        Err(ProgramError::MissingRequiredSignature)
    }
}

/// Checks an account against an address recorded in the market header.
///
/// Comparing against a stored address is a 32-byte memcmp. Re-deriving the address with
/// `find_program_address` would cost on the order of a thousand compute units, on every
/// instruction, to learn something the market already knows.
///
/// # Errors
///
/// `error` if the addresses differ.
pub(crate) fn expect_address(
    account: &AccountView,
    expected: &[u8; 32],
    error: ClobError,
) -> Result<(), ProgramError> {
    if account.address().as_ref() == expected.as_slice() {
        Ok(())
    } else {
        Err(error.into())
    }
}
