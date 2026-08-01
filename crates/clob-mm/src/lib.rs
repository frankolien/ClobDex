//! A market maker for a ClobDex market.
//!
//! It exists to give the venue the one thing hand-placed orders cannot: a book that is
//! two-sided all the time. Everything downstream of that — compute under load, indexer
//! throughput, candles with a shape — is only measurable against continuous traffic.
//!
//! # What it will not do
//!
//! It never takes. Every quote is post-only, and [`Params::validate`] refuses a
//! configuration that could produce a crossing quote in the first place. A maker that
//! crosses the spread has mispriced and is paying a fee to find out.

#![deny(missing_docs)]

pub mod params;

pub use params::{Params, ParamsError};
