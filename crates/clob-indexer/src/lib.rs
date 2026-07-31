//! Deriving the trade tape and book deltas for a ClobDex market.
//!
//! This is the part of an indexer that turns on-chain state into something queryable,
//! separated from the part that talks to a gRPC endpoint and a database — because this
//! part can be tested exhaustively and that part cannot.
//!
//! # The tape does not depend on events
//!
//! Event receipts are opt-in and cost about 1,500 compute units, so a market maker will
//! not emit them. An indexer that needed events would therefore see a partial tape.
//!
//! Instead, [`derive`](derive::derive) reads the tape out of the book itself: liquidity
//! that disappeared between two snapshots, attributed using the transaction's
//! instructions. Receipts, when a taker does pay for one, become a cross-check rather
//! than a dependency — see [`agrees_with_event`](cross_check::agrees_with_event).

#![warn(missing_docs)]

pub mod cross_check;
pub mod derive;
pub mod tape;

pub use derive::{ObservedInstruction, derive};
pub use tape::{BookDelta, Posted, Removal, RemovalReason, Trade};
