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

#![warn(missing_docs)]

pub mod address;
