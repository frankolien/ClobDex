//! Integer quantities and the lot/tick geometry that gives them meaning.
//!
//! Matching must be bit-for-bit reproducible across every validator replaying the
//! transaction, so there are no floats anywhere in this crate. Every price and size is
//! an integer, and a market's [`LotConfig`] defines what those integers denote.
//!
//! | Unit | Meaning |
//! |---|---|
//! | [`BaseAtoms`] / [`QuoteAtoms`] | Raw SPL amounts — what actually moves between vaults. |
//! | [`BaseLots`] | Minimum tradable size increment. |
//! | [`QuoteLots`] | Minimum quote-value increment. |
//! | [`Ticks`] | Price, quoted per *base unit* (one whole token), not per base lot. |
//!
//! Pricing per base unit rather than per base lot is what keeps quotes legible: a
//! SOL/USDC market quotes ~$200 per SOL, not per 0.001 SOL.

mod lots;
mod units;

pub use lots::{LotConfig, LotConfigError};
pub use units::{BaseAtoms, BaseLots, QuoteAtoms, QuoteLots, Ticks};
