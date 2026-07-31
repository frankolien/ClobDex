//! Program-derived addresses.
//!
//! Both derivations are also done on-chain, but with the bump supplied rather than
//! searched for — `find_program_address` loops until it finds an off-curve address and
//! costs on the order of a thousand compute units. Doing the search here and passing the
//! answer down is why order entry is as cheap as it is.

use clob_program::state::{LOG_AUTHORITY_SEED, VAULT_SIGNER_SEED};
use solana_pubkey::Pubkey;

/// The authority owning a market's two token vaults, and its bump.
///
/// One signer for both vaults rather than one each: a vault is only ever moved by this
/// authority, so a second PDA would be a second thing to derive and pass for no gain.
pub fn vault_signer(program_id: &Pubkey, market: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SIGNER_SEED, market.as_ref()], program_id)
}

/// The program-wide event signer, and its bump.
///
/// Not market-scoped, because its only job is to prove that an event came from this
/// program. Which market the event concerns is already in the transaction's accounts.
pub fn log_authority(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[LOG_AUTHORITY_SEED], program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivations_are_deterministic_and_distinct() {
        let program = Pubkey::new_from_array([1u8; 32]);
        let market = Pubkey::new_from_array([2u8; 32]);

        assert_eq!(vault_signer(&program, &market), vault_signer(&program, &market));
        assert_ne!(vault_signer(&program, &market).0, log_authority(&program).0);
    }

    #[test]
    fn each_market_gets_its_own_vault_signer() {
        // Otherwise one market's authority could move another's vaults.
        let program = Pubkey::new_from_array([1u8; 32]);
        let a = vault_signer(&program, &Pubkey::new_from_array([2u8; 32]));
        let b = vault_signer(&program, &Pubkey::new_from_array([3u8; 32]));

        assert_ne!(a.0, b.0);
    }
}
