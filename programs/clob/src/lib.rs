//! Solana program exposing a crankless, atomically-settled spot CLOB.
//!
//! The matching logic lives in [`clob_engine`]; this crate is the Solana surface around
//! it — accounts, instruction encoding, token transfers, and the zero-copy load that
//! turns a market account into a live market.
//!
//! # Two accounts to trade
//!
//! Placing, cancelling and reducing an order each take exactly two accounts: the market
//! and the trader's signature. No token accounts, no vaults, no token program, because
//! funds were deposited beforehand and settlement happens inside the market account.
//! Aggregators price account count into routing, and this is the cheapest a venue can
//! be on that axis.
//!
//! # Nothing is deserialized
//!
//! A market account is `[header][engine market]`, and the engine market is cast in
//! place with `bytemuck`. Borsh-decoding a book with thousands of resting orders would
//! exhaust the compute budget before matching anything.
//!
//! Vault addresses are recorded in the header at creation, so verifying a vault is a
//! 32-byte comparison rather than a `find_program_address` call — which would otherwise
//! cost around a thousand compute units on every deposit and withdrawal.

#![cfg_attr(target_os = "solana", no_std)]
#![warn(missing_docs)]

pub mod error;
pub mod state;
