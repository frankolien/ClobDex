//! From a correlated change to a derived delta.
//!
//! This is the seam where the streaming half hands off to [`clob_indexer`], which owns
//! all of the derivation logic and is tested against the engine. Nothing here decides
//! what a trade is; it decodes, attributes, and delegates.

use anyhow::{Context, Result};
use clob_client::state::MarketState;
use clob_indexer::{BookDelta, ObservedInstruction, derive};
use solana_pubkey::Pubkey;

use crate::attribute;
use crate::correlate::Change;

/// A derived change, with the state it produced.
#[derive(Clone, Debug)]
pub struct Derived {
    /// Which market.
    pub market: Pubkey,
    /// Slot it landed in.
    pub slot: u64,
    /// The transaction.
    pub signature: [u8; 64],
    /// What it did to the book.
    pub delta: BookDelta,
    /// The market as it now stands.
    pub state: MarketState,
}

/// What came of processing one change.
///
/// Two outcomes rather than an `Option`, because "there was nothing to diff against" and
/// "nothing happened" are different facts and the caller must treat them differently: a
/// baseline still has to be recorded, or the market stays invisible until its second
/// transaction.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// First sighting of a market. Its state is now known; nothing was derived.
    Baseline {
        /// Which market.
        market: Pubkey,
        /// Slot the state came from.
        slot: u64,
        /// The market as it stands.
        state: MarketState,
    },
    /// A change that could be diffed against a known earlier state.
    Derived(Derived),
}

/// Derives what one change did.
///
/// The first update for a market yields [`Outcome::Baseline`]: there is nothing to diff
/// against, and reporting the whole resting book as newly posted would publish a burst
/// of fictional activity on every restart.
pub fn process(change: &Change, program_id: &Pubkey) -> Result<Outcome> {
    let after = MarketState::decode(&change.after)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .context("decoding the market after the transaction")?;

    let Some(before_bytes) = &change.before else {
        return Ok(Outcome::Baseline {
            market: change.market,
            slot: change.slot,
            state: after,
        });
    };

    let before = MarketState::decode(before_bytes)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .context("decoding the market before the transaction")?;

    // Attribution reads the earlier state: a seat claimed in this transaction owned none
    // of the liquidity it consumed.
    let observed: Vec<ObservedInstruction> = change
        .instructions
        .iter()
        .filter(|instruction| attribute::market_of(instruction) == Some(&change.market))
        .filter_map(|instruction| attribute::observe(instruction, program_id, &before))
        .collect();

    let delta = derive(&before, &after, &observed, change.slot);
    Ok(Outcome::Derived(Derived {
        market: change.market,
        slot: change.slot,
        signature: change.signature,
        delta,
        state: after,
    }))
}

/// Whether a derived delta agrees with the fee the market recorded.
///
/// A free audit on every fee-charging market: the fee counter is written by the program,
/// and the trades are derived independently from the book. Disagreement means the
/// derivation invented, missed, or mispriced a fill.
pub fn reconciles(derived: &Derived) -> bool {
    derived
        .delta
        .fees_reconcile(derived.state.fees().taker_fee_bps)
}
