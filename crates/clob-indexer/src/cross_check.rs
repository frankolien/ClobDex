//! Checking a derived tape against an emitted receipt.
//!
//! Two independent accounts of the same transaction: one derived from the book by this
//! crate, one written by the program at the time. They should agree, and when they do
//! not, something is wrong that nobody would otherwise notice — a wire format change, a
//! missed instruction, a bug here.
//!
//! Only possible when the taker paid for a receipt, which is why it is a check rather
//! than the source.

use clob_book::{BaseLots, QuoteLots};
use clob_client::event::OrderPlaced;

use crate::tape::BookDelta;

/// Where a derived tape and an emitted receipt disagree.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Disagreement {
    /// The two report different total sizes.
    BaseLots {
        /// What the diff said.
        derived: BaseLots,
        /// What the program said.
        reported: BaseLots,
    },
    /// The two report different total values.
    QuoteLots {
        /// What the diff said.
        derived: QuoteLots,
        /// What the program said.
        reported: QuoteLots,
    },
    /// The two report a different number of fills.
    FillCount {
        /// What the diff said.
        derived: usize,
        /// What the program said.
        reported: u32,
    },
}

/// Compares a derived delta with the receipt the program emitted.
///
/// Aggregates are compared always. The per-fill count is compared only when the receipt
/// carried a complete tape — beyond its buffer the program drops detail and says so, and
/// a derived tape being *longer* than a truncated receipt is correct rather than wrong.
///
/// # Errors
///
/// The first [`Disagreement`] found.
pub fn agrees_with_event(delta: &BookDelta, event: &OrderPlaced) -> Result<(), Disagreement> {
    let derived_base = delta.base_lots_traded();
    if derived_base != event.base_lots_filled {
        return Err(Disagreement::BaseLots {
            derived: derived_base,
            reported: event.base_lots_filled,
        });
    }

    let derived_quote = delta.quote_lots_traded();
    if derived_quote != event.quote_lots_filled {
        return Err(Disagreement::QuoteLots {
            derived: derived_quote,
            reported: event.quote_lots_filled,
        });
    }

    if !event.truncated() && delta.trades.len() != event.fills_seen as usize {
        return Err(Disagreement::FillCount {
            derived: delta.trades.len(),
            reported: event.fills_seen,
        });
    }

    Ok(())
}
