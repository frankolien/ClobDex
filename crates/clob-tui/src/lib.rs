//! A terminal view of a ClobDex market.
//!
//! Read-only, and deliberately so. Everything here comes from `clob-stream` over HTTP and a
//! WebSocket, and no key is ever loaded — so this can be left running on a screen, pointed
//! at somebody else's wallet, or screenshotted, without any of those being a decision about
//! custody.
//!
//! The parts worth testing are separated from the parts that need a terminal: [`feed`] is a
//! pure reducer and [`lots`] is pure arithmetic, both covered without a TTY or a network.

pub mod app;
pub mod feed;
pub mod indexer;
pub mod lots;
pub mod terminal;
pub mod ui;
pub mod wire;
