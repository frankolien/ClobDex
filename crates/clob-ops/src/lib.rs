//! What every ClobDex tool needs before it can do anything on a cluster.
//!
//! Three things, and no strategy: where the cluster is and who signs
//! ([`config`]), how to send and read ([`rpc`]), and which accounts make up the
//! market ([`record`]).
//!
//! It exists because the CLI and the market maker are two programs against the same
//! market. The record in particular is a contract between them — `create-market` writes
//! the file and everything else reads it — and a contract kept in two places is one that
//! eventually disagrees with itself.
//!
//! Nothing here decides what to trade. That belongs to the tool, and keeping it out is
//! what lets a third one be written without inheriting a second one's opinions.

#![deny(missing_docs)]

pub mod config;
pub mod record;
pub mod rpc;

pub use config::Config;
pub use record::{MarketRecord, TraderRecord};
pub use rpc::Client;
