//! Integer quantities.
//!
//! Matching must be bit-for-bit reproducible across every validator replaying the
//! transaction, so there are no floats anywhere in this crate. Every price and size is
//! an integer.
//!
//! | Unit | Meaning |
//! |---|---|
//! | [`BaseAtoms`] / [`QuoteAtoms`] | Raw SPL amounts — what actually moves between vaults. |
//! | [`BaseLots`] | Minimum tradable size increment. |
//! | [`QuoteLots`] | Minimum quote-value increment. |
//! | [`Ticks`] | Price, quoted per *base unit* (one whole token), not per base lot. |

mod units;

pub use units::{BaseAtoms, BaseLots, QuoteAtoms, QuoteLots, Ticks};
