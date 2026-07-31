//! Zero-copy limit order book primitives for on-chain CLOBs.
//!
//! This crate is the data layer of a Solana central limit order book: the structures a
//! market account *is*, with no matching policy and no Solana dependency. The matching
//! engine, settlement, and seat model are built on top of it.
//!
//! # Layers
//!
//! - [`quantities`] — integer price/size units and the exact tick↔lot conversion.
//! - [`order`] — order identity, side, and resting-order state.
//! - [`tree`] — a fixed-capacity red-black tree over a `Pod` node arena.
//! - [`book`] — the two-sided book: bids, asks, and a shared sequence counter.
//!
//! # Three constraints that shaped it
//!
//! 1. **A market account is cast, not deserialized.** Every type is `#[repr(C)]` and
//!    `bytemuck::Pod`. Borsh-decoding a 100k-order book would exhaust the compute
//!    budget before matching a single order.
//! 2. **Zeroed memory is already valid.** A new Solana account is all zeros, and an
//!    all-zero book reads back as empty at full capacity — so there is no init pass.
//! 3. **Every operation has a bounded worst case.** Capacities are const generics, so
//!    rent and maximum compute cost are known at market creation.
//!
//! # Example
//!
//! ```
//! use clob_book::{BaseLots, OrderBook, RestingOrder, Side, Ticks};
//!
//! let mut book = OrderBook::<64, 64>::new_boxed();
//!
//! book.place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(5)));
//! book.place(Side::Bid, Ticks(101), RestingOrder::new(1, BaseLots(3)));
//! book.place(Side::Ask, Ticks(103), RestingOrder::new(2, BaseLots(7)));
//!
//! // Best bid is the highest price; best ask is the lowest.
//! assert_eq!(book.best_bid().unwrap().key.price_in_ticks, Ticks(101));
//! assert_eq!(book.best_ask().unwrap().key.price_in_ticks, Ticks(103));
//! assert_eq!(book.spread_in_ticks(), Some(2));
//! ```

#![no_std]
#![warn(missing_docs)]

#[cfg(any(feature = "std", test))]
extern crate std;

pub mod book;
pub mod order;
pub mod quantities;
pub mod tree;

pub use book::{OrderBook, Reduction};
pub use order::{FIFOOrderId, RestingOrder, Side};
pub use quantities::{BaseAtoms, BaseLots, LotConfig, LotConfigError, QuoteAtoms, QuoteLots, Ticks};
pub use tree::{Entry, Handle, Invariant, NIL, RedBlackTree};
