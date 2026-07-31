//! Crankless matching engine and atomic settlement for on-chain CLOBs.
//!
//! Built on [`clob_book`], which supplies the data structures. This crate supplies the
//! policy: order types, self-trade rules, fees, seats, and settlement.

#![no_std]
#![warn(missing_docs)]

#[cfg(any(feature = "std", test))]
extern crate std;

pub mod error;
pub mod fees;
pub mod fill;
pub mod order;
pub mod trader;

pub use error::{EngineError, Result};
pub use fees::{BPS_DENOMINATOR, FeeSchedule};
pub use fill::{Fill, FillObserver, MatchStop, OrderOutcome};
pub use order::{OrderPacket, PostOnlyRejection, SelfTradeBehavior};
pub use trader::{NO_SEAT, SeatIndex, TraderKey, TraderState, TraderTable};
