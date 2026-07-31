//! Crankless matching engine and atomic settlement for on-chain CLOBs.
//!
//! Built on [`clob_book`], which supplies the data structures. This crate supplies the
//! policy: order types, self-trade rules, fees, seats, and settlement.

#![no_std]
#![warn(missing_docs)]

#[cfg(any(feature = "std", test))]
extern crate std;

pub mod error;

pub use error::{EngineError, Result};
