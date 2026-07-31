//! Client SDK for the ClobDex spot market.
//!
//! Three things a client needs and the program deliberately does not provide: typed
//! instruction builders, decoding for market accounts, and decoding for trade events.
//!
//! # One copy of the wire format
//!
//! This crate depends on [`clob_program`] rather than restating discriminants and byte
//! offsets. Two copies of a byte layout is how a client and a program drift apart; one
//! copy cannot. The cost is that the SDK pulls in the program crate, which is small and
//! compiles on the host.
//!
//! # Example
//!
//! ```no_run
//! use clob_book::{BaseLots, Side, Ticks};
//! use clob_client::instruction::{self, MarketAddresses, Receipt};
//! use solana_pubkey::Pubkey;
//!
//! let addresses = MarketAddresses::new(
//!     Pubkey::new_unique(), // program
//!     Pubkey::new_unique(), // market
//!     Pubkey::new_unique(), // base vault
//!     Pubkey::new_unique(), // quote vault
//! );
//! let trader = Pubkey::new_unique();
//!
//! // A resting quote: two accounts, no event.
//! let quote = instruction::place_order(
//!     &addresses,
//!     &trader,
//!     &instruction::limit(Side::Bid, Ticks(100), BaseLots(10)),
//!     Receipt::Off,
//! );
//! assert_eq!(quote.accounts.len(), 2);
//!
//! // A taker who wants a receipt: four.
//! let take = instruction::place_order(
//!     &addresses,
//!     &trader,
//!     &instruction::market_order(Side::Bid, BaseLots(10), 16),
//!     Receipt::On,
//! );
//! assert_eq!(take.accounts.len(), 4);
//! ```

#![warn(missing_docs)]

pub mod address;
pub mod decode;
pub mod event;
pub mod instruction;
pub mod setup;
pub mod state;

pub use decode::{ClobInstruction, InstructionDecodeError};
pub use event::{EventError, FillRecord, OrderPlaced};
pub use instruction::{MarketAddresses, Receipt, TOKEN_PROGRAM_ID};
pub use setup::{CreateMarketParams, MarketSetup, TOKEN_ACCOUNT_LEN, create_market};
pub use state::{BookOrder, DecodeError, Level, MarketState, Sweep};
