//! Zero-copy limit order book primitives for on-chain CLOBs.
//!
//! This is the data layer of a Solana central limit order book: the structures a market
//! account *is*. No matching policy, no settlement, no Solana dependency.

#![no_std]
#![warn(missing_docs)]

#[cfg(any(feature = "std", test))]
extern crate std;

pub mod book;
pub mod order;
pub mod quantities;
pub mod tree;
