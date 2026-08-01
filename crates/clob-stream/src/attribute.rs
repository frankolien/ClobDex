//! Turning raw instructions into ones an observer can attribute.
//!
//! [`clob_indexer::derive`] needs to know who submitted each instruction, because the
//! owner is the only thing separating a self-trade from a fill. The instruction data
//! carries no trader — it is an account — so it is read out of the account list here.

use clob_client::decode::{self, ClobInstruction};
use clob_client::state::MarketState;
use clob_engine::TraderKey;
use clob_indexer::ObservedInstruction;
use solana_pubkey::Pubkey;

use crate::source::RawInstruction;

/// Where the market appears in every instruction's account list.
pub const MARKET_ACCOUNT_INDEX: usize = 0;

/// Where the submitting trader appears.
///
/// Fixed across every trader-facing instruction the program defines, and checked against
/// the SDK builders in `tests/attribution.rs` so a reordering cannot quietly change who
/// a trade is attributed to.
pub const TRADER_ACCOUNT_INDEX: usize = 1;

/// Decodes one instruction and resolves its submitter to a seat.
///
/// `state` is the market *before* the transaction: a trader who claimed their seat in
/// this same transaction did not own any of the liquidity it consumed, so resolving
/// against the earlier state is both correct and the only thing available in order.
///
/// Returns `None` for instructions addressed elsewhere, or data this program never
/// produced — an observer that guessed at unparseable bytes would be inventing history.
pub fn observe(
    instruction: &RawInstruction,
    program_id: &Pubkey,
    state: &MarketState,
) -> Option<ObservedInstruction> {
    if &instruction.program_id != program_id {
        return None;
    }
    let decoded = decode::decode(&instruction.data).ok()?;

    match seat_of(instruction, state) {
        Some(seat) => Some(ObservedInstruction::new(decoded, seat)),
        // A trader with no seat cannot own resting liquidity, so nothing it consumes can
        // be a self-trade. Anonymous is the honest answer and the safe one: the
        // derivation treats unresolved ownership as traded rather than as self-traded.
        None => Some(ObservedInstruction::anonymous(decoded)),
    }
}

/// The seat held by whoever submitted this instruction.
fn seat_of(instruction: &RawInstruction, state: &MarketState) -> Option<u32> {
    let trader = instruction.account(TRADER_ACCOUNT_INDEX)?;
    state.seat_of(&TraderKey(trader.to_bytes()))
}

/// Which market an instruction addresses.
pub fn market_of(instruction: &RawInstruction) -> Option<&Pubkey> {
    instruction.account(MARKET_ACCOUNT_INDEX)
}

/// Whether a decoded instruction concerns the book at all.
///
/// `Deposit`, `Withdraw`, `ClaimSeat` and the rest move funds without touching resting
/// liquidity, so a transaction containing only those cannot have produced a trade.
pub fn touches_the_book(instruction: &ClobInstruction) -> bool {
    matches!(
        instruction,
        ClobInstruction::PlaceOrder { .. }
            | ClobInstruction::Swap { .. }
            | ClobInstruction::CancelOrder { .. }
            | ClobInstruction::ReduceOrder { .. }
            | ClobInstruction::CancelAllOrders { .. }
    )
}
