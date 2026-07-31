//! Crankless matching engine and atomic settlement for on-chain CLOBs.
//!
//! Built on [`clob_book`], which supplies the data structures. This crate supplies the
//! policy: order types, self-trade rules, fees, seats, and settlement.
//!
//! # Crankless
//!
//! Serum matched on-chain but could not settle synchronously, so fills went into an
//! event queue that an off-chain *crank* had to consume. That meant an operational
//! dependency, a capacity cliff when the queue filled, and proceeds that were not
//! spendable until someone else ran a process.
//!
//! Here a fill moves value between two seats' balances inside the taker's transaction.
//! What makes that safe is that maker funds are locked at placement: by the time a
//! resting order is hit, the value behind it is already committed, so settlement is a
//! transfer that cannot fail for lack of funds. No queue, no crank, no second step.
//!
//! # The invariant
//!
//! [`Market::check_conservation`] states it: seat balances sum to the deposited totals,
//! locked funds correspond exactly to resting orders, and every order has a live owner.
//! If a sequence of operations can break it, the market can be drained. It is asserted
//! after every operation in the property tests.
//!
//! # This crate assumes the caller can revert
//!
//! [`Market::place_order`] may mutate the market and *then* return an error — the
//! fill-or-kill path cannot know it fell short until it has already matched. On Solana
//! that is free and correct, because a returned error aborts the instruction and every
//! account write is discarded. Off-chain callers must discard the market on error
//! instead of continuing to use it. Conservation holds either way.
//!
//! # Example
//!
//! ```
//! use clob_engine::{FeeSchedule, Market, OrderPacket, TraderKey};
//! use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
//!
//! let lots = LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap();
//! let mut market = Market::<32, 32, 8>::new_boxed(lots, FeeSchedule::new(2).unwrap()).unwrap();
//!
//! let maker = market.claim_seat(TraderKey([1; 32])).unwrap();
//! let taker = market.claim_seat(TraderKey([2; 32])).unwrap();
//! market.deposit(maker, BaseLots(100), QuoteLots::ZERO).unwrap();
//! market.deposit(taker, BaseLots::ZERO, QuoteLots(1_000_000)).unwrap();
//!
//! // The maker rests an offer, then the taker lifts half of it.
//! market
//!     .place_order(maker, OrderPacket::limit(Side::Ask, Ticks(100), BaseLots(10)), &mut ())
//!     .unwrap();
//! let outcome = market
//!     .place_order(taker, OrderPacket::market(Side::Bid, BaseLots(4)), &mut ())
//!     .unwrap();
//!
//! assert_eq!(outcome.base_lots_filled, BaseLots(4));
//! // Settled immediately: the maker can spend the proceeds in the next instruction.
//! assert_eq!(market.traders().state(maker).unwrap().quote_lots_free, QuoteLots(400));
//! assert_eq!(market.check_conservation(), Ok(()));
//! ```

#![no_std]
#![warn(missing_docs)]

#[cfg(any(feature = "std", test))]
extern crate std;

pub mod error;
pub mod fees;
pub mod fill;
pub mod market;
pub mod order;
pub mod trader;

pub use error::{EngineError, Result};
pub use fees::{BPS_DENOMINATOR, FeeSchedule};
pub use fill::{Fill, FillObserver, MatchStop, OrderOutcome};
pub use market::{ConservationError, Market, MarketHeader};
pub use order::{OrderPacket, PostOnlyRejection, SelfTradeBehavior};
pub use trader::{NO_SEAT, SeatIndex, TraderKey, TraderState, TraderTable};
