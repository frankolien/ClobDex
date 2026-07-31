//! The event sink.

use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::{ProgramError, ProgramResult};

use super::at;
use crate::error::ClobError;
use crate::state::LOG_AUTHORITY_SEED;

/// Accounts: log authority (signer).
///
/// Does nothing on purpose. The call exists so its data lands in the transaction's
/// inner instructions, where an indexer can read it in full — unlike program logs,
/// which are capped per transaction and truncated exactly when a sweep is deep enough
/// to be interesting.
///
/// The signer requirement is what makes an event trustworthy. Only this program can
/// make its own PDA a signer, so no other program — and no user transaction — can
/// produce a `LogEvent` carrying this program's id. Passing the real PDA address is not
/// enough; it has to actually sign.
pub fn log_event(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let authority = at(accounts, 0)?;
    if !authority.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let &bump = data.first().ok_or(ClobError::InstructionDataTooShort)?;
    let expected = Address::derive_address(&[LOG_AUTHORITY_SEED], Some(bump), program_id);
    if authority.address() != &expected {
        return Err(ClobError::InvalidLogAuthority.into());
    }
    Ok(())
}
